# Batch eight: nine still true, one closed by a fix made the same afternoon the caveat was written. The guard's own source comment says so, and nothing joined the two up for six days.

- **Date:** 2026-09-26
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `docs/caveat-status.json`
- **Kind:** process — re-triage, no code

## What changed

Ten caveats from 2026-09-19 and 2026-09-20 read against the tree. Nine still
true and stamped. One closed: **"It covers one file"**, about
`scripts/check_handlers.py` hard-coding `service.rs`. It does not — `SOURCES`
is `crates/slate-server/src` and `crates/slate-serverd/src`, walked with
`rglob("*.rs")`.

## Why

The closure is the seventh stale caveat found by reading and the sixth with the
same shape, and it is the sharpest instance yet because **the fix documents the
caveat by name and still did not close it**. From `check_handlers.py:85`:

> Directories rather than one file, because the first version of this check
> read `service.rs` alone and missed a `fingerprint::check` in `convert.rs` — a
> file away, reachable only through callers, and invisible to a check whose
> whole subject is "is this reachable without authorising". **That is the
> caveat this check's own entry named, biting within the hour.**

So somebody wrote the caveat, hit it the same afternoon, fixed it, and wrote
down that they had hit the caveat — in the *guard's source*, where the tracker
cannot see it, rather than in the tracker. The entry is append-only and correct
as of its date; the status lives in `docs/caveat-status.json`; and the person
holding both facts wrote them in a third place.

That is worth naming because it is not the same failure as the previous five.
Those were two entries that did not know about each other. This is one person,
one afternoon, one fact, recorded in the wrong file. The tracker existed by
then — `2026-09-25` is when it was built, so it did not. The caveat has been
closable since the hour it was written and unclosed for six days because there
was nowhere to say so until yesterday.

## Alternatives rejected

**Treat a source comment naming a caveat as a closure signal.** Tempting after
this: grep the tree for "caveat" near a `ledger/` citation and reconcile. The
citation guard (`scripts/check_cited_docs.py`) already proves such citations
exist and are real filenames. Rejected because the comment here does not cite a
file — it says "this check's own entry", which resolves only if you know which
entry that is. A grep for the *phrase* would have found this one and nothing
else, which is the same false-positive rate the two failed heuristics had.

**Widen the tracker to accept a `by:` pointing at a source comment.** It
already does: `by` is free text and this closure's `by` names the file and the
line. Nothing structural was missing — only the act of writing it down.

## Evidence

| caveat | checked against | verdict |
|---|---|---|
| the handler check covers one file | `SOURCES` at `scripts/check_handlers.py:91`, two directories, `rglob("*.rs")` at `:494` | **closed** |
| the Python client has no `ForeignKey` type | no occurrence in `clients/python/src/slate/` | still true |
| multi-line `run:` blocks are matched by name | four now, not three — see below | still true |
| no named reverse relation is generated | `scripts/codegen.py:549` generates child→parent foreign keys only | still true |
| Python's parent helper is in the adapter | no `parents` helper in the client package | still true |
| nothing validates against the published checks before sending | no client-side validation anywhere | still true |
| nothing checks Go and TypeScript for the constructor drift | no guard; the reading in that entry is still the only evidence | still true |
| `purge_deleted` needs two grants | unchanged | still true |
| `RESTRICT` on a parent's soft delete is not revisited | unchanged | still true |
| nothing checks the Go or TypeScript identity formatter | no `String()` on Go's `Identity`, no `toString()` on TypeScript's | still true |

**A count that drifted without the claim going stale.** The `run: |` caveat
says "Three exist and none is a static check." There are **four** now — `Both
binaries exist`, `The generated declarations match the catalog`, `The retention
example's declaration matches its catalog`, `Start MinIO`. The number is wrong
and the claim is not: none of the four is a static check, so none belongs in
`scripts/check.sh`, and the hole the caveat warns about — a static check hidden
inside an existing block — has not opened. The new block was added by name and
`scripts/test_check_sh.py` accounts for it, which is the guard working.

Left open with the stale count rather than corrected, because an entry is
append-only. A reader who counts will find four and should; the claim they are
reading is the sentence after the number.

**No mutation run.** Nothing executable changed.

**Counts.** `scripts/caveats.py`: **849 caveats: 290 open, 187 closed, 317
deliberate, 0 untriaged.** Seven closed today over 80 reads.

## What this does not do

**Five of the ten were checked by absence** — no `ForeignKey`, no `parents`
helper, no guard, no `String()`, no client-side validation. A search that finds
nothing is weaker than finding the thing and reading it, and a differently
spelled implementation would pass all five.

**It does not go looking for more caveats closed in a source comment.** This
one surfaced because the comment happened to be in a file the check read. There
may be others and the alternatives above say why grepping for them does not
work; nothing systematic was attempted.

**The `run: |` count will drift again.** Nothing ties the number in that entry
to `ci.yml`, and nothing should — the entry is a dated record. But a reader
comparing them will find a discrepancy and has only this note to tell them it
is expected.
