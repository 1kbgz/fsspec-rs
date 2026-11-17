"""Adapt fsspec's parallel range lists to typed reads grouped by path."""

from operator import index

from fsspec_rs.fsspec_rs import Match, Missing


def _offset(value):
    if value is None:
        return None
    value = index(value)
    if not -(1 << 63) <= value < 1 << 63:
        raise OverflowError("range offsets must fit in a signed 64-bit integer")
    return value


def cat_ranges(fs, paths, starts, ends, max_gap=None, on_error="return", *, condition=None, max_request_size=8388608, max_concurrency=8):
    if not isinstance(paths, list):
        raise TypeError("paths must be a list")
    if not isinstance(starts, list):
        starts = [starts] * len(paths)
    if not isinstance(ends, list):
        ends = [ends] * len(paths)
    if len(starts) != len(paths) or len(ends) != len(paths):
        raise ValueError("paths, starts and ends must have equal lengths")
    if on_error not in ("return", "raise"):
        raise ValueError("on_error must be 'return' or 'raise'")
    if condition is not None and not isinstance(condition, (Match, Missing)):
        raise TypeError("condition must be Match, Missing, or None")
    if isinstance(condition, Missing):
        raise ValueError("the Missing condition is only valid for writes")  # noqa: TRY004 - Valid condition, invalid operation.
    max_gap = 65536 if max_gap is None else index(max_gap)
    max_request_size = index(max_request_size)
    max_concurrency = index(max_concurrency)
    if max_gap < 0 or max_request_size <= 0 or max_concurrency <= 0:
        raise ValueError("max_gap must be nonnegative; max_request_size and max_concurrency must be positive")
    if max(max_gap, max_request_size, max_concurrency) >= 1 << 64:
        raise OverflowError("range options must fit in an unsigned 64-bit integer")
    groups = {}
    for position, (path, start, end) in enumerate(zip(paths, starts, ends)):
        path = fs.unstrip_protocol(fs._strip_protocol(path))
        groups.setdefault(path, []).append((position, (_offset(start), _offset(end))))
    if condition is not None and len(groups) > 1:
        raise ValueError("a condition can only be used with one unique path")
    output = [None] * len(paths)
    for path, entries in groups.items():
        try:
            result = fs.read_ranges(
                path,
                [bounds for _, bounds in entries],
                condition=condition,
                max_gap=max_gap,
                max_request_size=max_request_size,
                max_concurrency=max_concurrency,
            )
        except Exception as error:
            if on_error == "raise":
                raise
            for position, _ in entries:
                output[position] = error
        else:
            for (position, _), data in zip(entries, result.data):
                output[position] = data
    return output
