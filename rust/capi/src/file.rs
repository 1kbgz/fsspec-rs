use std::io::{Read, Seek, SeekFrom, Write};
use std::slice;

use fsspec_rs::FsFile;

use crate::memory::fsspec_file_info;
use crate::status::{fsspec_status, guard, handle_arg_mut, invalid, write_out};

/// An open file. Close with `fsspec_file_close`.
pub struct fsspec_file {
    pub(crate) inner: Box<dyn FsFile>,
}

impl fsspec_file {
    pub(crate) fn into_raw(inner: Box<dyn FsFile>) -> *mut Self {
        Box::into_raw(Box::new(Self { inner }))
    }
}

/// Reads up to `len` bytes into `buf`. `out_read` receives the count, which is
/// zero at end of file.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_read(
    file: *mut fsspec_file,
    buf: *mut u8,
    len: usize,
    out_read: *mut usize,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        if buf.is_null() && len != 0 {
            return Err(invalid("buffer pointer is null"));
        }
        let buf = if len == 0 {
            &mut [][..]
        } else {
            slice::from_raw_parts_mut(buf, len)
        };
        let read = file.inner.read(buf)?;
        write_out(out_read, read)
    })
}

/// Writes `len` bytes from `data`. `out_written`, when not null, receives the
/// count accepted by this call.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_write(
    file: *mut fsspec_file,
    data: *const u8,
    len: usize,
    out_written: *mut usize,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        if data.is_null() && len != 0 {
            return Err(invalid("data pointer is null"));
        }
        let data = if len == 0 {
            &[][..]
        } else {
            slice::from_raw_parts(data, len)
        };
        let written = file.inner.write(data)?;
        if !out_written.is_null() {
            out_written.write(written);
        }
        Ok(())
    })
}

/// Moves the cursor. `whence` is 0 (start), 1 (current), or 2 (end), as in
/// `fseek`. `out_position`, when not null, receives the new offset.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_seek(
    file: *mut fsspec_file,
    offset: i64,
    whence: i32,
    out_position: *mut u64,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        let target = match whence {
            0 => SeekFrom::Start(
                u64::try_from(offset).map_err(|_| invalid("negative offset from start"))?,
            ),
            1 => SeekFrom::Current(offset),
            2 => SeekFrom::End(offset),
            _ => return Err(invalid("whence must be 0, 1, or 2")),
        };
        let position = file.inner.seek(target)?;
        if !out_position.is_null() {
            out_position.write(position);
        }
        Ok(())
    })
}

/// The current offset.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_tell(
    file: *mut fsspec_file,
    out_position: *mut u64,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        let position = file.inner.stream_position()?;
        write_out(out_position, position)
    })
}

/// Flushes buffered writes.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_flush(file: *mut fsspec_file) -> fsspec_status {
    guard(|| Ok(handle_arg_mut(file, "file")?.inner.flush()?))
}

/// The file size, or -1 when the backend does not know it.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_size(
    file: *mut fsspec_file,
    out_size: *mut i64,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        let size = file
            .inner
            .size()?
            .and_then(|size| i64::try_from(size).ok())
            .unwrap_or(-1);
        write_out(out_size, size)
    })
}

/// Metadata for the open file.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_info(
    file: *mut fsspec_file,
    out: *mut fsspec_file_info,
) -> fsspec_status {
    guard(|| {
        let file = handle_arg_mut(file, "file")?;
        let info = file.inner.info()?;
        write_out(out, fsspec_file_info::from_info(info))
    })
}

/// Commits a transactional write.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_commit(file: *mut fsspec_file) -> fsspec_status {
    guard(|| handle_arg_mut(file, "file")?.inner.commit())
}

/// Discards a transactional write.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_discard(file: *mut fsspec_file) -> fsspec_status {
    guard(|| handle_arg_mut(file, "file")?.inner.discard())
}

/// Flushes and closes the file. The handle is freed even when the flush
/// fails. Null is ignored.
#[no_mangle]
pub unsafe extern "C" fn fsspec_file_close(file: *mut fsspec_file) -> fsspec_status {
    guard(|| {
        if file.is_null() {
            return Ok(());
        }
        let mut file = Box::from_raw(file);
        let flushed = file.inner.flush();
        drop(file);
        Ok(flushed?)
    })
}
