//! Native object reads without filesystem path stripping or directory inference.
use super::*;

/// A read-only view of one S3 filesystem's native client.
/// Keys are relative to its bucket, never filesystem URLs or bucket-prefixed paths.
#[derive(Clone)]
pub struct S3ObjectReader {
    inner: Arc<S3Fs>,
}

impl S3ObjectReader {
    pub(super) fn new(inner: Arc<S3Fs>) -> Self {
        Self { inner }
    }

    fn path(key: &str) -> FsResult<ObjectPath> {
        let parsed = ObjectPath::parse(key).map_err(|e| FsError::InvalidArgument(e.to_string()))?;
        if key.is_empty() || parsed.as_ref() != key || key.contains('\\') || key.contains("://") {
            return Err(FsError::InvalidArgument(
                "expected a nonempty relative object key".into(),
            ));
        }
        Ok(parsed)
    }

    /// Native HEAD: missing objects never trigger a directory listing.
    /// Metadata uses FileInfo as a transport: type is always File, modified is
    /// present, and extra contains the native ETag/version when available.
    pub fn head(&self, key: &str) -> Result<FileInfo, OperationError> {
        let path = Self::path(key)?;
        let meta = self
            .inner
            .rt
            .block_on(self.inner.store.head(&path))
            .map_err(|e| operation_error(e, false))?;
        let mut extra = HashMap::new();
        if let Some(value) = meta.e_tag {
            extra.insert("etag".into(), value);
        }
        if let Some(value) = meta.version {
            extra.insert("version".into(), value);
        }
        Ok(FileInfo {
            name: key.into(),
            size: meta.size,
            file_type: FileType::File,
            created: None,
            modified: Some(meta.last_modified.into()),
            extra,
        })
    }

    /// Strict, nonnegative half-open ranges, in requested order. Oversized ends
    /// are truncated; empty ranges and starts at/past EOF fail. Empty input does
    /// no I/O. Requests share S3Fs's coalescing and endpoint concurrency limits.
    pub fn read_ranges(
        &self,
        key: &str,
        ranges: &[ByteRange],
    ) -> Result<RangeReadResult, OperationError> {
        let path = Self::path(key)?;
        if ranges.iter().any(|r| {
            r.start.is_none_or(|v| v < 0) || r.end.is_none_or(|v| v <= r.start.unwrap_or(-1))
        }) {
            return Err(
                FsError::InvalidArgument("object ranges require 0 <= start < end".into()).into(),
            );
        }
        self.inner
            .read_ranges_at(path, ranges, RangeReadOptions::default(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;

    fn filesystem() -> Arc<S3Fs> {
        Arc::new(S3Fs::from_object_store("bucket", Arc::new(InMemory::new())).unwrap())
    }

    fn range(start: i64, end: i64) -> ByteRange {
        ByteRange {
            start: Some(start),
            end: Some(end),
        }
    }

    #[test]
    fn native_metadata_and_keys_share_the_filesystem_client() {
        let fs = filesystem();
        let key = ObjectPath::parse("bucket/data%2Fname").unwrap();
        fs.rt
            .block_on(fs.store.put(&key, Bytes::from_static(b"0123456789").into()))
            .unwrap();
        let objects = fs.objects();
        let native = fs.rt.block_on(fs.store.head(&key)).unwrap();
        let meta = objects.head("bucket/data%2Fname").unwrap();
        assert_eq!(meta.name, "bucket/data%2Fname");
        assert_eq!(meta.size, native.size);
        assert_eq!(meta.file_type, FileType::File);
        assert_eq!(meta.modified, Some(native.last_modified.into()));
        assert_eq!(meta.extra.get("etag"), native.e_tag.as_ref());
        assert_eq!(meta.extra.get("version"), native.version.as_ref());
        assert_eq!(
            objects.head("bucket").unwrap_err().kind(),
            ErrorKind::NotFound
        );
        drop(fs);
        let result = objects
            .read_ranges(
                "bucket/data%2Fname",
                &[range(7, 99), range(1, 4), range(7, 9)],
            )
            .unwrap();
        assert_eq!(
            result.data,
            [b"789".to_vec(), b"123".to_vec(), b"78".to_vec()]
        );
    }

    #[test]
    fn strict_ranges_reject_invalid_bounds_and_eof() {
        let fs = filesystem();
        fs.upload_data("data", b"012").unwrap();
        let objects = fs.objects();
        for range in [
            range(-1, 1),
            range(0, 0),
            range(2, 1),
            range(3, 4),
            range(4, 5),
            ByteRange {
                start: None,
                end: Some(1),
            },
            ByteRange {
                start: Some(0),
                end: None,
            },
        ] {
            assert_eq!(
                objects.read_ranges("data", &[range]).unwrap_err().kind(),
                ErrorKind::Invalid
            );
        }
        assert!(objects.read_ranges("missing", &[]).unwrap().data.is_empty());
    }

    #[test]
    fn object_paths_never_normalize_or_strip() {
        let fs = filesystem();
        let objects = fs.objects();
        for key in [
            "",
            "/a",
            "a/",
            "a//b",
            "a/./b",
            "a/../b",
            "a\\b",
            "s3://bucket/a",
            "a\0b",
        ] {
            assert_eq!(
                objects.head(key).unwrap_err().kind(),
                ErrorKind::Invalid,
                "{key:?}"
            );
        }
        fs.upload_data("bucket", b"key").unwrap();
        assert_eq!(objects.head("bucket").unwrap().size, 3);
    }
}
