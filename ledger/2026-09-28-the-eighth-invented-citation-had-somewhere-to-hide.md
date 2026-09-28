# Both invented-citation guards now read the workflow, the config and the page

- **Date:** 2026-09-28
- **Author:** an agent session, closing the residual of
  `ledger/2026-09-26-the-citation-nobody-could-follow.md`
- **Touches:** `scripts/check_cited_docs.py`, `scripts/check_retired_claims.py`,
  and both of their test suites
- **Kind:** process

## What changed

`check_cited_docs.py` read four source suffixes; it now reads eleven, adding
`.tsx`, `.js`, `.sh`, `.yml`, `.yaml`, `.toml` and `.html`.
`check_retired_claims.py` read six trees and ten suffixes; it now reads
`.github/` too, and `.yml` and `.yaml` with it. One `FIXTURES` row was needed
for the widening: `.githooks/test-pre-commit.sh` `git init`s a scratch
repository and writes fourteen `ledger/…` paths into it, none of which is a
claim about this one.

## Why

The residual said where the next fabricated filename would be, in as many
words: *"a filename written in a suffix none of the four guards reads — a
.yml, a .toml, an .html, a shell script … and the eighth invented citation
will be there."* Seven have been recorded here in a fortnight and the only
thing that caught any of them was a tree something happened to open. Every one
of those suffixes is a place this repository cites an entry from today:
`.github/workflows/ci.yml` names three, `examples/explorer/head.toml` names
two, `site/docs/index.html` names two, `scripts/check.sh` names one. A reader
sent from a job comment to a file that is not there is at least as stuck as
one sent from an error message, and arguably worse off — they are already
debugging something else.

`check_retired_claims.py` had the same shape of hole for a different reason.
`.html` was added to it on 2026-09-27 after a claim struck in
`docs/orm-comparison.md` was found still standing on `site/docs/roadmap.html`;
`.github/` was never considered at all, and CI's workflow carries more prose
about this repository than most of its source files do.

## Alternatives rejected

**Read every text file, and drop the suffix list.** Simplest, and it is what
the list is approximating. Rejected on a measurement rather than a feeling:
`playwright-core`'s type definitions alone cite `docs/user_data_dir.md`, and
the vendored-tree skip list already exists because of it. A suffix list is a
second filter that fails safe — a new vendored tree with an unusual extension
is invisible rather than noisy — and a guard that is noisy gets switched off.
The cost is exactly what happened here: a suffix is missing until somebody
notices, which is why the residual predicted this and why closing it took a
year's warning to act on.

**Roster the fourteen hook-suite paths individually in `NOT_A_FILE`.** That
roster is keyed on `(file, citation)` precisely so a real record with one
illustrative path keeps its other citations checked, and it would have worked.
Rejected because `.githooks/test-pre-commit.sh` is not a record with an
illustrative path in it — it is a fixture script all the way through, which is
what `FIXTURES` is for, and fourteen rows saying the same sentence fourteen
times is a roster nobody reads before adding the fifteenth.

**Add `ledger/` to `check_retired_claims.py`'s `ROOTS` while widening it.**
Measured and rejected: two ledger entries contain a retired phrase today, and
both are the entries that *retired* it. An entry striking a claim has to quote
it, so reading entries would report every retirement as a survival — the
guard's own success looking exactly like its failure. `check_cited_docs.py`
overturned the parallel argument for citations, and the difference is real: a
citation that never resolved is a fabrication whoever reads it, whereas a
quoted phrase in a retiring entry is the retirement.

**Leave `check_cited_tests.py` and `check_caveat_citations.py` alone.** Done,
and deliberately. The second reads `docs/caveat-status.json` and has no suffix
surface at all. The first scans `docs/**/*.md` for test names and excludes
`ledger/` and `CLAUDE.md` with a written reason; widening its *source* tree
would be a different change with a different argument, and it is not what the
residual named.

## Evidence

Seven mutations, all caught by a *named* test, via `scripts/mutate.py`:
`ledger/mutations/20260928T193424-scripts-check-cited-docs-py.json` (four against `check_cited_docs.py`:
dropping `.sh`, `.yml`, `.html`, and the new `FIXTURES` row) and
`ledger/mutations/20260928T193456-scripts-check-retired-claims-py.json` (three against
`check_retired_claims.py`: dropping `.github` from `ROOTS`, `.yml`, `.yaml`).
Each of the seven was independently necessary — dropping `.github` failed both
workflow cases, dropping either suffix failed only its own.

The widening was run over the real tree before anything was rostered:
`check_cited_docs.py` went from 439 citations across source and prose to
**794**, of which **14 were broken and all 14 were in
`.githooks/test-pre-commit.sh`** — the fixture tree, now rostered; the run is
clean at **780 checked, 12 rostered**. `check_retired_claims.py` went from
**635 files scanned to 639**, the four workflows, with **zero** retired phrases
standing in any of them.

So the widening found **no defect**, which is the honest result and not the
point: two of the seven recorded fabrications were caught only because prose
happened to be readable, and these four suffixes were the ones nothing could
read at all.

`.yaml` is exercised by a fixture and by nothing else — this repository has no
`.yaml` file. That is deliberate: a suffix nothing exercises is one a mutation
deleting it survives, and this one nearly did.

## What this does not do

**The two guards still disagree about `ledger/`, and only one of them has a
test proving the disagreement is intentional.** `check_cited_docs.py` reads it;
`check_retired_claims.py` does not, on the argument above. The argument is in a
comment, and nothing fails if somebody adds `"ledger"` to `ROOTS` — the two
entries that would then report are real, so the run would go red and read as a
finding rather than as a misconfiguration.

**A citation written in a suffix still outside the eleven is still
invisible** — a `.rb`, a `.proto`, a `Dockerfile`, a bare `Makefile`. The list
is shorter than "every text file" for the reason in Alternatives, so this class
does not close; it only gets narrower each time somebody notices. The next one
will be found the way this one was, by asking what the tree contains rather
than by the guard failing.

**Existence, not aptness.** Unchanged and still the larger half: every guard
here asks whether a cited file opens, and none asks whether it says what the
citing sentence claims. That caveat is recorded `deliberate` against the
original entry and this widening does not approach it.
