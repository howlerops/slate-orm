#!/usr/bin/env python3
"""The TypeScript client's copy of the proto must match the canonical one.

`crates/slate-server/proto/slate/v1/records.proto` is the schema. **Four things
derive from it and only one of them was unchecked**, which is the whole reason
this script is narrow:

- `clients/python/src/slate/_proto/…` and `clients/go/internal/pb/…` are
  *generated* stubs, committed rather than built at install time, and each has
  its own freshness test that regenerates and compares bytes. Those caught a
  stale stub the same afternoon this script was written — see the ledger entry
  for 2026-09-21 about windows on the wire, where they caught mine.
- `clients/typescript/proto/…` is a *literal copy*, because `@grpc/proto-loader`
  reads the file at run time and an npm package cannot reach into a sibling
  crate. Nothing generated it, so nothing regenerated it, so nothing compared
  it. That is the gap here.

It is the shape of defect this repository has closed several times under other
names (the decoder rosters, the generated declarations, the `EXPECTED_REFUSALS`
lists): a second statement of one fact, with no check that the two agree.

What a drift costs is worse than a stale comment: the TypeScript client would
build a request against a schema the server does not have. A field added only to
the canonical copy is invisible to it; a field number reused only in the copy is
a request the server misparses rather than rejects. Neither shows up as a
TypeScript error, because the loader reads the file it is given.

Byte-for-byte rather than semantically. A parser that compared only the
declarations would let a comment drift, and the comments in that file carry most
of the reasoning about what each field means — a client author reading a stale
one is the failure this is about, one level up.
"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: Canonical path -> every copy that must match it.
COPIES: dict[str, list[str]] = {
    "crates/slate-server/proto/slate/v1/records.proto": [
        "clients/typescript/proto/slate/v1/records.proto",
    ],
}


def main() -> int:
    problems: list[str] = []
    checked = 0
    for source, copies in COPIES.items():
        original = ROOT / source
        if not original.is_file():
            problems.append(f"{source}: the canonical file is missing")
            continue
        want = original.read_bytes()
        for relative in copies:
            copy = ROOT / relative
            checked += 1
            if not copy.is_file():
                problems.append(f"{relative}: missing; copy it from {source}")
            elif copy.read_bytes() != want:
                problems.append(
                    f"{relative} differs from {source}; "
                    f"run `cp {source} {relative}`"
                )
            else:
                print(f"ok    {relative} matches {source}")

    if problems:
        for problem in problems:
            print(f"FAIL  {problem}")
        print(
            f"\n{len(problems)} proto copy/copies out of date. A client reading a "
            "stale schema builds requests the server does not understand, and "
            "nothing else in the build would say so."
        )
        return 1
    print(f"\n{checked} proto copy/copies match their source")
    return 0


if __name__ == "__main__":
    sys.exit(main())
