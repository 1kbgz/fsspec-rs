/* Proposed native S3 constructor; not exported by libfsspec yet.
   Implemented file, context and operation APIs are in fsspec.h. */
#ifndef FSSPEC_ASYNC_IO_PROPOSAL_H
#define FSSPEC_ASYNC_IO_PROPOSAL_H
#include "fsspec.h"
#ifdef __cplusplus
extern "C" {
#endif
/* Null context selects a new default-budget context. */
fsspec_operation_status fsspec_async_s3_open(
    const char *bucket, size_t bucket_len, const struct fsspec_options *options,
    const fsspec_async_context *context, fsspec_async_fs **out,
    fsspec_async_error **out_error);
#ifdef __cplusplus
}
#endif
#endif
