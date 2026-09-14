"""Are the committed protobuf stubs the ones the `.proto` produces?

The stubs are committed rather than generated at install time, for the reasons
`scripts/generate_proto.py` gives. The cost of committing a generated file is
drift, and drift here is the same failure this project treats as the worst
kind: the same request meaning something different, with nothing in a diff to
show it.

So the freshness of the artifact is asserted rather than trusted. This
regenerates into a temporary directory and requires byte equality.

It skips rather than fails when `grpcio-tools` is absent, because a consumer
installing the wheel has no reason to have it — that is the point of committing
the output.
"""

from __future__ import annotations

import pathlib
import tempfile

import pytest

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / "scripts" / "generate_proto.py"


def _generator_versions() -> str:
    """What produced the comparison, for the failure message."""
    import importlib.metadata as metadata

    parts = []
    for package in ("grpcio-tools", "mypy-protobuf", "protobuf"):
        try:
            parts.append(f"{package} {metadata.version(package)}")
        except metadata.PackageNotFoundError:
            parts.append(f"{package} (not installed)")
    return ", ".join(parts)


def test_the_committed_stubs_match_the_proto() -> None:
    pytest.importorskip("grpc_tools", reason="grpcio-tools is a dev dependency")
    pytest.importorskip("mypy_protobuf", reason="mypy-protobuf is a dev dependency")

    import importlib.util

    spec = importlib.util.spec_from_file_location("generate_proto", SCRIPT)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)

    with tempfile.TemporaryDirectory() as tmp:
        fresh = pathlib.Path(tmp) / "out"
        module.generate(fresh)
        committed = module.PACKAGE

        def tree(root: pathlib.Path) -> dict[str, bytes]:
            # `__pycache__` is the interpreter's, not protoc's. Excluded by
            # name rather than by cleaning the tree, because deleting files
            # under the source directory to make a test pass is how a test
            # comes to destroy the thing it is checking.
            return {
                str(p.relative_to(root)): p.read_bytes()
                for p in sorted(root.rglob("*"))
                if p.is_file() and "__pycache__" not in p.parts
            }

        generated, on_disk = tree(fresh), tree(committed)
        assert set(generated) == set(on_disk), (
            "the set of generated files changed; run `python scripts/generate_proto.py`"
        )
        differing = [name for name in generated if generated[name] != on_disk[name]]
        # The generator versions are in the message because they are the most
        # likely cause and the least visible one. This test went red in CI with
        # nothing changed in the repository — `grpcio-tools` was pinned `>=`,
        # `pip` resolved a newer one, and its output differed. The message said
        # "run generate_proto.py", which would have silently rewritten the
        # stubs to whatever that machine happened to have installed.
        assert not differing, (
            f"these committed stubs are stale: {differing}.\n"
            f"Generated here with {_generator_versions()}.\n"
            "If those match the pins in pyproject.toml's `dev` extra, run "
            "`python scripts/generate_proto.py` and commit the result. If they "
            "do not, fix the installed versions first — regenerating with a "
            "different generator is a change to the stubs, not a fix to them."
        )
