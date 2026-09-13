#!/usr/bin/env python3
"""Regenerate the protobuf and gRPC stubs from the head node's `.proto`.

The output is committed. That is a decision, not laziness, and the argument is
the same one `crates/slate-server/build.rs` makes for using `protox` instead of
shelling out to `protoc`: a build that depends on an external code generator
fails differently on every machine. `grpcio-tools` pins its own `protoc` and
its own `protobuf` runtime, and generating at install time makes the wheel's
contents depend on which version of it the installing machine resolved. A
committed file also shows up in a diff, so a protocol change is reviewed rather
than absorbed.

The cost is drift, and drift is the failure this project treats as the worst
kind — the same request meaning something different. So it is tested:
`tests/test_generated.py` regenerates into a temporary directory and requires
the result to be byte-identical to what is committed. The generated code is a
build artifact whose freshness is asserted, rather than one that is trusted.

Run: python scripts/generate_proto.py
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
PACKAGE = HERE.parent / "src" / "slate" / "_proto"
PROTO_ROOT = HERE.parents[2] / "crates" / "slate-server" / "proto"
PROTO = "slate/v1/records.proto"

# `grpcio-tools` emits `from slate.v1 import records_pb2 as ...` in the _grpc
# module: an absolute import rooted at the proto package, which only resolves if
# the generated tree is itself on `sys.path`. Putting it on `sys.path` would
# claim the top-level name `slate.v1` for this package, and any other library in
# the process generated from a proto with the same package would collide with
# it. Rewriting the one import to a relative one keeps the tree a private
# subpackage. protoc has no flag for this.
_ABSOLUTE_IMPORT = re.compile(
    r"^from slate\.v1 import (\w+) as (\w+)$",
    re.MULTILINE,
)


def generate(into: pathlib.Path) -> None:
    into.mkdir(parents=True, exist_ok=True)
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "grpc_tools.protoc",
            f"--proto_path={PROTO_ROOT}",
            f"--python_out={into}",
            f"--grpc_python_out={into}",
            # `mypy-protobuf` rather than protoc's own `--pyi_out`: protoc
            # types the messages and nothing else, leaving the generated
            # service stub untyped, and an untyped stub is exactly the boundary
            # where a wrong field name survives type checking. `--mypy_grpc_out`
            # types the stub, so every RPC call in this package is checked
            # against the schema rather than against a comment.
            f"--mypy_out=quiet:{into}",
            f"--mypy_grpc_out=quiet:{into}",
            PROTO,
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"protoc failed:\n{result.stdout}\n{result.stderr}")

    for path in sorted(into.rglob("*_pb2_grpc.py*")):
        text = path.read_text()
        path.write_text(_ABSOLUTE_IMPORT.sub(r"from . import \1 as \2", text))

    # `protoc` does not create `__init__.py`, so the tree is a namespace package
    # and does not ship inside a wheel reliably. Written here rather than
    # committed by hand so that the drift test compares a complete tree.
    for directory in [into, *(p for p in into.rglob("*") if p.is_dir())]:
        (directory / "__init__.py").write_text(
            '"""Generated from `slate/v1/records.proto`. Do not edit.\n\n'
            "Regenerate with `python scripts/generate_proto.py`.\n"
            '"""\n'
        )


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        staged = pathlib.Path(tmp) / "out"
        generate(staged)
        # Replace wholesale rather than merge: a file that protoc has stopped
        # emitting must disappear, and a stale one left behind would keep
        # importing.
        if PACKAGE.exists():
            for path in sorted(PACKAGE.rglob("*"), reverse=True):
                path.unlink() if path.is_file() else path.rmdir()
            PACKAGE.rmdir()
        staged.rename(PACKAGE)
    print(f"generated into {PACKAGE}")


if __name__ == "__main__":
    main()
