#!/usr/bin/env python3
"""Every file `docs/` points at must be a file, here or named as elsewhere's.

`scripts/check_cited_tests.py` checks the *names* a document points at:
`docs/security-review.md` spent a week naming four probes that no longer
existed, each inverted by the commit that fixed the finding it demonstrated,
and nothing compiles a document. This is the same failure one level up. A
document that says "see `crates/slate-kernel/tests/oracle.rs`" after that file
is renamed is as useless as one naming a dead function, and until this guard
nothing in the repository looked at a cited *path* in `docs/` at all —
`check_cited_docs.py` runs the other way, from source to `docs/`, and
`check_cited_tests.py` matches snake_case identifiers, which a path is not.

Found by mutation, and the null result is worth as much as the guard: of 109
distinct paths cited across `docs/`, 108 resolve and the one that does not is
another project's. So this catches nothing today and is here for the same
reason `check_cited_tests.py` is — the class has a week-long instance on
record, in this repository, in a security review.

**A citation may be written relative to a crate.** `docs/performance.md` says
`examples/replicas.rs`, meaning `crates/slate-slatedb/examples/replicas.rs`,
and `docs/correctness.md` says `tests/rls_join.rs` for
`crates/slate-server/tests/rls_join.rs`. Twenty of the hundred and nine are
written that way and all of them read correctly in context, so a path resolves
when it is a **suffix** of a tracked file's path. That is weaker than an exact
match and it is the honest rule: what this catches is a file that is not there
at all, which is the failure that happened.

**And a citation may belong to another repository.** `docs/clickbench.md`
reviews `pgrust` and quotes the file describing that project's benchmark
methodology. `EXTERNAL` names those, one line each saying whose they are, and
an entry that stops being cited — or that starts resolving here — is itself a
failure, because a roster nobody is forced to edit is a roster that goes stale.

Run directly: `python3 scripts/check_cited_files.py`.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Where a path is read as a live claim. `docs/` only, for the reasons
#: `check_cited_tests.py`'s docstring sets out at length: `ledger/` cites dead
#: paths on purpose, because an entry is a dated record and its provenance is
#: the thing a ledger is for.
SCOPE = "docs"

#: A backticked path: at least one slash, and an extension this repository
#: writes. The extension list is what keeps `google/rpc` and `tokio/net` out —
#: a module path is not a file path, and flagging one would be the 87% false
#: positive rate `check_cited_tests.py` measured and rejected.
CITED = re.compile(
    r"`([A-Za-z0-9_.\-]+(?:/[A-Za-z0-9_.\-]+)+"
    r"\.(?:rs|py|ts|tsx|go|toml|json|sh|md|proto|yml|yaml|css|html))`"
)

#: Paths that belong to another project, with whose they are. Each must still
#: be cited somewhere in scope, and must still not resolve here; both are
#: checked below, so this list cannot quietly outlive its reason.
EXTERNAL: dict[str, str] = {
    "benchmarks/README.md": (
        "pgrust's. docs/clickbench.md reviews that project and quotes the file "
        "describing its own benchmark methodology, which is the thing being "
        "assessed — there is no copy here and there should not be one."
    ),
}


def tracked() -> set[str]:
    """Every path in the index, and every suffix of one. See the docstring."""
    files = subprocess.run(
        ["git", "ls-files"], cwd=ROOT, capture_output=True, text=True, check=True
    ).stdout.split()
    suffixes: set[str] = set()
    for path in files:
        parts = path.split("/")
        suffixes.update("/".join(parts[at:]) for at in range(len(parts)))
    return suffixes


def cited() -> dict[str, list[str]]:
    """Each path cited under `SCOPE`, and the pages citing it."""
    where: dict[str, list[str]] = {}
    for page in sorted((ROOT / SCOPE).rglob("*.md")):
        for found in CITED.finditer(page.read_text()):
            where.setdefault(found.group(1), []).append(page.relative_to(ROOT).as_posix())
    return where


def main() -> int:
    problems: list[str] = []
    here, seen = tracked(), cited()

    if not seen:
        # The never-fires case. `docs/` moved, or the pattern stopped matching,
        # and a rule that checked nothing must not print the line a rule that
        # checked a hundred and nine prints.
        print(
            f"no file paths cited under {SCOPE}/. Either the tree moved or "
            "CITED stopped matching — both need a person, not a pass.",
            file=sys.stderr,
        )
        return 1

    for path, pages in sorted(seen.items()):
        if path in EXTERNAL:
            continue
        if path not in here:
            problems.append(
                f"{', '.join(sorted(set(pages)))} cites `{path}`, which is not "
                "a file here, nor the tail of one.\n"
                "  A document naming a file that is not there points a reader "
                "at nothing, and nothing compiles a document. Fix the path, or "
                "add it to EXTERNAL saying whose file it is."
            )

    for path, whose in sorted(EXTERNAL.items()):
        if path not in seen:
            problems.append(
                f"EXTERNAL names `{path}` and nothing under {SCOPE}/ cites it "
                "any more.\n  Drop the entry. A roster nobody is forced to "
                "edit is a roster that goes stale, which is this guard's own "
                f"subject. It said: {whose}"
            )
        elif path in here:
            problems.append(
                f"EXTERNAL names `{path}` as another project's and it now "
                "resolves here.\n  Either a file arrived at that path or the "
                "citation changed meaning. Drop the entry so the path is "
                f"checked like any other. It said: {whose}"
            )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    print(
        f"ok    {len(seen)} file paths cited across {SCOPE}/, all resolving; "
        f"{len(EXTERNAL)} named as another project's"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
