#!/usr/bin/env python3
"""Regenerate the Go protobuf and gRPC stubs from the head node's `.proto`.

The same decision `clients/python/scripts/generate_proto.py` documents, for the
same reason: the output is committed so that a protocol change shows up in a
diff rather than being absorbed by whatever code generator the building machine
happened to resolve.

And the same cost, which is drift — except that this copy was **not** tested and
Python's was, so it drifted. The committed stubs were generated before
`Scalar.round` and `Scalar.calendar_part` existed and nothing noticed for two
rounds of work, because no Go code referenced them: a stale generated file is
invisible until someone tries to use the field that is missing. `TestStubsAreFresh`
in `slate/generated_test.go` now regenerates into a temporary directory and
requires the result to be byte-identical to what is committed, which is what
Python has had from the start.

Needs `protoc`, `protoc-gen-go` and `protoc-gen-go-grpc`. The versions are
pinned here rather than taken from `PATH`, because a different generator
version rewrites the whole file and the diff would say nothing about the
protocol. They are installed on demand into `GOPATH/bin`.

Run: python3 clients/go/scripts/generate_proto.py
"""

from __future__ import annotations

import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
GO_CLIENT = HERE.parent
REPO = GO_CLIENT.parent.parent
PROTO_ROOT = REPO / "crates" / "slate-server" / "proto"
PROTO = "slate/v1/records.proto"

#: The Go import path the generated package claims. The `.proto` carries no
#: `go_package` option — deliberately, since the Rust build does not want one —
#: so it is supplied here, which is also the only place it is written down.
GO_PACKAGE = "github.com/howlerops/slate-orm/clients/go/internal/pb/slate/v1"
MODULE = "github.com/howlerops/slate-orm/clients/go"

#: Pinned, for the reason in the module docstring. These are the versions the
#: committed files name in their own headers.
PROTOC_GEN_GO = "google.golang.org/protobuf/cmd/protoc-gen-go@v1.36.12"
PROTOC_GEN_GO_GRPC = "google.golang.org/grpc/cmd/protoc-gen-go-grpc@v1.6.2"

OUT = GO_CLIENT / "internal" / "pb" / "slate" / "v1"
FILES = ["records.pb.go", "records_grpc.pb.go"]


def gopath_bin() -> pathlib.Path:
    out = subprocess.run(
        ["go", "env", "GOPATH"], cwd=GO_CLIENT, capture_output=True, text=True, check=True
    )
    return pathlib.Path(out.stdout.strip()) / "bin"


def ensure_plugins() -> pathlib.Path:
    """Install the two generators if they are not already there."""
    binaries = gopath_bin()
    env = {**os.environ, "GOFLAGS": "-mod=mod"}
    for name, package in (
        ("protoc-gen-go", PROTOC_GEN_GO),
        ("protoc-gen-go-grpc", PROTOC_GEN_GO_GRPC),
    ):
        if (binaries / name).exists():
            continue
        subprocess.run(["go", "install", package], cwd=GO_CLIENT, env=env, check=True)
    return binaries


def generate(into: pathlib.Path) -> None:
    """Write the two stubs under `into`, in their package directory."""
    binaries = ensure_plugins()
    env = {**os.environ, "PATH": f"{os.environ['PATH']}:{binaries}"}
    subprocess.run(
        [
            "protoc",
            "-I",
            str(PROTO_ROOT),
            f"--go_out={into}",
            f"--go_opt=M{PROTO}={GO_PACKAGE}",
            f"--go_opt=module={MODULE}",
            f"--go-grpc_out={into}",
            f"--go-grpc_opt=M{PROTO}={GO_PACKAGE}",
            f"--go-grpc_opt=module={MODULE}",
            PROTO,
        ],
        env=env,
        check=True,
    )


def main() -> int:
    with tempfile.TemporaryDirectory() as work:
        staging = pathlib.Path(work)
        generate(staging)
        produced = staging / "internal" / "pb" / "slate" / "v1"
        OUT.mkdir(parents=True, exist_ok=True)
        for name in FILES:
            shutil.copyfile(produced / name, OUT / name)
            print(f"wrote {(OUT / name).relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
