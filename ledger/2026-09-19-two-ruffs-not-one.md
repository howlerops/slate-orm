# The Python client's lint is a separate run from the repository's, and only one of them was run

- **Date:** 2026-09-19
- **Author:** Claude Opus (session 015RgS5KMW89YEg1UGgZvfDo)
- **Touches:** `clients/python/tests/test_details.py`
- **Kind:** fix

## What changed

One assertion in `test_the_order_is_the_schemas_and_not_the_maps`:
`[f.check for f in check_failures_of(CHECKS_BLOB)][0]` became
`check_failures_of(CHECKS_BLOB)[0].check`. Same assertion, no list built to
throw away.

## Why

CI run 185 went red on it — `RUF015`, "prefer `next(...)` over single element
slice" — on a commit whose other seventeen jobs were green.

The interesting part is not the lint. It is that `ruff check .` was run before
that push and passed. There are **two** ruff runs in `ci.yml`: one in
`clients/python`, one at the repository root for everything else. They read
different configurations and cover disjoint trees, so a finding in
`clients/python/tests/` is invisible to the root run and vice versa.
`CLAUDE.md`'s local-checks list says so in as many words — "`ruff check .` …
the Python outside `clients/python`" — and I ran the one it names and not the
one it excludes. Nothing was newer in CI here; the same ruff in this container
reproduces the finding in one second, from the right directory.

## Alternatives rejected

**`next(f.check for f in ...)`, as the lint suggests.** It is what ruff offers
and it is worse to read: a generator and a `next` to say "the first one", when
the list is three long and already in hand. Indexing the list and taking the
attribute says the same thing with no machinery, and satisfies the lint because
no throwaway list is built.

**Silence the rule for this file.** A `noqa` would have been one character
shorter to type and would leave the next reader wondering what was special
here. Nothing is.

**Add `clients/python` to the root ruff invocation.** Tempting, and wrong: the
client is a published package with its own `pyproject.toml` and its own rule
selection, which is why it has its own job. Merging them means one of the two
configurations loses. The fix for "I ran one of two checks" is to run both, not
to have one.

## Evidence

`cd clients/python && ruff check .` reproduces the CI failure exactly, and
passes after the change. `pytest tests/test_details.py` is 21 passed either
way, which is the point: the assertion is unchanged.

## What this does not do

**Nothing stops this recurring.** The two ruff runs are still two commands and
a reader still has to know that the repository-root one does not cover the
client. A single `make lint` that ran both would fix it; that is a change to
how this repository is driven, which is worth doing deliberately rather than
while red. The same shape exists for `ty`, which also runs twice with two
environments, and for the client suites generally.
