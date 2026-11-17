//! Conversion of typed object operations across the C boundary.

use std::mem::size_of;
use std::ptr;

use fsspec_rs::{
    Condition, FsResult, IoCapabilities, OperationError, ReadOptions, ReadResult, Validator,
    WriteOptions, WriteResult,
};

use crate::types::*;
use crate::util::{invalid, str_arg};

pub fn operation_status(error: &OperationError) -> fsspec_operation_status {
    match error {
        OperationError::Conflict(_) => FSSPEC_CONFLICT,
        OperationError::FileSystem(error) => crate::status_code(error) as i32,
        _ => FSSPEC_OTHER as i32,
    }
}

pub fn operation_error(status: fsspec_operation_status, message: String) -> OperationError {
    if status == FSSPEC_CONFLICT {
        return OperationError::Conflict(message);
    }
    let status = match status {
        1 => FSSPEC_NOT_FOUND,
        2 => FSSPEC_PERMISSION_DENIED,
        3 => FSSPEC_ALREADY_EXISTS,
        4 => FSSPEC_NOT_A_DIRECTORY,
        5 => FSSPEC_IS_A_DIRECTORY,
        6 => FSSPEC_IO,
        7 => FSSPEC_INVALID_ARGUMENT,
        8 => FSSPEC_NOT_SUPPORTED,
        _ => FSSPEC_OTHER,
    };
    OperationError::FileSystem(crate::error_from_status(status, message))
}

impl fsspec_validator {
    pub fn from_validator(value: Option<Validator>) -> Self {
        match value {
            Some(value) => Self {
                scope: fsspec_string::from_string(value.scope),
                value: fsspec_string::from_string(value.value),
            },
            None => Self {
                scope: fsspec_string::null(),
                value: fsspec_string::null(),
            },
        }
    }

    pub unsafe fn to_validator(&self) -> FsResult<Option<Validator>> {
        if self.scope.data.is_null()
            && self.scope.len == 0
            && self.value.data.is_null()
            && self.value.len == 0
        {
            return Ok(None);
        }
        if self.scope.data.is_null() || self.value.data.is_null() {
            return Err(invalid("validator requires both scope and value"));
        }
        Ok(Some(Validator {
            scope: str_arg(self.scope.data, self.scope.len)?.into(),
            value: str_arg(self.value.data, self.value.len)?.into(),
        }))
    }

    pub unsafe fn free(self) {
        self.scope.free();
        self.value.free();
    }
}

unsafe fn condition_arg(condition: *const fsspec_condition) -> FsResult<Option<Condition>> {
    let Some(condition) = condition.as_ref() else {
        return Ok(None);
    };
    match condition.kind {
        FSSPEC_CONDITION_MATCH => {
            let validator = condition
                .validator
                .as_ref()
                .ok_or_else(|| invalid("Match requires a validator"))?;
            Ok(Some(Condition::Match(
                validator
                    .to_validator()?
                    .ok_or_else(|| invalid("Match requires a validator"))?,
            )))
        }
        FSSPEC_CONDITION_MISSING if condition.validator.is_null() => Ok(Some(Condition::Missing)),
        _ => Err(invalid("invalid condition kind or validator")),
    }
}

pub unsafe fn read_options(options: *const fsspec_read_options) -> FsResult<ReadOptions> {
    if options.is_null() {
        return Ok(ReadOptions::default());
    }
    if ptr::read(options.cast::<usize>()) < size_of::<fsspec_read_options>() {
        return Err(invalid("read options struct_size is too small"));
    }
    let options = &*options;
    Ok(ReadOptions {
        start: options.start.as_ref().copied(),
        end: options.end.as_ref().copied(),
        condition: condition_arg(options.condition)?,
    })
}

pub unsafe fn write_options(options: *const fsspec_write_options) -> FsResult<WriteOptions> {
    if options.is_null() {
        return Ok(WriteOptions::default());
    }
    if ptr::read(options.cast::<usize>()) < size_of::<fsspec_write_options>() {
        return Err(invalid("write options struct_size is too small"));
    }
    Ok(WriteOptions {
        condition: condition_arg((*options).condition)?,
    })
}

// Keep borrowed C strings and pointers alive until the synchronous callback returns.
fn with_condition<T>(
    condition: &Option<Condition>,
    call: impl FnOnce(*const fsspec_condition) -> T,
) -> T {
    match condition {
        None => call(ptr::null()),
        Some(Condition::Missing) => call(&fsspec_condition {
            kind: FSSPEC_CONDITION_MISSING,
            validator: ptr::null(),
        }),
        Some(Condition::Match(value)) => {
            let validator = fsspec_validator {
                scope: fsspec_string {
                    data: value.scope.as_ptr().cast_mut().cast(),
                    len: value.scope.len(),
                },
                value: fsspec_string {
                    data: value.value.as_ptr().cast_mut().cast(),
                    len: value.value.len(),
                },
            };
            call(&fsspec_condition {
                kind: FSSPEC_CONDITION_MATCH,
                validator: &validator,
            })
        }
        _ => call(&fsspec_condition {
            kind: u32::MAX,
            validator: ptr::null(),
        }),
    }
}

pub fn with_read_options<T>(
    options: &ReadOptions,
    call: impl FnOnce(*const fsspec_read_options) -> T,
) -> T {
    with_condition(&options.condition, |condition| {
        call(&fsspec_read_options {
            struct_size: size_of::<fsspec_read_options>(),
            start: options.start.as_ref().map_or(ptr::null(), |v| v),
            end: options.end.as_ref().map_or(ptr::null(), |v| v),
            condition,
        })
    })
}

pub fn with_write_options<T>(
    options: &WriteOptions,
    call: impl FnOnce(*const fsspec_write_options) -> T,
) -> T {
    with_condition(&options.condition, |condition| {
        call(&fsspec_write_options {
            struct_size: size_of::<fsspec_write_options>(),
            condition,
        })
    })
}

impl fsspec_read_result {
    pub fn from_result(value: ReadResult) -> Self {
        Self {
            data: fsspec_bytes::from_vec(value.data),
            validator: fsspec_validator::from_validator(value.validator),
        }
    }

    pub unsafe fn into_result(self) -> FsResult<ReadResult> {
        let validator = self.validator.to_validator();
        let data = self.data.as_slice().to_vec();
        self.free();
        Ok(ReadResult {
            data,
            validator: validator?,
        })
    }

    pub unsafe fn free(self) {
        self.data.free();
        self.validator.free();
    }
}

impl fsspec_write_result {
    pub fn from_result(value: WriteResult) -> Self {
        Self {
            validator: fsspec_validator::from_validator(value.validator),
        }
    }
    pub unsafe fn into_result(self) -> FsResult<WriteResult> {
        let validator = self.validator.to_validator();
        self.free();
        Ok(WriteResult {
            validator: validator?,
        })
    }
    pub unsafe fn free(self) {
        self.validator.free();
    }
}

impl From<IoCapabilities> for fsspec_io_capabilities {
    fn from(value: IoCapabilities) -> Self {
        Self {
            read_match: value.read_match,
            write_match: value.write_match,
            write_missing: value.write_missing,
        }
    }
}

impl From<fsspec_io_capabilities> for IoCapabilities {
    fn from(value: fsspec_io_capabilities) -> Self {
        Self {
            read_match: value.read_match,
            write_match: value.write_match,
            write_missing: value.write_missing,
        }
    }
}
