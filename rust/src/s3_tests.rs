//! Integration tests for `S3Fs` against Backblaze B2 (or any S3-compatible store).
//!
//! These tests require the following environment variables to be set:
//!   - `FSSPEC_S3_KEY` — access key id
//!   - `FSSPEC_S3_SECRET` — secret access key
//!   - `FSSPEC_S3_ENDPOINT_URL` — endpoint URL (e.g. https://s3.us-east-005.backblazeb2.com)
//!
//! They are skipped automatically (via `#[ignore]`) in CI unless you run with
//! `cargo test -- --ignored` explicitly.

use std::env;

use crate::error::FsError;
use crate::fs::FileSystem;
use crate::s3::{S3Config, S3Fs};

/// Helper: build an S3Fs from environment variables.
/// Returns `None` if the required env vars are not set, so tests can skip.
fn make_s3fs() -> Option<S3Fs> {
    let key = env::var("FSSPEC_S3_KEY").ok()?;
    let secret = env::var("FSSPEC_S3_SECRET").ok()?;
    let endpoint = env::var("FSSPEC_S3_ENDPOINT_URL").ok()?;
    let bucket = env::var("FSSPEC_S3_BUCKET").unwrap_or_else(|_| "timkpaine-public".into());
    let region = env::var("FSSPEC_S3_REGION").unwrap_or_else(|_| "us-east-005".into());

    let mut cfg = S3Config::new(bucket);
    cfg.access_key_id = Some(key);
    cfg.secret_access_key = Some(secret);
    cfg.endpoint_url = Some(endpoint);
    cfg.region = Some(region);

    Some(S3Fs::new(cfg).expect("failed to construct S3Fs"))
}

fn expected_file_count() -> usize {
    env::var("FSSPEC_S3_EXPECTED_FILE_COUNT")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(64)
}

/// Skip the test if credentials are not set.
macro_rules! require_s3 {
    () => {
        match make_s3fs() {
            Some(fs) => fs,
            None => {
                eprintln!("SKIP: S3 credentials not set");
                return;
            }
        }
    };
}

// ============================================================================
// Protocol / metadata
// ============================================================================

#[test]
#[ignore]
fn test_s3_protocol() {
    let fs = require_s3!();
    assert_eq!(fs.protocol(), &["s3"]);
}

#[test]
#[ignore]
fn test_s3_strip_protocol() {
    let fs = require_s3!();
    assert_eq!(
        fs.strip_protocol("s3://timkpaine-public/projects/organizeit2"),
        "projects/organizeit2"
    );
    assert_eq!(
        fs.strip_protocol("timkpaine-public/projects/organizeit2"),
        "projects/organizeit2"
    );
}

#[test]
#[ignore]
fn test_s3_unstrip_protocol() {
    let fs = require_s3!();
    assert_eq!(
        fs.unstrip_protocol("projects/organizeit2"),
        "s3://timkpaine-public/projects/organizeit2"
    );
}

// ============================================================================
// info
// ============================================================================

#[test]
#[ignore]
fn test_s3_info_bucket_root() {
    let fs = require_s3!();
    let info = fs.info("s3://timkpaine-public").unwrap();
    assert!(info.is_dir());
}

#[test]
#[ignore]
fn test_s3_info_directory() {
    let fs = require_s3!();
    let info = fs
        .info("s3://timkpaine-public/projects/organizeit2")
        .unwrap();
    assert!(info.is_dir());
}

#[test]
#[ignore]
fn test_s3_info_file() {
    let fs = require_s3!();
    // This file should exist — it's part of the organizeit2 test data
    // (the test files are 0-byte placeholders)
    let info = fs
        .info("s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt")
        .unwrap();
    assert!(info.is_file());
    assert_eq!(info.size, 0);
}

#[test]
#[ignore]
fn test_s3_info_not_found() {
    let fs = require_s3!();
    let result = fs.info("s3://timkpaine-public/nonexistent/path/that/doesnt/exist");
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), FsError::NotFound(_)));
}

// ============================================================================
// ls
// ============================================================================

#[test]
#[ignore]
fn test_s3_ls_top_level() {
    let fs = require_s3!();
    let entries = fs
        .ls("s3://timkpaine-public/projects/organizeit2", true)
        .unwrap();
    assert!(!entries.is_empty());
    // Should have subdirs: subdir1, subdir2, subdir3, subdir4
    let dir_names: Vec<&str> = entries
        .iter()
        .filter(|e| e.is_dir())
        .map(|e| e.name.as_str())
        .collect();
    assert!(
        dir_names.len() >= 4,
        "expected at least 4 subdirs, got: {dir_names:?}"
    );
}

#[test]
#[ignore]
fn test_s3_ls_subdir() {
    let fs = require_s3!();
    let entries = fs
        .ls("s3://timkpaine-public/projects/organizeit2/subdir1", true)
        .unwrap();
    assert!(!entries.is_empty());
    // Should contain files
    let file_count = entries.iter().filter(|e| e.is_file()).count();
    assert!(file_count > 0, "expected files in subdir1");
}

// ============================================================================
// exists / isdir / isfile
// ============================================================================

#[test]
#[ignore]
fn test_s3_exists() {
    let fs = require_s3!();
    assert!(fs
        .exists("s3://timkpaine-public/projects/organizeit2")
        .unwrap());
    assert!(!fs
        .exists("s3://timkpaine-public/nonexistent-abc-xyz")
        .unwrap());
}

#[test]
#[ignore]
fn test_s3_isdir() {
    let fs = require_s3!();
    assert!(fs
        .isdir("s3://timkpaine-public/projects/organizeit2")
        .unwrap());
    assert!(!fs
        .isdir("s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt")
        .unwrap());
}

#[test]
#[ignore]
fn test_s3_isfile() {
    let fs = require_s3!();
    assert!(fs
        .isfile("s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt")
        .unwrap());
    assert!(!fs
        .isfile("s3://timkpaine-public/projects/organizeit2")
        .unwrap());
}

// ============================================================================
// cat_file / head / tail / read_text
// ============================================================================

#[test]
#[ignore]
fn test_s3_cat_file() {
    let fs = require_s3!();
    // The test files are 0-byte placeholders; just check the call succeeds.
    let data = fs
        .cat_file(
            "s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt",
            None,
            None,
        )
        .unwrap();
    assert!(data.is_empty());
}

#[test]
#[ignore]
fn test_s3_head() {
    let fs = require_s3!();
    // 0-byte file: head returns 0 bytes
    let data = fs
        .head(
            "s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt",
            5,
        )
        .unwrap();
    assert_eq!(data.len(), 0);
}

#[test]
#[ignore]
fn test_s3_tail() {
    let fs = require_s3!();
    // 0-byte file: tail returns 0 bytes
    let data = fs
        .tail(
            "s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt",
            3,
        )
        .unwrap();
    assert_eq!(data.len(), 0);
}

#[test]
#[ignore]
fn test_s3_read_text() {
    let fs = require_s3!();
    // 0-byte file: read_text returns empty string
    let text = fs
        .read_text("s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt")
        .unwrap();
    assert!(text.is_empty());
}

// ============================================================================
// open (read)
// ============================================================================

#[test]
#[ignore]
fn test_s3_open_read() {
    use std::io::Read;
    let fs = require_s3!();
    // 0-byte file: just verify the open/read pipeline works
    let mut f = fs
        .open(
            "s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt",
            crate::types::OpenMode::Read,
            None,
        )
        .unwrap();
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).unwrap();
    assert!(buf.is_empty());
}

// ============================================================================
// find / walk
// ============================================================================

#[test]
#[ignore]
fn test_s3_find() {
    let fs = require_s3!();
    let files = fs
        .find("s3://timkpaine-public/projects/organizeit2", None, false)
        .unwrap();
    // organizeit2 test data has 64 files (from the test_backends.py assertion)
    assert_eq!(
        files.len(),
        expected_file_count(),
        "expected {} files, got {}",
        expected_file_count(),
        files.len()
    );
}

#[test]
#[ignore]
fn test_s3_walk() {
    let fs = require_s3!();
    let entries = fs
        .walk("s3://timkpaine-public/projects/organizeit2", None, true)
        .unwrap();
    assert!(!entries.is_empty());
    // Check we see the top-level directory
    assert_eq!(
        entries[0].dirpath,
        "s3://timkpaine-public/projects/organizeit2"
    );
    // Should have subdirs
    assert!(entries[0].dirnames.len() >= 4);
}

// ============================================================================
// size
// ============================================================================

#[test]
#[ignore]
fn test_s3_size() {
    let fs = require_s3!();
    // 0-byte placeholder file
    let sz = fs
        .size("s3://timkpaine-public/projects/organizeit2/subdir1/file1.txt")
        .unwrap();
    assert_eq!(sz, 0);
}
