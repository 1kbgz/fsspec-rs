from importlib.resources import files
from json import loads


def test_cargo_license_bundle_contains_dependency_texts():
    resource = files("fsspec_rs").joinpath("licenses/cargo.json")
    assert resource.is_file(), "Cargo dependency license bundle is missing from the package"
    bundle = loads(resource.read_text(encoding="utf-8"))
    assert bundle["root_name"] == "fsspec_rs_py"
    libraries = bundle["third_party_libraries"]
    assert {"fsspec_rs", "pyo3", "object_store", "tokio"} <= {library["package_name"] for library in libraries}
    assert {library["package_version"] for library in libraries if library["package_name"] == "r-efi"} == {"5.3.0", "6.0.0"}
    for library in libraries:
        assert library["licenses"], library["package_name"]
        for license in library["licenses"]:
            assert license["text"].strip(), library["package_name"]
            assert license["text"].strip() != "NOT FOUND", library["package_name"]
    for library in libraries:
        if library["package_name"] == "r-efi":
            assert {license["license"] for license in library["licenses"]} == {"MIT", "Apache-2.0", "LGPL-2.1-or-later"}
            for license in library["licenses"]:
                assert "Copyright (C)" in license["text"]
                if license["license"] == "MIT":
                    assert "Permission is hereby granted, free of charge" in license["text"]
                elif license["license"] == "Apache-2.0":
                    assert "TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION" in license["text"]
                else:
                    assert "GNU LESSER GENERAL PUBLIC LICENSE" in license["text"]
