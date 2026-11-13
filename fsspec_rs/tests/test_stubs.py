"""The type stub for the compiled module lists the same names as the module itself."""

import ast
from pathlib import Path

import fsspec_rs.fsspec_rs as compiled

STUB = Path(compiled.__file__).with_name("fsspec_rs.pyi")
# Dunder methods the stub declares for typing only.
TYPING_ONLY = {"__init__", "__eq__", "__enter__", "__exit__"}


def _public(names):
    return {name for name in names if not name.startswith("_")}


def _stub():
    return ast.parse(STUB.read_text())


def test_stub_lists_the_module_names():
    names = set()
    for node in _stub().body:
        if isinstance(node, (ast.ClassDef, ast.FunctionDef)):
            names.add(node.name)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.add(node.target.id)
    assert names == _public(dir(compiled))


def test_stub_lists_each_class_member():
    for node in _stub().body:
        if isinstance(node, ast.ClassDef):
            members = {item.name for item in node.body if isinstance(item, ast.FunctionDef)} - TYPING_ONLY
            assert members == _public(dir(getattr(compiled, node.name))), node.name
