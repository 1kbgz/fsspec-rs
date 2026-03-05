"""Tests for fsspec_rs core types exposed via PyO3."""

import pytest


class TestVersion:
    """Test that the package version is accessible."""

    def test_version_exists(self):
        from fsspec_rs import __version__

        assert __version__ == "0.1.0"

    def test_version_is_string(self):
        from fsspec_rs import __version__

        assert isinstance(__version__, str)


class TestFileType:
    """Test the FileType enum exposed from Rust."""

    def test_file_type_file(self):
        from fsspec_rs import FileType

        ft = FileType.file()
        assert ft.as_str() == "file"

    def test_file_type_directory(self):
        from fsspec_rs import FileType

        ft = FileType.directory()
        assert ft.as_str() == "directory"

    def test_file_type_other(self):
        from fsspec_rs import FileType

        ft = FileType.other()
        assert ft.as_str() == "other"

    def test_file_type_str(self):
        from fsspec_rs import FileType

        ft = FileType.file()
        assert str(ft) == "file"

    def test_file_type_repr(self):
        from fsspec_rs import FileType

        ft = FileType.file()
        assert repr(ft) == "FileType.file"

    def test_file_type_eq(self):
        from fsspec_rs import FileType

        assert FileType.file() == FileType.file()
        assert FileType.directory() == FileType.directory()
        assert not (FileType.file() == FileType.directory())

    def test_file_type_directory_repr(self):
        from fsspec_rs import FileType

        ft = FileType.directory()
        assert repr(ft) == "FileType.directory"

    def test_file_type_other_repr(self):
        from fsspec_rs import FileType

        ft = FileType.other()
        assert repr(ft) == "FileType.other"


class TestFileInfo:
    """Test the FileInfo struct exposed from Rust."""

    def test_file_info_creation(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt", size=1024, file_type="file")
        assert info.name == "test.txt"
        assert info.size == 1024
        assert info.file_type == "file"

    def test_file_info_defaults(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt")
        assert info.name == "test.txt"
        assert info.size == 0
        assert info.file_type == "file"

    def test_file_info_directory(self):
        from fsspec_rs import FileInfo

        info = FileInfo("mydir", size=0, file_type="directory")
        assert info.is_dir()
        assert not info.is_file()

    def test_file_info_file(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt", size=100, file_type="file")
        assert info.is_file()
        assert not info.is_dir()

    def test_file_info_str(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt", size=100, file_type="file")
        s = str(info)
        assert "test.txt" in s
        assert "100" in s

    def test_file_info_repr(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt", size=100, file_type="file")
        r = repr(info)
        assert "test.txt" in r
        assert "100" in r
        assert "file" in r

    def test_file_info_eq(self):
        from fsspec_rs import FileInfo

        a = FileInfo("test.txt", size=100, file_type="file")
        b = FileInfo("test.txt", size=100, file_type="file")
        assert a == b

    def test_file_info_neq_name(self):
        from fsspec_rs import FileInfo

        a = FileInfo("a.txt", size=100, file_type="file")
        b = FileInfo("b.txt", size=100, file_type="file")
        assert not (a == b)

    def test_file_info_neq_size(self):
        from fsspec_rs import FileInfo

        a = FileInfo("test.txt", size=100, file_type="file")
        b = FileInfo("test.txt", size=200, file_type="file")
        assert not (a == b)

    def test_file_info_neq_type(self):
        from fsspec_rs import FileInfo

        a = FileInfo("test", size=0, file_type="file")
        b = FileInfo("test", size=0, file_type="directory")
        assert not (a == b)

    def test_file_info_to_dict(self):
        from fsspec_rs import FileInfo

        info = FileInfo("test.txt", size=1024, file_type="file")
        d = info.to_dict()
        assert isinstance(d, dict)
        assert d["name"] == "test.txt"
        assert d["size"] == 1024
        assert d["type"] == "file"

    def test_file_info_to_dict_directory(self):
        from fsspec_rs import FileInfo

        info = FileInfo("mydir", size=0, file_type="directory")
        d = info.to_dict()
        assert d["type"] == "directory"

    def test_file_info_invalid_type(self):
        from fsspec_rs import FileInfo

        with pytest.raises(ValueError, match="unknown file type"):
            FileInfo("test.txt", size=0, file_type="invalid")

    def test_file_info_other_type(self):
        from fsspec_rs import FileInfo

        info = FileInfo("link", size=0, file_type="other")
        assert info.file_type == "other"
        assert not info.is_file()
        assert not info.is_dir()
