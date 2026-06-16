use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::time::{SystemTime, UNIX_EPOCH};

use fsspec_rs::{FileSystem, FileType, FsError, OpenMode};
use fsspec_rs_bridge::{url_to_fs, PyFsspecFs};

fn storage_options() -> HashMap<String, String> {
    HashMap::new()
}

fn memory_fs() -> PyFsspecFs {
    PyFsspecFs::from_protocol("memory", &storage_options()).unwrap()
}

fn unique_dir(name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("/fsspec-rs-bridge-{nanos}-{name}")
}

#[test]
fn protocol_metadata_comes_from_python_filesystem() {
    let fs = memory_fs();

    assert_eq!(fs.protocol(), &["python"]);
    assert_eq!(fs.source_protocol(), "memory");
    assert_eq!(fs.root_marker(), "/");
    assert_eq!(fs.sep(), "/");
}

#[test]
fn url_to_fs_builds_filesystem_and_start_path() {
    let (fs, start) = url_to_fs("memory://", &storage_options()).unwrap();

    assert_eq!(fs.source_protocol(), "memory");
    assert_eq!(start, "/");
}

#[test]
fn lists_info_and_reads_python_filesystem_entries() {
    let fs = memory_fs();
    let dir = unique_dir("read");
    let path = format!("{dir}/data.txt");
    fs.mkdir(&dir, true).unwrap();
    fs.pipe_file(&path, b"abcdef").unwrap();

    let info = fs.info(&path).unwrap();
    assert_eq!(info.name, path);
    assert_eq!(info.size, 6);
    assert_eq!(info.file_type, FileType::File);
    assert!(info.extra.contains_key("created"));

    let names = fs
        .ls(&dir, true)
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .collect::<Vec<_>>();
    assert_eq!(names, vec![path.clone()]);

    assert_eq!(fs.cat_file(&path, None, None).unwrap(), b"abcdef");
    assert_eq!(fs.cat_file(&path, Some(1), Some(4)).unwrap(), b"bcd");
    assert_eq!(fs.head(&path, 2).unwrap(), b"ab");
    assert_eq!(fs.tail(&path, 2).unwrap(), b"ef");
}

#[test]
fn copies_removes_and_removes_directories() {
    let fs = memory_fs();
    let dir = unique_dir("mutate");
    let src = format!("{dir}/source.txt");
    let dst = format!("{dir}/copy.txt");
    fs.mkdir(&dir, true).unwrap();
    fs.pipe_file(&src, b"copy me").unwrap();

    fs.cp_file(&src, &dst).unwrap();
    assert_eq!(fs.cat_file(&dst, None, None).unwrap(), b"copy me");

    fs.rm_file(&src).unwrap();
    fs.rm_file(&dst).unwrap();
    fs.rmdir(&dir).unwrap();
    assert!(matches!(fs.info(&dir), Err(FsError::NotFound(_))));
}

#[test]
fn open_supports_read_write_seek_and_metadata() {
    let fs = memory_fs();
    let dir = unique_dir("open");
    let path = format!("{dir}/data.txt");
    fs.mkdir(&dir, true).unwrap();

    {
        let mut file = fs.open(&path, OpenMode::Write, None).unwrap();
        file.write_all(b"hello world").unwrap();
        file.commit().unwrap();
    }

    let mut file = fs.open(&path, OpenMode::Read, None).unwrap();
    assert_eq!(file.size().unwrap(), Some(11));
    assert_eq!(file.info().unwrap().size, 11);
    assert_eq!(file.seek(SeekFrom::Start(6)).unwrap(), 6);

    let mut text = String::new();
    file.read_to_string(&mut text).unwrap();
    assert_eq!(text, "world");
}

#[test]
fn get_and_put_file_delegate_to_python_filesystem() {
    let fs = memory_fs();
    let dir = unique_dir("transfer");
    let remote = format!("{dir}/remote.txt");
    let uploaded = format!("{dir}/uploaded.txt");
    fs.mkdir(&dir, true).unwrap();
    fs.pipe_file(&remote, b"downloaded").unwrap();

    let tmp = tempfile::tempdir().unwrap();
    let local_download = tmp.path().join("download.txt");
    fs.get_file(&remote, local_download.to_str().unwrap())
        .unwrap();
    assert_eq!(std::fs::read(&local_download).unwrap(), b"downloaded");

    let local_upload = tmp.path().join("upload.txt");
    std::fs::write(&local_upload, b"uploaded").unwrap();
    fs.put_file(local_upload.to_str().unwrap(), &uploaded)
        .unwrap();
    assert_eq!(fs.cat_file(&uploaded, None, None).unwrap(), b"uploaded");
}

#[test]
fn python_errors_map_to_fsspec_rs_errors() {
    let fs = memory_fs();
    let path = format!("{}/missing.txt", unique_dir("errors"));

    assert!(matches!(fs.info(&path), Err(FsError::NotFound(_))));
    assert!(matches!(fs.rm_file(&path), Err(FsError::NotFound(_))));
}
