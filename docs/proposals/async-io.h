/* Future file/context API: these declarations are not exported by libfsspec.
   Implemented operation/dispatcher declarations come from fsspec.h. */
#ifndef FSSPEC_ASYNC_IO_PROPOSAL_H
#define FSSPEC_ASYNC_IO_PROPOSAL_H

#include "fsspec.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef struct fsspec_async_context fsspec_async_context;
typedef struct fsspec_async_fs fsspec_async_fs;
typedef struct fsspec_async_file fsspec_async_file;

/* Defaults: 64 admitted operations, 64 MiB accounted buffers, 8 MiB per read. */
typedef struct fsspec_async_context_options {
    size_t struct_size;
    size_t max_operations;
    uint64_t max_buffered_bytes;
    size_t max_read_size;
} fsspec_async_context_options;

/* Uses libfsspec's shared managed runtime for native tasks and supervision. */
fsspec_operation_status fsspec_async_context_create(
    const fsspec_async_context_options *options,
    fsspec_async_context **out, fsspec_async_error **out_error);
fsspec_async_context *fsspec_async_context_retain(const fsspec_async_context *context);
void fsspec_async_context_release(fsspec_async_context *context);

/* Relative monotonic timeout, including queueing. UINT64_MAX means no deadline. */
typedef struct fsspec_async_options {
    size_t struct_size;
    uint64_t timeout_ns;
} fsspec_async_options;

/* Proposed additional completion payloads; consume one producer reference. */
void fsspec_completion_info(fsspec_completion *completion,
                            const struct fsspec_file_info *info);
void fsspec_completion_file(fsspec_completion *completion, fsspec_async_file *file);
void fsspec_completion_position(fsspec_completion *completion, uint64_t position);

/* Backend callbacks are nonblocking starts, invoked through its dispatcher.
   They receive one owned completion reference; inputs are borrowed during start. */
typedef struct fsspec_async_fs_vtable {
    size_t struct_size;
    uint32_t abi_version;
    void (*info)(void *data, const char *path, size_t path_len,
                 fsspec_completion *completion);
    void (*open_read)(void *data, const char *path, size_t path_len,
                      fsspec_completion *completion);
    /* Optional async cleanup of provider-owned resources, after its files drain. */
    void (*close)(void *data, fsspec_completion *completion);
    void (*release)(void *data);
} fsspec_async_fs_vtable;

typedef struct fsspec_async_file_vtable {
    size_t struct_size;
    uint32_t abi_version;
    void (*read)(void *data, size_t max_bytes, fsspec_completion *completion);
    void (*seek)(void *data, int64_t offset, uint32_t whence,
                 fsspec_completion *completion);
    void (*info)(void *data, fsspec_completion *completion);
    void (*close)(void *data, fsspec_completion *completion);
    void (*release)(void *data);
} fsspec_async_file_vtable;

fsspec_operation_status fsspec_async_fs_from_vtable(
    const fsspec_async_context *context, const fsspec_dispatcher *dispatcher,
    const fsspec_async_fs_vtable *vtable, void *data,
    fsspec_async_fs **out, fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_file_from_vtable(
    const fsspec_async_context *context, const fsspec_dispatcher *dispatcher,
    const fsspec_async_file_vtable *vtable, void *data,
    fsspec_async_file **out, fsspec_async_error **out_error);
fsspec_async_fs *fsspec_async_fs_retain(const fsspec_async_fs *fs);
void fsspec_async_fs_release(fsspec_async_fs *fs);
fsspec_async_file *fsspec_async_file_retain(const fsspec_async_file *file);
void fsspec_async_file_release(fsspec_async_file *file);

/* Native S3 construction: null context selects a new default-budget context. */
fsspec_operation_status fsspec_async_s3_open(
    const char *bucket, size_t bucket_len, const struct fsspec_options *options,
    const fsspec_async_context *context, fsspec_async_fs **out,
    fsspec_async_error **out_error);

/* Starts copy arguments before returning. No caller output buffer is retained. */
fsspec_operation_status fsspec_async_fs_info(
    const fsspec_async_fs *fs, const char *path, size_t path_len,
    const fsspec_async_options *options, fsspec_operation **out,
    fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_fs_open_read(
    const fsspec_async_fs *fs, const char *path, size_t path_len,
    const fsspec_async_options *options, fsspec_operation **out,
    fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_fs_close(
    fsspec_async_fs *fs, const fsspec_async_options *options,
    fsspec_operation **out, fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_file_read(
    fsspec_async_file *file, size_t max_bytes, const fsspec_async_options *options,
    fsspec_operation **out, fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_file_seek(
    fsspec_async_file *file, int64_t offset, uint32_t whence,
    const fsspec_async_options *options, fsspec_operation **out,
    fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_file_info(
    const fsspec_async_file *file, const fsspec_async_options *options,
    fsspec_operation **out, fsspec_async_error **out_error);
fsspec_operation_status fsspec_async_file_close(
    fsspec_async_file *file, const fsspec_async_options *options,
    fsspec_operation **out, fsspec_async_error **out_error);
/* Reject new work, cancel pending work, and finish only after provider cleanup. */
fsspec_operation_status fsspec_async_context_close(
    fsspec_async_context *context, const fsspec_async_options *options,
    fsspec_operation **out, fsspec_async_error **out_error);

#ifdef __cplusplus
}
#endif
#endif
