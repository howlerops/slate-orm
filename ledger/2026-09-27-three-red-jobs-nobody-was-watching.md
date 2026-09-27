# Reading CI's own conclusion after a batch of pushes found three failures: a guard that resolved a path against the wrong thing, a browser expectation that had gone stale, and a registry that stopped answering.

- **Date:** 2026-09-27
- **Author:** Claude (agent session), at jacob.beck.018@gmail.com's direction
- **Touches:** `scripts/check_caveat_citations.py`, `scripts/test_check_caveat_citations.py`, `site/check/workbench.py`, `.github/workflows/*.yml`, `CLAUDE.md`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

Four things, all of them found by asking GitHub how the last five runs ended
rather than by anything in this container:

1. `check_caveat_citations.py` asks git what a checkout will **not** carry: a
   cited path must be on disk *and* not ignored. A verdict cited
   `site/slate_wasm_bg.wasm`, a build output in `.gitignore`: present on the
   machine that built it, absent from the checkout. The guard printed `ok` here
   and failed CI on a file nobody had touched. The verdict now names the built
   bundle without a path.
2. The workbench's kitchen-sink case asserted the shape of the example's
   *second* statement; the example grew a third, and the grid shows the last
   one. It now counts the statements that ran and refuses none of them.
3. Every `actions/*` in the workflows that still targets Node 20 moves to its
   first Node 24 major: `checkout@v5`, `setup-node@v5`, `setup-python@v6`,
   `setup-go@v6`.
4. `CLAUDE.md` says to read the run's conclusion, with the two calls that do it.

## Why

`ledger/2026-09-14-ci-clippy-is-newer-than-mine.md` left two caveats, and both
are about the same blindness:

> **It does not close the reporting gap.** The reason three red runs went
> unnoticed is that the watch I set matched on the Pages string and expired
> after thirty minutes without ever reporting the CI conclusion. [...] the
> honest fix is to check the run's conclusion explicitly rather than trusting a
> watch to surface it.

> **It does not address the deprecation warnings** in the same log —
> `actions/checkout@v4` on Node 20 — which are noise today and a failure
> whenever GitHub finishes the removal.

Six commits went out this session on a green local `check.sh`, and one of them
was red in CI the whole time. Nothing in this container says so: `check.sh`
runs 67 checks and is explicit that it is necessary and not sufficient, and
after a push there is no signal at all unless somebody asks for one.

## Alternatives rejected

**A watch, again, on the right string.** What was tried before, and the failure
mode is structural rather than a bad pattern: a watch expires, and an expiry
reads exactly like silence. Asking is two calls and cannot time out into a
false green.

**Exempt build outputs from the citation guard with a roster.** The obvious fix
for finding 1 and the wrong one: a roster of generated paths is a second list
to keep true, and the *reason* the guard was wrong is not that `.wasm` files
are special — it is that "exists" meant two different things in two places.
Asking git makes the local run and CI's ask the same question, and `.gitignore`
is a roster somebody already maintains.

**Resolve against `git ls-files` — what git *tracks*.** The first version, and
it fails the commit that adds the cited file: a new ledger entry is untracked
until `git add`, and `check.sh` is run before staging. It failed on this very
entry. Ignored-ness is the right question — an ignored path is never going to
be in a checkout; an untracked one is about to be.

**Bump every action to its newest major.** `checkout` is on v7. Rejected for
the first Node 24 major of each instead: the warning is about a runtime, and
the smallest change that ends it is the one whose inputs are least likely to
have moved. `download-artifact` is left at v4 — measured, **no** major of it
targets Node 24 yet, and upload/download must stay in the same family.
`upload-pages-artifact` is a composite action with no Node runtime at all.

**Change the MinIO image so that job goes green.** Rejected as a guess. See
below.

## Evidence

**Run 381 (`a60ce46`), three jobs failed**, none of them caused by that
commit's diff:

| job | failure |
| --- | --- |
| the Python that is not the client | `2026-09-15-a-month-is-not-a-number-of-seconds.md: site/slate_wasm_bg.wasm` |
| the kernel in a browser | `the kitchen sink example runs when a reader clicks it` |
| integration against MinIO | `docker: unauthorized: access to the requested resource is not authorized` |

**Finding 1 reproduces here now, and did not before.** With the guard reading
the filesystem, `check_caveat_citations.py` printed `ok` over
`site/slate_wasm_bg.wasm`, a file `git ls-files` has never listed. With it
reading the index, the same run reports the same line CI reported. That is the
point of the change: the two runs now ask the same question.

**Finding 2 was a stale expectation, not a defect, and the difference was worth
establishing.** The rebuilt bundle reproduced the failure locally:

```
{'status': '10 rows · 223.4 ms',
 'headers': ['ID','PICKUP_ZONE','DROPOFF_ZONE','PICKUP_TIME','DURATION',
             'PASSENGERS','DISTANCE','FARE','TIP','TOTAL','PAYMENT'],
 'rows': 10}
```

The log shows all three statements ran — 1 row, 20 rows, 10 rows — so the grid
is showing the third, which the case did not know about. Eleven headers for a
`SELECT pickup_zone, fare` looked like a lost projection and is not: the spec
carries `"columns": [1, 7]`, and the grid deliberately shows every column and
marks the ones the plan never read, which the case *"a column the plan never
read is not rendered as a null"* three screens above is asserting. A plain
`SELECT pickup_zone, fare FROM trips WHERE fare > 20 ORDER BY fare DESC LIMIT
10` renders the same eleven, which is what ruled the bracketed disjunction out
as the cause.

**Finding 3's versions are measured, not recalled.** `action.yml`'s
`runs.using`, read from each tag over the git proxy:

| action | v4 | v5 | v6 | v7 |
| --- | --- | --- | --- | --- |
| `checkout` | node20 | **node24** | node24 | node24 |
| `setup-node` | node20 | **node24** | node24 | node24 |
| `setup-python` | node16 | node20 | **node24** | node24 |
| `setup-go` | — | node20 | **node24** | — |
| `download-artifact` | node20 | node20 | node20 | — |
| `upload-pages-artifact` | — | composite | composite | — |

**Mutations.** Three runs, ten cases:
`ledger/mutations/20260927T000800-scripts-check-caveat-citations-py.json` (four
cases against the tracked-file version, three caught, one survivor),
`ledger/mutations/20260927T000814-scripts-check-caveat-citations-py.json` (that
survivor re-run after its test was written: caught), and
`ledger/mutations/20260927T001251-scripts-check-caveat-citations-py.json` (five
against the ignored-path version, four caught, one survivor).

| mutation | outcome |
| --- | --- |
| the ignore list is never consulted | caught, 2 named cases |
| an ignored parent directory does not hide its contents | caught |
| a path that is not on disk resolves anyway | caught, 3 named cases |
| the ignored marker is read as any status line | caught, 2 named cases |
| git failing reads as nothing-ignored rather than no answer | **survived** |

Two survivors, two different lessons. Against the tracked-file version, *"a
cited directory no longer resolves"* survived because every fixture cited a
file and `git ls-files` never names a directory — a missing test, written and
then caught.

The second is redundant code and it is the more interesting one, because the
redundancy was *created* by the rewrite: under the tracked-file rule, "git
could not answer" and "git tracks nothing" wanted opposite behaviour, so
`ignored` returned `None` for one and a set for the other. Under the
ignored-path rule they want the same behaviour — fall back to the filesystem —
and the `None` branch stopped meaning anything. It was deleted. A safety check
that survives a rewrite is a safety check nobody re-derived.

**And the `ty`/`ruff` gap, in the same commit that is about it.** The first
draft of the test helper passed `subprocess.run`'s quiet kwargs through a
`**dict`; `ty` cannot pick an overload through that, and `ruff` wanted the new
import sorted. Both were caught by `check.sh` before the push this time.

**Suites.** `scripts/test_check_caveat_citations.py` 34 passed 0 failed (was
30). `site/check/workbench.py`: the whole browser suite green against a
freshly built bundle. `sh scripts/check.sh` 67 of 67.

## What this does not do

**The MinIO job is still red, and it is not this repository's to fix today.**
`quay.io/minio/minio:RELEASE.2025-09-07T16-13-09Z` now answers an anonymous
pull with `unauthorized`, and quay's own API answers `Requires authentication`
for the tag listing. Docker Hub has no `minio/minio` tag by that name — checked
— so there is no verified replacement to port, and swapping the image on a
guess turns a known failure into an unknown one. What it needs is either a
registry credential in the workflow's secrets or a tag somebody can confirm
pullable, and neither can be established from this container.

**The Node 24 bump is unverified until CI runs it.** Four actions, seven files,
inputs unchanged — `python-version`, `node-version`, `cache`,
`cache-dependency-path`, `go-version` are stable across these majors — but the
proof is the next run, and if a major renamed an input the job says so
immediately. That is the deliberate trade: `download-artifact`, the one whose
pairing could break quietly, was left alone.

**Reading the conclusion is a practice, not a check.** `CLAUDE.md` now says to
do it and gives the two calls. Nothing enforces it, and nothing can from
inside the repository: the thing being checked is whether a session asks.
