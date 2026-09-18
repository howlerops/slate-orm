# `ruff format` is not adopted, and the measurement that decided it is written down

- **Date:** 2026-09-18
- **Author:** Claude (session on `claude/rust-orm-record-layer-gswxlu`)
- **Touches:** `pyproject.toml`, `clients/python/pyproject.toml`
- **Kind:** process

## What changed

Nothing runs differently. Two comments, one in each `pyproject.toml`, record
that `ruff format` is deliberately not run and why — so the next person who
notices Astral ships a formatter and this tree does not use it finds the
answer rather than the question.

## Why

It was an open question left over from swapping mypy for ty: ruff was already
here, `uv` and `ty` came with it, and `ruff format` is the obvious fourth. An
open question that nobody writes down gets re-opened every few months, and each
time somebody spends the same hour on it.

## Alternatives rejected

**Adopt it across both trees.** One consistent layout, no argument about it
again, and it is what most Python projects do. Rejected on the measurement
below: on the one file it changes most, it makes the file worse in a way that
matters to how this repository works.

**Adopt it with a per-file exemption for the conformance corpus.** The narrow
fix, and tempting. Rejected because a formatter you have to remember to exempt
from is a formatter that reformats the next such file on the day somebody
forgets — and "the next such file" is the whole category of deliberately
laid-out table: the corpus, the site checks' case lists, anything where a
comment and its subject read as a unit.

**Adopt it in `clients/python` only**, which would survive it: its test files
are already formatted, and its diff is small. Rejected because a formatter that
is authority in one directory and not the next is worse than one that is
neither — a contributor moving between the two cannot tell which they are in,
which is the exact problem the shared `select` list was written to avoid.

## Evidence

`ruff format --diff .` over the non-client tree: **12 of 18 files would be
reformatted**, 376 lines removed and 576 added.

Almost all of that is one file. `examples/explorer/conformance/conformance.py`
is 573 lines and would lose 215 and gain 415. It is a list of
`(name, endpoint, body, identity)` tuples, each preceded by the comment saying
what it compares and separated from the next by a blank line, so that a case
and its reasoning read as a unit. The formatter does two things to it:

- deletes the blank lines, because they are inside a list literal;
- explodes each tuple across six lines.

A file whose entire job is to be read case by case becomes one nobody scans.

`site/check/workbench.py` is the counter-example, and is why this was measured
rather than assumed: 1,142 lines, losing 16 and gaining 7, all ordinary.
`clients/python/tests/test_decimal.py` is already formatted, unchanged. So the
finding is not "the formatter is wrong" — it is "one file in this tree is laid
out on purpose and a formatter has no way to know that."

**What is still enforced.** `ruff check` selects `I`, so imports are sorted,
which is the part of formatting that actually causes diffs between
contributors. `E501` is ignored deliberately — long lines here are almost all
prose in comments, where wrapping to 100 makes the argument harder to follow
than the length does.

## What this does not do

- **No check enforces the decision.** Nothing fails if somebody runs
  `ruff format` and commits the result; the comment is the whole mechanism. A
  `ruff format --check` in CI would enforce the *opposite* of this decision, and
  there is no "assert unformatted" to run.
- **The measurement is a snapshot.** `ruff==0.16.7` is pinned in
  `clients/python`; the root tree's ruff is whatever the CI job installs with
  it. A future formatter that preserved blank lines inside a list literal would
  change the answer, and nothing here would notice.
- **It says nothing about `cargo fmt`**, which *is* run and is checked by CI —
  the Rust formatter has no equivalent of this failure because Rust has no
  blank-line-separated literal that reads as a table. `CLAUDE.md`'s note about
  `cargo fmt --all` touching other agents' files is a different concern and
  still stands.
