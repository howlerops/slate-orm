#!/usr/bin/env python3
"""`None` goes at the end of a type union, everywhere in this repository's Python.

    python3 scripts/check_none_last.py

`ledger/2026-09-20-cis-ruff-is-newer-than-mine-too.md` records the failure this
prevents: CI's `ruff` knows `RUF036` — *"`None` not at the end of the type
union"* — and this container's does not, so a green local `ruff check` and a
red `scripts` job happened twice in one annotation. Its own caveat said what
was missing:

  > **Two occurrences, one rule.** I did not sweep the tree for other
  > annotations that put `None` first; `ruff --fix` with a newer version would,
  > and running one is the thing that is not possible here.

This is the sweep, and it runs every time.

# Why an AST walk rather than the lint

Because the lint is not available. `RUF036` is a **preview** rule in the
`ruff` this container has (`--select RUF036` answers *"has no effect because
preview is not enabled"*), and enabling preview repository-wide to get one rule
turns on every other unstable rule with it — which is how a lint config becomes
a thing people disable. CI's `ruff` may have it stable, may not, and will
certainly have it before this repository notices.

A walk over `ast` needs no lint at all, so it cannot be newer or older than
anything.

# It is deliberately stricter than the lint

`RUF036` reads *annotations*. Run with `--preview`, it passes on this
repository's one real occurrence:

```python
PyValue = None | Null | bool | int | float | str | bytes | _uuid.UUID | Vector | Array
```

because a bare assignment is a type *alias*, not an annotation. Both are the
same type — `None | X` and `X | None` are identical to every type checker —
so nothing was wrong; but the house rule `CLAUDE.md` adopted after two red
builds is "write `str | dict[str, str] | None`", and a rule that holds in
annotations and not in the aliases beside them is a rule nobody can apply from
memory. The alias was reordered and this checks both.

That also means this can go red on a file `ruff` calls clean, which is the
opposite of the usual direction here and is stated so nobody reads a failure as
a `ruff` disagreement.

# What it cannot see

A union spelled `Optional[X]`, `Union[None, X]`, or in a string annotation.
None occurs in this tree — checked when this was written — and each would need
its own arm. The union operator is what the house rule is about and what
`RUF036` reads.
"""

from __future__ import annotations

import ast
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: Trees that are vendored, generated, or not ours. The same list
#: `check_cited_docs.py` carries, for the same reason.
SKIP = frozenset(
    {
        "node_modules",
        "dist",
        "dist-test",
        "target",
        ".git",
        "_proto",
        "__pycache__",
        "venv",
        ".venv",
        "ci-env",
    }
)


def operands(node: ast.expr) -> list[ast.expr]:
    """Flatten an `A | B | C` chain into its parts, left to right.

    `ast` parses `A | B | C` as `(A | B) | C`, so a walk that looked at each
    `BinOp` on its own would see `A | B` and call `A` "not last" when it is not
    last *of that node* and is fine in the whole union. Flattening first is
    what makes the question askable.
    """
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):
        return operands(node.left) + operands(node.right)
    return [node]


def is_none(node: ast.expr) -> bool:
    return isinstance(node, ast.Constant) and node.value is None


def offenders(source: str) -> list[tuple[int, str]]:
    """`(line, the union as written)` for each union with `None` not last."""
    try:
        tree = ast.parse(source)
    except SyntaxError:
        # A file this Python cannot parse is not this check's business: the
        # suite that owns it will say so, and guessing would report a syntax
        # error as a style finding.
        return []
    found: list[tuple[int, str]] = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.BinOp) or not isinstance(node.op, ast.BitOr):
            continue
        parts = operands(node)
        # Only the outermost node of a chain: an inner `A | B` of `A | B | None`
        # is the same union seen half-finished, and reporting it would turn one
        # finding into as many as the chain is long.
        if any(is_none(part) for part in parts[:-1]) and not _nested(tree, node):
            found.append((node.lineno, ast.unparse(node)))
    return found


def _nested(tree: ast.AST, node: ast.BinOp) -> bool:
    """Is `node` the left arm of a wider `|` chain?"""
    for other in ast.walk(tree):
        if (
            isinstance(other, ast.BinOp)
            and isinstance(other.op, ast.BitOr)
            and other.left is node
        ):
            return True
    return False


def check(root: Path = ROOT) -> list[tuple[str, bool, str]]:
    """`(what, ok, detail)` per check, in the shape the other guards use."""
    out: list[tuple[str, bool, str]] = []
    bad: list[str] = []
    read = 0
    for path in sorted(root.rglob("*.py")):
        if SKIP & set(path.relative_to(root).parts):
            continue
        read += 1
        for line, union in offenders(path.read_text(encoding="utf-8", errors="replace")):
            bad.append(f"{path.relative_to(root)}:{line}  {union}")

    # The never-fires guard every check in this directory carries. A `rglob`
    # that matched nothing reports no offender and reads exactly like a tree
    # where every union is correct.
    out.append(("some Python was read at all", read > 0, f"{read} files"))
    out.append(
        (
            "every type union writes `None` last",
            not bad,
            "\n      ".join(bad),
        )
    )
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check():
        if ok:
            print(f"ok    {what}" + (f"  ({detail})" if detail else ""))
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
            print(
                "      Write `str | dict[str, str] | None`. CI's ruff knows "
                "RUF036 and this container's does not, so the build that finds "
                "this otherwise is not one you can run here."
            )
    print()
    if failed:
        print(f"{failed} failed")
        return 1
    print("`None` is last in every type union this repository writes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
