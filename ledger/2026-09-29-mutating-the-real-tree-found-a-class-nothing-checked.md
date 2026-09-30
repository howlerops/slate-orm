# Mutating the real tree at six guards, and the class none of them covered

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-the-dialect-mutate-py-was-missing.md`
- **Touches:** `scripts/check_cited_files.py`,
  `scripts/test_check_cited_files.py`, `scripts/check.sh`,
  `.github/workflows/ci.yml`, `docs/clickbench.md`, `docs/caveat-status.json`
- **Kind:** process

## What changed

Six guards had a real-tree mutation aimed at them for the first time, using
the runner the previous entry added. Five caught theirs. The sixth did not —
and the reason was not a defect in that guard but a gap between two of them:
**nothing in the repository checked that a file path named in `docs/` is a
file.** `scripts/check_cited_files.py` is that check, with its tests, in
`check.sh` and in `ci.yml`'s `guards` job.

`docs/clickbench.md`'s reference to `benchmarks/README.md` now says whose file
it is.

## Why

The previous entry left this:

> **Only `check_renamed_column.py` was actually mutated against.** … at least
> one of those is likely to be in the state `check_renamed_column.py` was in:
> green for a reason that has nothing to do with what it checks.

That prediction was wrong in an interesting way. Six guards were mutated and
none of them was green for the wrong reason. What the survey found instead is
the shape one level out: a guard can be entirely correct and the *set* of
guards still leave a hole, and a hole is invisible to every guard that is not
looking at it.

The hole found was this. `scripts/check_cited_tests.py` exists because
`docs/security-review.md` spent a week naming four probes that no longer
existed, each inverted by the commit that fixed the finding it demonstrated.
Its docstring is explicit that nothing compiles a document. But it matches
snake_case *identifiers*, and a path is not one. `check_cited_docs.py` runs the
other way, from source to `docs/`. So a document naming
`crates/slate-kernel/tests/oracle.rs` after that file is renamed says nothing
to anybody — the same failure, at file granularity, with the same week-long
instance on record in this repository.

## Alternatives rejected

**Widen `check_cited_tests.py` to match paths too.** One guard, one roster, one
scope declaration. Rejected because the two rules have different resolution
semantics and different false-positive sources, and merging them would mean
one green/red bit for two unrelated failures. A dead identifier is resolved
against every `fn` and `def` in the tree with a five-word threshold and a
paragraph rule; a dead path is resolved against the git index with a suffix
rule and a roster of other projects' files. They share the word "cited" and
nothing else.

**Resolve paths exactly.** Twenty of the hundred and nine citations are written
relative to a crate — `docs/performance.md` says `examples/replicas.rs` for
`crates/slate-slatedb/examples/replicas.rs`, and all twenty read correctly in
context. Requiring an exact path would flag every one of them, which is the 87%
false-positive rate `check_cited_tests.py` measured on the broad rule and
rejected. The suffix rule is weaker and it is what the evidence supports: what
this catches is a file that is not there **at all**.

**Rewrite the twenty crate-relative citations to be absolute.** Then the exact
rule works. Rejected: twenty edits to prose that is already correct, to serve a
regex, and it would make `docs/performance.md` read worse — the crate is named
in the paragraph above each one.

**Leave it, since nothing is broken.** The survey's honest answer is that
today's tree has zero dead citations. But `check_cited_tests.py` was written
for a class whose only instance had already been fixed, for the same reason:
the failure is silent, so its absence today says nothing about next month. The
distinction that would have made "leave it" right is a class that *cannot*
recur, and this one recurs every time a file is renamed.

## Evidence

**The survey.** Six guards, one real-tree mutation each, through
`scripts/mutate.py` with `scripts/mutate_guard.py` as the command:

| guard | the mutation | caught |
| --- | --- | --- |
| `check_workspace` | a crate leaves `members` in the root `Cargo.toml` | yes |
| `check_none_last` | `str \| None` becomes `None \| str` | yes |
| `check_retired_claims` | a withdrawn phrase restated in `README.md` | yes |
| `check_proto_copies` | the TypeScript proto copy's first message renamed | yes |
| `check_cited_docs` | a source file's `docs/…md` citation misspelled | yes |
| `check_client_identity` | the Go `Identity` grows a `SecretKey` field | yes |
| `check_cited_tests` | a five-word test name in `docs/` misspelled | yes |
| `check_cited_tests` | `tests/oracle.rs` in `docs/` → `tests/oracles.rs` | **no** |

The last row is the finding, and the guard is not at fault: a path is not an
identifier and its docstring never claimed otherwise. Running *all
twenty-seven* guards against that same mutation — `python3
scripts/mutate_guard.py $(ls scripts/check_*.py | grep -v test_)` — printed
`27 passed, 0 failed`. Nothing saw it.

**The exposure, measured rather than asserted.** 144 backticked file paths
across `docs/`, 109 distinct. Resolved exactly: 88. Resolved as the tail of a
tracked path: 20 more. Resolving nowhere: **one**, and it is
`docs/clickbench.md` naming `benchmarks/README.md`, which belongs to
`pgrust` — the project that page reviews, quoting the file that describes its
own benchmark methodology. So the null result is the honest headline: no
document in this repository currently points at a file that is gone. The
sentence now reads "pgrust's own `benchmarks/README.md`", which the guard
cannot tell apart from any other path — hence `EXTERNAL`, whose one entry says
whose file it is and is itself checked from both sides.

**The new guard.** `python3 scripts/check_cited_files.py` reports `ok    109
file paths cited across docs/, all resolving; 1 named as another project's`.
`python3 scripts/test_check_cited_files.py` reports `7 passed, 0 failed`.

Mutations, all caught, no survivors:
`ledger/mutations/20260929T080619-scripts-check-cited-files-py.json` (7 cases
over the rules), plus two against the real tree —
`ledger/mutations/20260929T080630-docs-correctness-md.json` (1 case: the
renamed test file, the mutation that started this) and
`ledger/mutations/20260929T080631-docs-clickbench-md.json` (1 case: the one
external citation stops being cited, and the roster entry is reported as
dead).

Two cases exist because a mutation demanded them. Reading the git index rather
than the filesystem survived everything until `a cited file on disk but not in
the index` was written — a file half-added is one a reader can open locally and
nobody else can. And the suffix rule needed a case where a *relative* citation
resolves nowhere, because with exact resolution removed the clean case already
failed and nothing distinguished the two directions.

`sh scripts/check.sh` reports `83 passed, all of them`, after a
`python3 scripts/reclaim.py` — the first attempt died with `No space left on
device` partway through, freeing 3.97 GB across 5779 files put it right, which
is the container note in `CLAUDE.md` behaving exactly as written.

## What this does not do

**Twenty guards still have no real-tree mutation.** Six of twenty-seven is a
sample, not a survey, and the sample was drawn by which mutations were cheapest
to construct — `check_handlers`, `check_write_paths`, `check_secret_types` and
`check_site_claims` are the substantial ones and none of them is here. The
finding rate so far is one gap per six guards, which if it held would mean
three more. It is one observation and should not be treated as a rate.

**Nothing re-runs any of them.** Same gap the previous entry recorded, now with
eight mutations behind it rather than five. A `scripts/mutations.json` naming
the standing suites, run on a schedule, remains the shape that would fix it.

**`EXTERNAL` has one entry and no test of its own reason.** The guard checks
that a rostered path is still cited and still does not resolve here. It cannot
check that the file really belongs to `pgrust`, which is a claim about another
repository and is verified by reading `docs/clickbench.md`, not by running
anything.

**The suffix rule accepts a citation that resolves to the wrong file.**
`tests/oracle.rs` would match any `tests/oracle.rs` in the tree, and there
could be two. Today there is one of each of the twenty, checked by hand while
writing this; nothing keeps it that way, and an ambiguous suffix reads as
resolved rather than as a question.

**Only `docs/` is in scope.** `README.md`, `CLAUDE.md` and `site/` cite paths
too, and `site/check/docs.py` checks the site's links but not its file
citations. `ledger/` is out of scope by principle, for the reason
`check_cited_tests.py` sets out: an entry is a dated record and its dead paths
are provenance.
