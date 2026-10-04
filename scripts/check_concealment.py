#!/usr/bin/env python3
"""Every place a row is read out of storage is one the column rules account for.

    python3 scripts/check_concealment.py

# Why this exists

`docs/column-grants.md` puts the concealment of withheld columns in a handful
of places: `QueryCursor::next`, which every read, join side, chain step and
probe leaves through; `SecuredReads::get`; and the rows `delete_where` and
`update_where` hand back. `ledger/2026-10-03-column-grants-built.md` recorded
what that leaves open:

> **No guard requires a new row-returning path to conceal.** [...] A future
> path that hands out rows any other way would be caught only by the oracle,
> and only if it is added to the battery.

A path that hands out rows "any other way" has to get them from storage
somehow, and there are exactly five ways to do that: `read_row_unchecked`,
`scan_rows`, `decode_row`, `decode_row_columns` and `row_from_index_entry`.
Each is a primitive that produces a `Row` with every column in it. So this does
not try to recognise "returns a row to a caller" — a question with no reliable
textual answer — it asks the one that has one: **where are the primitives
called, and has somebody said why each call is safe?**

# What it checks

1. Every call to a primitive, in any workspace crate's `src/`, is inside a
   function named in `ROSTER` for that file, with a reason: either the rows
   reach a caller only through one of the concealing points, or they never
   reach a caller at all (a write path, a migration, a benchmark).
2. Every roster entry still matches a call. A stale entry is a reason nobody
   is relying on, and it would quietly excuse a new call that took the same
   function name.
3. At least one call is found. Zero is what a broken pattern looks like, and
   it would pass every rule above.

# What it does not check

That the reason is true. It cannot read data flow; the roster is a list of
claims a reviewer can check, kept next to the code they are about, which is
the same bargain `check_handlers.py`'s rosters make. And it sees calls by name:
a primitive reached through a function pointer or a re-export under another
name would not be seen. None exists today.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_handlers import enclosing_functions

ROOT = Path(__file__).resolve().parent.parent

PRIMITIVES = ("read_row_unchecked", "scan_rows", "decode_row", "decode_row_columns", "row_from_index_entry")
CALL = re.compile(r"\b(" + "|".join(PRIMITIVES) + r")\(")
DEFINITION = re.compile(r"\bfn\s+(" + "|".join(PRIMITIVES) + r")\b")

#: `(file relative to the root, enclosing function)` -> why its rows are safe.
ROSTER: dict[tuple[str, str], str] = {
    ("crates/slate-kernel/src/exec.rs", "materialise"): (
        "decodes a candidate row for `QueryCursor`, whose `next` conceals before anything leaves"
    ),
    ("crates/slate-kernel/src/exec.rs", "open"): (
        "opens the table-scan source of a `QueryCursor`; rows leave through `next`, which conceals"
    ),
    ("crates/slate-kernel/src/exec.rs", "next_admitted"): (
        "rebuilds a row from a covering index entry inside `QueryCursor`; rows leave through `next`"
    ),
    ("crates/slate-kernel/src/read.rs", "read_row_unchecked"): (
        "the primitive's own body, delegating to `decode_row`"
    ),
    ("crates/slate-kernel/src/read.rs", "read_row_projected"): (
        "the point-read source of a `QueryCursor` (`exec.rs`, `Source::Points`); rows leave through `next`"
    ),
    ("crates/slate-kernel/src/read.rs", "get"): (
        "`SecuredReads::get`, which applies `conceal` to the row before returning it"
    ),
    ("crates/slate-kernel/src/read.rs", "next"): (
        "`RowCursor::next`, the raw scan with no `SecurityContext` at all; its only caller is the "
        "ClickBench binary, as a superuser, and nothing on the wire constructs one. Predates column "
        "grants, and is as unconcealed as it is unauthorised"
    ),
    ("crates/slate-kernel/src/record.rs", "read_row_unchecked"): (
        "the transaction's private wrapper for its own write paths, each rostered by caller below"
    ),
    ("crates/slate-kernel/src/record.rs", "insert"): (
        "checks whether the key is taken; the row is compared, never returned"
    ),
    ("crates/slate-kernel/src/record.rs", "upsert"): (
        "reads the row it is about to replace; `upsert` returns `()`"
    ),
    ("crates/slate-kernel/src/record.rs", "read_rows_concurrently"): (
        "reads the rows a bulk write replaces, for `write_many`, which returns `()`"
    ),
    ("crates/slate-kernel/src/record.rs", "visible_row_with"): (
        "the USING check before a keyed update or delete; the row is tested and written, never returned"
    ),
    ("crates/slate-kernel/src/migrate.rs", "build"): (
        "a migration backfilling an index from every row; runs as the operator, returns nothing"
    ),
    ("crates/slate-schema/src/row.rs", "decode_row"): (
        "the primitive's own body, delegating to `decode_row_columns`"
    ),
    ("crates/slate-clickbench/src/main.rs", "main"): (
        "the ClickBench loader and timer, a benchmark binary with no caller identity"
    ),
}


def calls(root: Path = ROOT) -> list[tuple[str, int, str, str]]:
    """Every call to a primitive: `(file, line, enclosing function, primitive)`."""
    found = []
    for path in sorted((root / "crates").glob("*/src/**/*.rs")):
        lines = path.read_text().splitlines()
        owners = enclosing_functions(lines)
        relative = path.relative_to(root).as_posix()
        for at, line in enumerate(lines):
            code = line.split("//", 1)[0]
            match = CALL.search(code)
            if match and not DEFINITION.search(code):
                found.append((relative, at + 1, owners[at], match.group(1)))
    return found


def problems(root: Path = ROOT, roster: dict[tuple[str, str], str] | None = None) -> list[str]:
    roster = ROSTER if roster is None else roster
    found = calls(root)
    out = []
    if not found:
        out.append(
            f"no call to any of {', '.join(PRIMITIVES)} was found under crates/*/src, so every rule "
            "here checked nothing. The pattern is broken or the primitives were renamed."
        )
    used = set()
    for file, line, owner, primitive in found:
        if (file, owner) in roster:
            used.add((file, owner))
            continue
        out.append(
            f"{file}:{line}: `{owner}` calls `{primitive}`, which reads a row with every column in it, "
            "and is not in ROSTER.\n"
            "  A row read this way skips the concealment of withheld columns unless it reaches the "
            "caller through `QueryCursor::next`, `SecuredReads::get`, or `conceal`. Route it through "
            "one of those, or add it to ROSTER saying why it never reaches a caller "
            "(docs/column-grants.md)."
        )
    for key in sorted(set(roster) - used):
        out.append(
            f"ROSTER names `{key[1]}` in {key[0]}, which no longer calls a primitive; remove the entry, "
            "or a new call in a function of the same name will be excused by a reason written for "
            "something else."
        )
    return out


def main() -> int:
    found = problems()
    if found:
        for problem in found:
            print(f"FAIL  {problem}", file=sys.stderr)
        print(f"\n{len(found)} problem(s)", file=sys.stderr)
        return 1
    print(
        f"ok    {len(calls())} row reads from storage, in {len(ROSTER)} functions, each with a reason "
        "the column rules hold"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
