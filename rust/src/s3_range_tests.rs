use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::{
    ByteRange, Condition, FileSystem, FsError, OperationError, RangeReadOptions, S3Config, S3Fs,
};

#[derive(Clone, Copy)]
enum Fault {
    None,
    Precondition,
    WrongValidator,
    ShortBody,
    MissingValidator,
}

#[derive(Clone, Debug)]
struct Request {
    method: String,
    range: Option<(usize, usize)>,
    if_match: Option<String>,
}

struct State {
    requests: Vec<Request>,
    active: usize,
    peak: usize,
    released: bool,
    fault: Fault,
}

struct RangeServer {
    endpoint: String,
    state: Arc<(Mutex<State>, Condvar)>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RangeServer {
    fn new(gated: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new((
            Mutex::new(State {
                requests: Vec::new(),
                active: 0,
                peak: 0,
                released: !gated,
                fault: Fault::None,
            }),
            Condvar::new(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let server_state = state.clone();
        let server_stop = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut workers = Vec::new();
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        let state = server_state.clone();
                        workers.push(std::thread::spawn(move || serve(socket, state)));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            for worker in workers {
                worker.join().unwrap();
            }
        });
        Self {
            endpoint,
            state,
            stop,
            thread: Some(thread),
        }
    }

    fn filesystem(&self, bucket: &str) -> S3Fs {
        let mut config = S3Config::new(bucket);
        config.endpoint_url = Some(self.endpoint.clone());
        config.anon = true;
        S3Fs::new(config).unwrap()
    }

    fn fault(&self, fault: Fault) {
        self.state.0.lock().unwrap().fault = fault;
    }

    fn requests(&self) -> Vec<Request> {
        self.state.0.lock().unwrap().requests.clone()
    }

    fn wait_for_gets(&self, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.state.0.lock().unwrap();
        while state
            .requests
            .iter()
            .filter(|request| request.method == "GET")
            .count()
            < count
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            state = self.state.1.wait_timeout(state, remaining).unwrap().0;
        }
        true
    }

    fn release(&self) {
        self.state.0.lock().unwrap().released = true;
        self.state.1.notify_all();
    }
}

impl Drop for RangeServer {
    fn drop(&mut self) {
        self.release();
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn serve(mut socket: TcpStream, state: Arc<(Mutex<State>, Condvar)>) {
    socket.set_nonblocking(false).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    socket
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        if socket.read_exact(&mut byte).is_err() {
            return;
        }
        bytes.push(byte[0]);
    }
    let headers = String::from_utf8(bytes).unwrap();
    let method = headers.split_whitespace().next().unwrap().to_string();
    let header = |name: &str| {
        headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find_map(|(key, value)| {
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
    };
    let range = header("range").map(|value| {
        let (start, end) = value
            .strip_prefix("bytes=")
            .unwrap()
            .split_once('-')
            .unwrap();
        (
            start.parse::<usize>().unwrap(),
            end.parse::<usize>().unwrap() + 1,
        )
    });
    let request = Request {
        method,
        range,
        if_match: header("if-match"),
    };
    let fault = {
        let mut state = state.0.lock().unwrap();
        state.active += 1;
        state.peak = state.peak.max(state.active);
        state.requests.push(request.clone());
        state.fault
    };
    state.1.notify_all();
    let conflict = request
        .if_match
        .as_deref()
        .is_some_and(|etag| etag != "\"stable\"")
        || (request.method == "GET" && matches!(fault, Fault::Precondition));
    if conflict {
        state.0.lock().unwrap().active -= 1;
        let _ = socket.write_all(
            b"HTTP/1.1 412 Precondition Failed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
    } else {
        let (start, end) = request.range.unwrap_or((0, 128));
        let mut body: Vec<_> = (start..end).map(|value| value as u8).collect();
        let etag = match fault {
            Fault::MissingValidator => "",
            Fault::WrongValidator if request.method == "GET" => "ETag: \"changed\"\r\n",
            _ => "ETag: \"stable\"\r\n",
        };
        let content_range = request
            .range
            .map(|_| format!("Content-Range: bytes {start}-{}/128\r\n", end - 1))
            .unwrap_or_default();
        let status = if request.range.is_some() {
            "206 Partial Content"
        } else {
            "200 OK"
        };
        if request.method == "GET" && matches!(fault, Fault::ShortBody) {
            body.pop();
        }
        let headers = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\n{etag}{content_range}Last-Modified: Tue, 17 Nov 2026 00:00:00 GMT\r\nConnection: close\r\n\r\n", body.len());
        if request.method == "HEAD" {
            state.0.lock().unwrap().active -= 1;
        }
        let _ = socket.write_all(headers.as_bytes());
        if request.method == "GET" {
            // Hold bodies after sending headers, so the limit covers body reads too.
            let mut guard = state.0.lock().unwrap();
            while !guard.released {
                guard = state.1.wait(guard).unwrap();
            }
            // Retire before sending the last bytes: the client can release its
            // permit as soon as they arrive, before this worker runs again.
            guard.active -= 1;
            drop(guard);
            let _ = socket.write_all(&body);
        }
    }
    state.1.notify_all();
}

fn range(start: i64, end: i64) -> ByteRange {
    ByteRange {
        start: Some(start),
        end: Some(end),
    }
}

#[test]
fn s3_ranges_coalesce_and_split_requests_with_one_validator() {
    let server = RangeServer::new(false);
    let fs = server.filesystem("bucket");
    let ranges = [
        range(20, 24),
        range(0, 4),
        range(2, 8),
        range(10, 12),
        range(2, 8),
        range(5, 1),
        range(-4, 200),
    ];
    let result = fs
        .read_ranges(
            "key",
            &ranges,
            RangeReadOptions {
                max_gap: 2,
                max_request_size: 6,
                ..Default::default()
            },
        )
        .unwrap();
    let expected: Vec<Vec<u8>> = [
        (20, 24),
        (0, 4),
        (2, 8),
        (10, 12),
        (2, 8),
        (0, 0),
        (124, 128),
    ]
    .into_iter()
    .map(|(start, end)| (start..end).collect())
    .collect();
    assert_eq!(result.data, expected);
    assert_eq!(result.validator.as_ref().unwrap().value, "\"stable\"");
    let requests = server.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.method == "HEAD")
            .count(),
        1
    );
    let gets: Vec<_> = requests
        .iter()
        .filter(|request| request.method == "GET")
        .collect();
    assert_eq!(gets.len(), 4);
    for request in gets {
        let (start, end) = request.range.unwrap();
        assert!(end - start <= 6);
        assert_eq!(request.if_match.as_deref(), Some("\"stable\""));
    }
}

#[test]
fn s3_range_errors_discard_partial_output_and_release_permits() {
    let server = RangeServer::new(false);
    let fs = server.filesystem("bucket");
    for fault in [
        Fault::Precondition,
        Fault::WrongValidator,
        Fault::ShortBody,
        Fault::MissingValidator,
    ] {
        server.fault(fault);
        let result = fs.read_ranges(
            "key",
            &[range(0, 4), range(20, 24)],
            RangeReadOptions {
                max_gap: 0,
                ..Default::default()
            },
        );
        match fault {
            Fault::Precondition | Fault::WrongValidator => assert!(
                matches!(result, Err(OperationError::Conflict(_))),
                "{result:?}"
            ),
            Fault::MissingValidator => assert!(
                matches!(
                    result,
                    Err(OperationError::FileSystem(FsError::NotSupported(_)))
                ),
                "{result:?}"
            ),
            Fault::ShortBody => assert!(result.is_err()),
            Fault::None => unreachable!(),
        }
    }
    server.fault(Fault::None);
    assert_eq!(
        fs.read_ranges("key", &[range(0, 4)], RangeReadOptions::default())
            .unwrap()
            .data,
        vec![vec![0, 1, 2, 3]]
    );
}

#[test]
fn s3_empty_ranges_still_validate_conditions_and_options() {
    let server = RangeServer::new(false);
    let fs = server.filesystem("bucket");
    assert!(fs
        .read_ranges("key", &[], RangeReadOptions::default())
        .unwrap()
        .data
        .is_empty());
    for options in [
        RangeReadOptions {
            max_concurrency: 0,
            ..Default::default()
        },
        RangeReadOptions {
            max_request_size: 0,
            ..Default::default()
        },
        RangeReadOptions {
            condition: Some(Condition::Missing),
            ..Default::default()
        },
    ] {
        assert!(matches!(
            fs.read_ranges("key", &[], options),
            Err(OperationError::FileSystem(FsError::InvalidArgument(_)))
        ));
    }
    assert!(server.requests().is_empty());
    let validator = fs
        .read_ranges("key", &[range(1, 1)], RangeReadOptions::default())
        .unwrap()
        .validator
        .unwrap();
    assert_eq!(server.requests().len(), 1);
    let options = RangeReadOptions {
        condition: Some(Condition::Match(validator.clone())),
        ..Default::default()
    };
    let result = fs.read_ranges("key", &[], options).unwrap();
    assert_eq!(result.validator, Some(validator.clone()));
    assert_eq!(server.requests().len(), 2);
    let options = RangeReadOptions {
        condition: Some(Condition::Match(validator.clone())),
        ..Default::default()
    };
    assert!(matches!(
        fs.read_ranges("other-key", &[], options),
        Err(OperationError::FileSystem(FsError::InvalidArgument(_)))
    ));
    assert_eq!(server.requests().len(), 2);
    let stale = crate::Validator {
        value: "stale".into(),
        ..validator
    };
    let options = RangeReadOptions {
        condition: Some(Condition::Match(stale)),
        ..Default::default()
    };
    assert!(matches!(
        fs.read_ranges("key", &[], options),
        Err(OperationError::Conflict(_))
    ));
}

#[test]
fn s3_range_concurrency_limits_cover_bodies_and_shared_instances() {
    for (callers, per_call, expected) in [(1, 3, 3), (2, 5, 8)] {
        let server = RangeServer::new(true);
        let filesystems: Vec<_> = (0..callers)
            .map(|index| server.filesystem(&format!("bucket{index}")))
            .collect();
        let ranges: Vec<_> = (0..8)
            .map(|index| range(index * 4, index * 4 + 2))
            .collect();
        let (saturated, exceeded, results) = std::thread::scope(|scope| {
            let handles: Vec<_> = filesystems
                .iter()
                .map(|fs| {
                    let ranges = &ranges;
                    scope.spawn(move || {
                        fs.read_ranges(
                            "key",
                            ranges,
                            RangeReadOptions {
                                max_gap: 0,
                                max_request_size: 2,
                                max_concurrency: per_call,
                                ..Default::default()
                            },
                        )
                    })
                })
                .collect();
            let saturated = server.wait_for_gets(expected, Duration::from_secs(5));
            let exceeded = server.wait_for_gets(expected + 1, Duration::from_millis(100));
            server.release();
            let results: Vec<_> = handles
                .into_iter()
                .map(|handle| handle.join().unwrap().unwrap())
                .collect();
            (saturated, exceeded, results)
        });
        assert!(saturated, "range requests were not concurrent");
        assert!(
            !exceeded,
            "concurrency bound was exceeded while bodies were blocked"
        );
        assert!(server.state.0.lock().unwrap().peak <= expected);
        for result in results {
            assert_eq!(result.data.len(), ranges.len());
            for (index, data) in result.data.iter().enumerate() {
                assert_eq!(data, &[index as u8 * 4, index as u8 * 4 + 1]);
            }
        }
    }
}

fn native_async_fs(
    server: &RangeServer,
    limits: crate::async_files::Limits,
) -> crate::async_files::AsyncFilesystem {
    let options = crate::FsOptions::from([
        ("endpoint_url".into(), server.endpoint.clone()),
        ("anon".into(), "true".into()),
        ("max_retries".into(), "0".into()),
    ]);
    let context = crate::async_files::AsyncContext::new(limits).unwrap();
    crate::async_files::AsyncFilesystem::s3("bucket", &options, &context).unwrap()
}

async fn async_file(fs: &crate::async_files::AsyncFilesystem) -> crate::async_files::AsyncFile {
    let operation = fs.open_read("s3://bucket/data", None).unwrap();
    let crate::async_io::Value::File(file) = operation.clone().await.unwrap() else {
        panic!("file expected")
    };
    operation.drain().await;
    file
}

#[tokio::test]
async fn native_async_s3_reads_seek_and_pin_validators() {
    use crate::AsyncFsFile;
    let server = RangeServer::new(false);
    let fs = native_async_fs(&server, Default::default());
    let mut file = async_file(&fs).await;
    assert_eq!(
        AsyncFsFile::read(&mut file, 5).await.unwrap(),
        [0, 1, 2, 3, 4]
    );
    assert_eq!(AsyncFsFile::read(&mut file, 3).await.unwrap(), [5, 6, 7]);
    assert_eq!(
        AsyncFsFile::seek(&mut file, std::io::SeekFrom::End(-2))
            .await
            .unwrap(),
        126
    );
    assert_eq!(AsyncFsFile::read(&mut file, 10).await.unwrap(), [126, 127]);
    let count = server.requests().len();
    assert!(AsyncFsFile::read(&mut file, 10).await.unwrap().is_empty());
    assert!(AsyncFsFile::read(&mut file, 0).await.unwrap().is_empty());
    assert_eq!(server.requests().len(), count);
    let requests: Vec<_> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET")
        .collect();
    assert_eq!(
        requests
            .iter()
            .map(|r| r.range.unwrap())
            .collect::<Vec<_>>(),
        [(0, 5), (5, 8), (126, 128)]
    );
    assert!(requests
        .iter()
        .all(|r| r.if_match.as_deref() == Some("\"stable\"")));
    assert_eq!(AsyncFsFile::info(&file).await.unwrap().size, 128);
    AsyncFsFile::close(&mut file).await.unwrap();
    fs.close(None).unwrap().await.unwrap();
}

#[tokio::test]
async fn native_async_s3_rejects_changed_or_malformed_reads() {
    use crate::{AsyncFsFile, ErrorKind};
    for fault in [
        Fault::Precondition,
        Fault::WrongValidator,
        Fault::MissingValidator,
        Fault::ShortBody,
    ] {
        let server = RangeServer::new(false);
        let fs = native_async_fs(&server, Default::default());
        let mut file = async_file(&fs).await;
        server.fault(fault);
        let operation = file.read(8, None).unwrap();
        let error = operation.clone().await.unwrap_err();
        operation.drain().await;
        if !matches!(fault, Fault::ShortBody) {
            assert_eq!(error.kind(), ErrorKind::Conflict);
        }
        assert_eq!(
            AsyncFsFile::read(&mut file, 8).await.unwrap_err().kind(),
            ErrorKind::Invalid
        );
        file.close(None).unwrap().await.unwrap();
    }
    let server = RangeServer::new(false);
    server.fault(Fault::MissingValidator);
    let fs = native_async_fs(&server, Default::default());
    assert_eq!(
        fs.open_read("data", None)
            .unwrap()
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::NotSupported
    );
    assert_eq!(
        fs.open_read("s3://other/data", None)
            .unwrap()
            .await
            .unwrap_err()
            .kind(),
        ErrorKind::Invalid
    );
}

#[tokio::test]
async fn native_async_s3_accounts_for_output_and_staging_before_dispatch() {
    use crate::async_files::Limits;
    use crate::async_io::Value;
    let server = RangeServer::new(false);
    let fs = native_async_fs(
        &server,
        Limits {
            max_operations: 8,
            max_buffered_bytes: 256,
            max_read_size: 128,
        },
    );
    let first = async_file(&fs).await;
    let second = async_file(&fs).await;
    let first_read = first.read(128, None).unwrap();
    first_read.drain().await;
    let second_read = second.read(128, None).unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(second_read.state(), 0);
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.method == "GET")
            .count(),
        1
    );
    assert!(matches!(
        first_read.take().unwrap().unwrap(),
        Value::Bytes(_)
    ));
    assert!(matches!(
        second_read.clone().await.unwrap(),
        Value::Bytes(_)
    ));
    second_read.drain().await;
    fs.close(None).unwrap().await.unwrap();
}

#[tokio::test]
async fn native_async_s3_cancel_timeout_and_close_drain_a_stalled_body() {
    use crate::ErrorKind;
    for timeout in [false, true] {
        let server = RangeServer::new(true);
        let fs = native_async_fs(&server, Default::default());
        let file = async_file(&fs).await;
        let operation = file
            .read(8, timeout.then_some(Duration::from_millis(100)))
            .unwrap();
        assert!(server.wait_for_gets(1, Duration::from_secs(2)));
        if !timeout {
            operation.cancel();
        }
        assert_eq!(
            operation.clone().await.unwrap_err().kind(),
            if timeout {
                ErrorKind::Timeout
            } else {
                ErrorKind::Cancelled
            }
        );
        tokio::time::timeout(Duration::from_secs(2), operation.drain())
            .await
            .unwrap();
        assert!(operation.is_quiescent());
        tokio::time::timeout(Duration::from_secs(2), file.close(None).unwrap())
            .await
            .unwrap()
            .unwrap();
        server.release();
    }
}

#[tokio::test]
async fn native_async_s3_close_releases_endpoint_capacity() {
    let server = RangeServer::new(true);
    let fs = native_async_fs(&server, Default::default());
    let mut files = Vec::new();
    for _ in 0..9 {
        files.push(async_file(&fs).await);
    }
    let reads: Vec<_> = files
        .iter()
        .map(|file| file.read(8, None).unwrap())
        .collect();
    assert!(server.wait_for_gets(8, Duration::from_secs(2)));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|r| r.method == "GET")
            .count(),
        8
    );
    tokio::time::timeout(Duration::from_secs(2), fs.close(None).unwrap())
        .await
        .unwrap()
        .unwrap();
    for read in reads {
        assert_eq!(
            read.clone().await.unwrap_err().kind(),
            crate::ErrorKind::Cancelled
        );
        read.drain().await;
    }
    server.release();
    let next = native_async_fs(&server, Default::default());
    let mut file = async_file(&next).await;
    assert_eq!(
        crate::AsyncFsFile::read(&mut file, 2).await.unwrap(),
        [0, 1]
    );
}

#[tokio::test]
async fn native_async_s3_rejects_staging_larger_than_context() {
    use crate::async_files::Limits;
    let server = RangeServer::new(false);
    let fs = native_async_fs(
        &server,
        Limits {
            max_operations: 8,
            max_buffered_bytes: 128,
            max_read_size: 128,
        },
    );
    let file = async_file(&fs).await;
    assert_eq!(
        file.read(128, None).err().unwrap().kind(),
        crate::ErrorKind::Invalid
    );
    assert!(!server.requests().iter().any(|r| r.method == "GET"));
}
