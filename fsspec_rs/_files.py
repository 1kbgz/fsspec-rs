"""Text handles with the same write lifecycle as native binary handles."""

import io

from fsspec.spec import AbstractFileSystem


class _TextFile(io.TextIOWrapper):
    def discard(self):
        try:
            buffer = self.buffer
        except ValueError:  # detached
            return
        if buffer is not None and not buffer.closed:
            buffer.discard()

    def __exit__(self, exc_type, exc_value, traceback):
        if exc_type is None:
            self.close()
        else:
            self.discard()

    def __del__(self):
        self.discard()


class FileSystem(AbstractFileSystem):
    def open(self, path, mode="rb", block_size=None, cache_options=None, compression=None, **kwargs):
        if "b" not in mode and compression is None:
            text_kwargs = {key: kwargs.pop(key) for key in ("encoding", "errors", "newline") if key in kwargs}
            binary = super().open(path, mode.replace("t", "") + "b", block_size, cache_options, **kwargs)
            return _TextFile(binary, **text_kwargs)
        return super().open(path, mode, block_size, cache_options, compression, **kwargs)
