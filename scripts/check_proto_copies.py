#!/usr/bin/env python3
"""The TypeScript client's copies of the protos must match the canonical ones.

`crates/slate-server/proto/` holds the schema. **Four things derive from it and
only one of them was unchecked**, which is the whole reason this script is
narrow:

- `clients/python/src/slate/_proto/…` and `clients/go/internal/pb/…` are
  *generated* stubs, committed rather than built at install time, and each has
  its own freshness test that regenerates and compares bytes. Those caught a
  stale stub the same afternoon this script was written — see the ledger entry
  for 2026-09-21 about windows on the wire, where they caught mine.
- `clients/typescript/proto/…` is a tree of *literal copies*, because
  `@grpc/proto-loader` reads the files at run time and an npm package cannot
  reach into a sibling crate. Nothing generated them, so nothing regenerated
  them, so nothing compared them. That is the gap here.

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

**The pairs are walked, not listed.** The first version held one hand-written
pair, `slate/v1/records.proto`, and its own entry said so:

> `COPIES` is still one pair and still has no completeness check.

It was right to worry. Two more files were duplicated across the same two trees
the whole time — `google/rpc/status.proto` and `google/rpc/error_details.proto`,
vendored so `clients/typescript/src/details.ts` can decode the `ErrorInfo` the
head node packs into `grpc-status-details-bin` — and a list of one third of the
copies reads exactly like a list of all of them. This
repository has a standing finding about lists maintained by hand —
`ledger/2026-09-21-two-of-three-lists-were-already-guarded.md` and
`ledger/2026-09-29-the-fifth-table-list-is-derived-now.md` are two of its
instalments — and the fix here is the same one: derive the pairs from the
trees, so a `.proto` added to either side is checked the moment it lands and a
roster cannot fall behind.

That makes the *set* of files a claim too, so both directions are checked: a
canonical file with no copy is a file the loader cannot resolve at run time,
and a copy with no canonical file is a schema nothing on the server side
defines.

Run directly: `python3 scripts/check_proto_copies.py`.
"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent

#: The schema, and the tree that must mirror it file for file.
CANONICAL = "crates/slate-server/proto"
MIRROR = "clients/typescript/proto"


def protos(under: pathlib.Path) -> dict[str, pathlib.Path]:
    """Every `.proto` beneath a tree, keyed by its path within that tree."""
    if not under.is_dir():
        return {}
    return {
        path.relative_to(under).as_posix(): path for path in sorted(under.rglob("*.proto"))
    }


def main() -> int:
    problems: list[str] = []
    source, mirror = protos(ROOT / CANONICAL), protos(ROOT / MIRROR)

    if not source and not mirror:
        # The never-fires case. Both trees moved, or `.proto` stopped being the
        # extension, and a rule that compared nothing must not print the line a
        # rule that compared three prints.
        print(
            f"FAIL  no .proto found under {CANONICAL} or {MIRROR}. Either the "
            "trees moved or the schema is gone — both need a person, not a pass."
        )
        return 1

    for name in sorted(set(source) | set(mirror)):
        if name not in mirror:
            problems.append(
                f"{MIRROR}/{name}: missing. @grpc/proto-loader reads these at "
                f"run time and cannot reach into the crate, so an import it "
                f"cannot resolve is a client that will not start.\n"
                f"      Copy it: cp {CANONICAL}/{name} {MIRROR}/{name}"
            )
        elif name not in source:
            problems.append(
                f"{CANONICAL}/{name}: missing, while {MIRROR}/{name} is there. "
                f"The client would be built against a schema the server does "
                f"not define.\n      Add the canonical file, or delete the copy."
            )
        elif mirror[name].read_bytes() != source[name].read_bytes():
            problems.append(
                f"{MIRROR}/{name} differs from {CANONICAL}/{name}.\n"
                f"      Run: cp {CANONICAL}/{name} {MIRROR}/{name}"
            )
        else:
            print(f"ok    {MIRROR}/{name} matches {CANONICAL}/{name}")

    if problems:
        for problem in problems:
            print(f"FAIL  {problem}")
        print(
            f"\n{len(problems)} proto copy/copies out of date. A client reading a "
            "stale schema builds requests the server does not understand, and "
            "nothing else in the build would say so."
        )
        return 1
    print(
        f"\n{len(source)} proto copy/copies match their source, and the two "
        f"trees hold the same files — walked, not listed, so a fourth is "
        f"checked the moment it lands"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
