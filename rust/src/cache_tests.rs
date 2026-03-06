//! Tests for the caching module and BufferedFile.

use std::io::{Read, Seek, SeekFrom, Write};

use crate::buffered::BufferedFile;
use crate::caching::*;
use crate::file::FsFile;

// ---------------------------------------------------------------------------
// Helper: build a fetcher backed by a fixed byte vector
// ---------------------------------------------------------------------------
fn fake_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

fn fake_fetcher(data: Vec<u8>) -> Fetcher {
    Box::new(move |start: u64, end: u64| {
        let s = start as usize;
        let e = (end as usize).min(data.len());
        if s >= data.len() {
            return Ok(Vec::new());
        }
        Ok(data[s..e].to_vec())
    })
}

// ==========================================================================
// CacheType parsing
// ==========================================================================

#[test]
fn test_cache_type_from_str() {
    assert_eq!(CacheType::from_str("none"), Some(CacheType::None));
    assert_eq!(CacheType::from_str("readahead"), Some(CacheType::ReadAhead));
    assert_eq!(
        CacheType::from_str("READ_AHEAD"),
        Some(CacheType::ReadAhead)
    );
    assert_eq!(CacheType::from_str("block"), Some(CacheType::Block));
    assert_eq!(CacheType::from_str("BLOCKCACHE"), Some(CacheType::Block));
    assert_eq!(CacheType::from_str("all"), Some(CacheType::All));
    assert_eq!(CacheType::from_str("bytes"), Some(CacheType::All));
    assert_eq!(CacheType::from_str("unknown"), None);
}

// ==========================================================================
// NoCache
// ==========================================================================

#[test]
fn test_no_cache_basic() {
    let data = fake_data(100);
    let mut cache = NoCache::new(fake_fetcher(data.clone()), Some(100));
    assert_eq!(cache.size(), Some(100));
    let result = cache.fetch(10, 20).unwrap();
    assert_eq!(result, &data[10..20]);
}

#[test]
fn test_no_cache_clamps_to_size() {
    let data = fake_data(50);
    let mut cache = NoCache::new(fake_fetcher(data.clone()), Some(50));
    let result = cache.fetch(40, 100).unwrap();
    assert_eq!(result, &data[40..50]);
}

#[test]
fn test_no_cache_empty_on_past_eof() {
    let data = fake_data(50);
    let mut cache = NoCache::new(fake_fetcher(data.clone()), Some(50));
    let result = cache.fetch(60, 70).unwrap();
    assert!(result.is_empty());
}

// ==========================================================================
// ReadAheadCache
// ==========================================================================

#[test]
fn test_readahead_sequential() {
    let data = fake_data(1000);
    let mut cache = ReadAheadCache::new(fake_fetcher(data.clone()), Some(1000), 256);

    // First read triggers fetch of [0, 256)
    let r1 = cache.fetch(0, 10).unwrap();
    assert_eq!(r1, &data[0..10]);

    // Second read is within cached block [0, 256)
    let r2 = cache.fetch(10, 50).unwrap();
    assert_eq!(r2, &data[10..50]);

    // Read beyond cached block: partial hit + extend
    let r3 = cache.fetch(200, 300).unwrap();
    assert_eq!(r3, &data[200..300]);
}

#[test]
fn test_readahead_miss_discards() {
    let data = fake_data(1000);
    let mut cache = ReadAheadCache::new(fake_fetcher(data.clone()), Some(1000), 100);

    // Fill cache at [0, 100)
    let _ = cache.fetch(0, 10).unwrap();

    // Seek far away — miss, discards old cache
    let r = cache.fetch(500, 520).unwrap();
    assert_eq!(r, &data[500..520]);
}

// ==========================================================================
// BlockCache
// ==========================================================================

#[test]
fn test_blockcache_basic() {
    let data = fake_data(1000);
    let mut cache = BlockCache::new(fake_fetcher(data.clone()), Some(1000), 100, 4);
    assert_eq!(cache.size(), Some(1000));

    // Single block
    let r = cache.fetch(50, 80).unwrap();
    assert_eq!(r, &data[50..80]);
}

#[test]
fn test_blockcache_cross_block() {
    let data = fake_data(1000);
    let mut cache = BlockCache::new(fake_fetcher(data.clone()), Some(1000), 100, 4);

    // Range spans block 0 [0,100) and block 1 [100,200)
    let r = cache.fetch(80, 150).unwrap();
    assert_eq!(r, &data[80..150]);
}

#[test]
fn test_blockcache_lru_eviction() {
    let data = fake_data(1000);
    let mut cache = BlockCache::new(fake_fetcher(data.clone()), Some(1000), 100, 2);

    // Load block 0
    let _ = cache.fetch(0, 10).unwrap();
    // Load block 1
    let _ = cache.fetch(100, 110).unwrap();
    // Load block 2 → should evict block 0 (LRU, max_blocks=2)
    let _ = cache.fetch(200, 210).unwrap();

    // Block 0 should have been evicted, block 1 and 2 remain
    // We can't directly inspect internals, but the result should be correct
    let r = cache.fetch(0, 10).unwrap();
    assert_eq!(r, &data[0..10]); // re-fetched
}

// ==========================================================================
// AllBytesCache
// ==========================================================================

#[test]
fn test_allbytes_basic() {
    let data = fake_data(500);
    let mut cache = AllBytesCache::new(fake_fetcher(data.clone()), Some(500));
    assert_eq!(cache.size(), Some(500));

    let r = cache.fetch(100, 200).unwrap();
    assert_eq!(r, &data[100..200]);
}

#[test]
fn test_allbytes_multiple_reads() {
    let data = fake_data(500);
    let mut cache = AllBytesCache::new(fake_fetcher(data.clone()), Some(500));

    let r1 = cache.fetch(0, 100).unwrap();
    assert_eq!(r1, &data[0..100]);

    let r2 = cache.fetch(400, 500).unwrap();
    assert_eq!(r2, &data[400..500]);
}

#[test]
fn test_allbytes_past_eof() {
    let data = fake_data(100);
    let mut cache = AllBytesCache::new(fake_fetcher(data.clone()), Some(100));

    let r = cache.fetch(50, 200).unwrap();
    assert_eq!(r, &data[50..100]);
}

// ==========================================================================
// make_cache factory
// ==========================================================================

#[test]
fn test_make_cache_none() {
    let data = fake_data(100);
    let mut c = make_cache(
        CacheType::None,
        fake_fetcher(data.clone()),
        Some(100),
        64,
        4,
    );
    let r = c.fetch(0, 10).unwrap();
    assert_eq!(r, &data[0..10]);
}

#[test]
fn test_make_cache_readahead() {
    let data = fake_data(100);
    let mut c = make_cache(
        CacheType::ReadAhead,
        fake_fetcher(data.clone()),
        Some(100),
        64,
        4,
    );
    let r = c.fetch(0, 10).unwrap();
    assert_eq!(r, &data[0..10]);
}

#[test]
fn test_make_cache_block() {
    let data = fake_data(100);
    let mut c = make_cache(
        CacheType::Block,
        fake_fetcher(data.clone()),
        Some(100),
        32,
        4,
    );
    let r = c.fetch(0, 10).unwrap();
    assert_eq!(r, &data[0..10]);
}

#[test]
fn test_make_cache_all() {
    let data = fake_data(100);
    let mut c = make_cache(CacheType::All, fake_fetcher(data.clone()), Some(100), 64, 4);
    let r = c.fetch(0, 10).unwrap();
    assert_eq!(r, &data[0..10]);
}

// ==========================================================================
// BufferedFile — read mode
// ==========================================================================

#[test]
fn test_buffered_file_read_basic() {
    let data = fake_data(200);
    let mut f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data.clone()),
        Some(200),
        CacheType::All,
        64,
        4,
    );

    let mut buf = vec![0u8; 50];
    let n = f.read(&mut buf).unwrap();
    assert_eq!(n, 50);
    assert_eq!(&buf[..n], &data[0..50]);
}

#[test]
fn test_buffered_file_read_sequential() {
    let data = fake_data(200);
    let mut f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data.clone()),
        Some(200),
        CacheType::ReadAhead,
        128,
        4,
    );

    let mut buf = vec![0u8; 50];
    f.read(&mut buf).unwrap();
    assert_eq!(&buf[..50], &data[0..50]);

    f.read(&mut buf).unwrap();
    assert_eq!(&buf[..50], &data[50..100]);
}

#[test]
fn test_buffered_file_seek_and_read() {
    let data = fake_data(500);
    let mut f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data.clone()),
        Some(500),
        CacheType::Block,
        128,
        4,
    );

    f.seek(SeekFrom::Start(100)).unwrap();
    let mut buf = vec![0u8; 20];
    let n = f.read(&mut buf).unwrap();
    assert_eq!(n, 20);
    assert_eq!(&buf[..n], &data[100..120]);

    // Seek from end
    f.seek(SeekFrom::End(-10)).unwrap();
    let n = f.read(&mut buf).unwrap();
    assert_eq!(n, 10);
    assert_eq!(&buf[..n], &data[490..500]);
}

#[test]
fn test_buffered_file_read_eof() {
    let data = fake_data(50);
    let mut f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data.clone()),
        Some(50),
        CacheType::All,
        64,
        4,
    );

    f.seek(SeekFrom::Start(50)).unwrap();
    let mut buf = vec![0u8; 10];
    let n = f.read(&mut buf).unwrap();
    assert_eq!(n, 0); // EOF
}

#[test]
fn test_buffered_file_readable_writable() {
    let data = fake_data(100);
    let f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data),
        Some(100),
        CacheType::All,
        64,
        4,
    );
    assert!(f.readable());
    assert!(!f.writable());
}

#[test]
fn test_buffered_file_info() {
    let data = fake_data(100);
    let f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data),
        Some(100),
        CacheType::All,
        64,
        4,
    );
    let info = f.info().unwrap();
    assert_eq!(info.name, "test.txt");
    assert_eq!(info.size, 100);
}

#[test]
fn test_buffered_file_size() {
    let data = fake_data(100);
    let f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data),
        Some(100),
        CacheType::All,
        64,
        4,
    );
    assert_eq!(f.size().unwrap(), Some(100));
}

// ==========================================================================
// BufferedFile — write mode
// ==========================================================================

#[test]
fn test_buffered_file_write_basic() {
    use std::sync::{Arc, Mutex};

    let uploaded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let uploaded_clone = Arc::clone(&uploaded);

    let uploader = Box::new(move |data: &[u8]| {
        *uploaded_clone.lock().unwrap() = data.to_vec();
        Ok(())
    });

    let mut f = BufferedFile::new_write("out.txt".into(), uploader, false);
    assert!(!f.readable());
    assert!(f.writable());

    f.write_all(b"hello world").unwrap();
    f.commit().unwrap();

    assert_eq!(&*uploaded.lock().unwrap(), b"hello world");
}

#[test]
fn test_buffered_file_write_autocommit() {
    use std::sync::{Arc, Mutex};

    let uploaded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let uploaded_clone = Arc::clone(&uploaded);

    let uploader = Box::new(move |data: &[u8]| {
        *uploaded_clone.lock().unwrap() = data.to_vec();
        Ok(())
    });

    {
        let mut f = BufferedFile::new_write("out.txt".into(), uploader, true);
        f.write_all(b"auto").unwrap();
        // Drop triggers commit
    }

    assert_eq!(&*uploaded.lock().unwrap(), b"auto");
}

#[test]
fn test_buffered_file_write_discard() {
    use std::sync::{Arc, Mutex};

    let uploaded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let uploaded_clone = Arc::clone(&uploaded);

    let uploader = Box::new(move |data: &[u8]| {
        *uploaded_clone.lock().unwrap() = data.to_vec();
        Ok(())
    });

    {
        let mut f = BufferedFile::new_write("out.txt".into(), uploader, true);
        f.write_all(b"discarded").unwrap();
        f.discard().unwrap();
        // Drop should NOT upload because discarded=true
    }

    assert!(uploaded.lock().unwrap().is_empty());
}

#[test]
fn test_buffered_file_write_seek() {
    use std::sync::{Arc, Mutex};

    let uploaded: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let uploaded_clone = Arc::clone(&uploaded);

    let uploader = Box::new(move |data: &[u8]| {
        *uploaded_clone.lock().unwrap() = data.to_vec();
        Ok(())
    });

    let mut f = BufferedFile::new_write("out.txt".into(), uploader, false);
    f.write_all(b"ABCDEF").unwrap();
    f.seek(SeekFrom::Start(0)).unwrap();
    f.write_all(b"XY").unwrap();
    f.commit().unwrap();

    assert_eq!(&*uploaded.lock().unwrap(), b"XYCDEF");
}

#[test]
fn test_buffered_file_write_info() {
    let uploader = Box::new(|_: &[u8]| Ok(()));
    let mut f = BufferedFile::new_write("out.txt".into(), uploader, false);
    f.write_all(b"12345").unwrap();
    let info = f.info().unwrap();
    assert_eq!(info.name, "out.txt");
    assert_eq!(info.size, 5);
    assert_eq!(f.size().unwrap(), Some(5));
}

// ==========================================================================
// BufferedFile — error cases
// ==========================================================================

#[test]
fn test_buffered_file_read_on_write_file() {
    let uploader = Box::new(|_: &[u8]| Ok(()));
    let mut f = BufferedFile::new_write("out.txt".into(), uploader, false);
    let mut buf = vec![0u8; 10];
    let err = f.read(&mut buf).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}

#[test]
fn test_buffered_file_write_on_read_file() {
    let data = fake_data(100);
    let mut f = BufferedFile::new_read(
        "test.txt".into(),
        fake_fetcher(data),
        Some(100),
        CacheType::All,
        64,
        4,
    );
    let err = f.write(b"nope").unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::Unsupported);
}
