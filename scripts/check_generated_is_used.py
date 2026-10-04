#!/usr/bin/env python3
"""Every generated declaration is imported, and reaches the wire.

`scripts/codegen.py --check` proves a generated file matches the catalog it was
written from. It cannot prove anybody reads it, which
`ledger/2026-09-18-the-catalog-writes-the-declaration-nobody-should-type.md`
left open:

    `--check` proves the file matches the catalog, not that anybody imports
    it. What proves that is G1-G3, which are mutations rather than tests: no
    check in CI would notice if an adapter stopped calling `declaring`.

A generated file nothing imports is a file that stays perfectly in step with a
catalog nobody consults — green forever, and the schema check it exists to
carry is simply absent from every request. That is the quiet failure the
fingerprint was built to prevent, one level up.

**The roster is read out of `ci.yml`.** Every path `codegen.py --check` is
given is a file this rule covers, so a sixth generated file comes under it by
being generated, not by being remembered. Two rules per file:

1. **Something imports it** — an import-shaped mention, inside the file's own
   package. Both qualifiers were bought by a failing test: the first draft
   matched the bare module stem anywhere in the tree, and `\\bschema\\b`
   occurs in **483** of this repository's files as of writing. Rule 1 was
   green because the word "schema" is everywhere, which is the guard
   committing the very offence it exists to detect. Measured on the Go
   adapter: 483 files match the bare stem, 7 once the search is scoped to the
   package, and 3 once it must also be import-shaped — and those 3 are the
   real importers. The counts move as files are added; the ratio is the point.
2. **It reaches the wire**, by the call named in `REACHES`. Importing a
   generated module for one constant while declaring nothing is the shape the
   caveat is actually about, and rule 1 alone would pass it.

Rule 2 is a roster, because how a declaration reaches the wire differs by
client and no rule infers it: Go and TypeScript attach one map once with
`Declaring`, and the Python client takes a `Table` per call, so there is no
single call to look for.

Run directly: `python3 scripts/check_generated_is_used.py`.
"""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"

#: The flags `codegen.py` writes a file for. `--config` and `--serverd` are
#: inputs and `--go-package` is a name, so only these four name outputs.
OUTPUT_FLAGS = ("--python", "--go", "--typescript", "--web")

#: Per generated file, the thing that puts its declarations on a request, and
#: where to find it. `None` means rule 2 does not apply and says why.
REACHES: dict[str, tuple[str, str] | str] = {
    "examples/explorer/backends/go/schema/schema.go": (
        r"Declaring\(declared\(\)\)",
        "examples/explorer/backends/go/main.go",
    ),
    "examples/explorer/backends/node/src/schema.ts": (
        r"\.declaring\(",
        # `adapter.ts` since the Node adapter was split so its endpoints could
        # run as a Cloudflare Worker too; `main.ts` is the Node shell now.
        "examples/explorer/backends/node/src/adapter.ts",
    ),
    # No `declaring` to look for: this client takes a `Table` per call, so the
    # declaration rides on each request rather than being attached once. The
    # import *is* the attachment, and a query naming one of these tables is
    # what carries the fingerprint.
    "examples/explorer/backends/python/adapter/schema.py": (
        r"^from \.schema import",
        "examples/explorer/backends/python/adapter/__main__.py",
    ),
    # Column headers in a browser, which never reach the server at all --
    # `api.ts` says so at length. Rule 1 still applies, and there is nothing
    # for rule 2 to check.
    "examples/explorer/web/src/catalog.ts": (
        "the browser holds these as labels; nothing hashes them and none reaches a request"
    ),
    # The retention example's seeder, which builds its own client and names
    # the generated table. `purge.py` also reaches it, by importing the module
    # its `--schema` argument names at run time — which is the more
    # interesting path and the one a static rule cannot follow, so the static
    # importer is what this checks.
    "examples/retention/schema.py": (
        r"^from schema import",
        "examples/retention/seed.py",
    ),
}


def generated() -> list[str]:
    """Every output path `codegen.py --check` is given in the workflow."""
    text = WORKFLOW.read_text()
    found: list[str] = []
    for flag in OUTPUT_FLAGS:
        # The invocations are line-continued shell, one flag per line.
        found.extend(re.findall(rf"^\s*{re.escape(flag)}\s+(\S+)\s*\\?\s*$", text, re.M))
    return sorted(set(found))


#: Files that mark the root of a package. An importer of a generated module
#: lives inside the same one: Go resolves `…/go/schema` against `go.mod`, node
#: resolves `./schema.js` against the importer's own directory. Nothing across
#: a package boundary can be importing it, so nothing across one counts.
MANIFESTS = ("go.mod", "package.json", "pyproject.toml", "Cargo.toml")


def package_of(path: str) -> str:
    """The subtree an importer of `path` could live in.

    The nearest ancestor holding a manifest, falling back to the file's own
    directory. The fallback is the narrow answer on purpose: `examples/retention`
    has no manifest, and widening to the repository root would put the rule
    back where it started. A wrong-but-narrow scope fails loudly — the importer
    is not found and somebody reads the message — where a wrong-but-wide one
    passes forever.
    """
    here = (ROOT / path).parent
    while here != ROOT:
        if any((here / manifest).exists() for manifest in MANIFESTS):
            return here.relative_to(ROOT).as_posix()
        here = here.parent
    return Path(path).parent.as_posix()


def importers(path: str) -> list[str]:
    """Files inside this module's package that import it, other than itself.

    By module stem rather than by path: an importer writes `from .schema
    import …` or `"./schema.js"`, never the path from the repository root. But
    the stem alone is a common English word — see the note on rule 1 above —
    so it has to appear in an import shape: directly after a
    `from`/`import`/`require` with nothing but module-path punctuation
    between, or inside a quoted path with a `/` or `.` in front of it. The
    second arm is what catches Go, whose import lines carry no keyword of
    their own. The first arm allows *nothing* in between for a reason: a draft
    that allowed any non-quote text matched the sentence "these labels come
    from the generated catalog", and a comment mentioning a module is exactly
    what rule 1 must not accept as reading it.

    `git grep` so build output and `node_modules` are out of scope for free.
    """
    stem = Path(path).stem
    # POSIX ERE, for `git grep -E`: `[[:space:]]` rather than `\s`, and an
    # explicit non-word-or-end rather than `\b`. `\b` is a GNU extension: glibc
    # honours it, macOS's regex library reads it as something no line contains,
    # so on a Mac this found no importer for any module and refused every one —
    # `scripts/check.sh` red on a clean tree, green in CI. `-P` would restore
    # `\b` but needs a git built with PCRE, which is the same bet one layer down.
    shaped = (
        rf"(from|import|require)[[:space:]]+[.\"'/]*{stem}([^A-Za-z0-9_]|$)"
        rf"|[\"'][^\"']*[./]{stem}(\.[A-Za-z]+)?[\"']"
    )
    result = subprocess.run(
        ["git", "grep", "-l", "-E", shaped, "--", package_of(path)],
        cwd=ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    return [line for line in result.stdout.splitlines() if line and line != path]


def contains(relative: str, pattern: str) -> bool:
    path = ROOT / relative
    return path.exists() and re.search(pattern, path.read_text(), re.M) is not None


def main() -> int:
    problems: list[str] = []
    files = generated()

    if not files:
        # The never-fires case, and the likeliest: the workflow is reformatted,
        # the regex stops matching, and a rule over an empty list passes.
        print(
            f"no generated files found in {WORKFLOW.relative_to(ROOT)}. Either "
            "`codegen.py --check` is gone or its invocation is spelled "
            "differently now — both need a person, not a pass.",
            file=sys.stderr,
        )
        return 1

    for path in files:
        if not (ROOT / path).exists():
            problems.append(
                f"{path} is generated by ci.yml and is not in the tree. Either "
                "it is not committed, or the workflow names a path that moved."
            )
            continue
        if not importers(path):
            problems.append(
                f"nothing in {package_of(path)} imports {path}.\n"
                "  A generated declaration nobody reads stays perfectly in "
                "step with a catalog nobody consults, and `--check` is green "
                "the whole time. Import it, or stop generating it."
            )
        reach = REACHES.get(path)
        if reach is None:
            problems.append(
                f"{path} is generated and REACHES has no entry for it.\n"
                "  Say what puts its declarations on a request, or say why "
                "nothing does — the browser catalog's entry is the shape for "
                "the second case."
            )
        elif isinstance(reach, tuple):
            pattern, where = reach
            if not contains(where, pattern):
                problems.append(
                    f"{path} is imported but {where} no longer matches "
                    f"`{pattern}`.\n"
                    "  That is the caveat exactly: an adapter that stopped "
                    "declaring would keep importing, keep passing `--check`, "
                    "and send every request with no schema claim on it."
                )

    for path in sorted(set(REACHES) - set(files)):
        problems.append(
            f"REACHES names {path}, which ci.yml no longer generates. Drop the "
            "entry, or this rule is describing a file that is gone."
        )

    if problems:
        for problem in problems:
            print(problem, file=sys.stderr)
        print(f"\n{len(problems)} problem(s)", file=sys.stderr)
        return 1

    reaching = sum(1 for path in files if isinstance(REACHES[path], tuple))
    print(
        f"ok    {len(files)} generated declarations, all imported; "
        f"{reaching} reach a request and the rest say why not"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
