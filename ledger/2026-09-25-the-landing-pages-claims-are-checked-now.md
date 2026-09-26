# The landing page says "No `unsafe`", "100,000 trips" and "265 zones", and nothing read any of it. A guard does now — and found that four of twelve crates did not forbid `unsafe`, so the first claim was true and unenforced.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `scripts/check_site_claims.py` (new), `scripts/test_check_site_claims.py` (new), `crates/slate-sql/src/lib.rs`, `crates/slate-wasm/src/lib.rs`, `crates/slate-clickbench/src/main.rs`, `scripts/check.sh`, `.github/workflows/ci.yml`
- **Kind:** process, and one real gap closed

## What changed

A guard that reads `site/index.html` and checks the claims in it against the
repository: no `unsafe` block anywhere and every crate root forbidding it; the
trip count against `make-trips.py`'s `SAMPLE`; the row count in the hero's plan
line against the same number; the zone count against `taxi_zones.csv`; the
three named clients against `clients/`. `UNCHECKED` is a roster of the claims
it cannot check, each with a reason and each required to still be on the page.

And `#![forbid(unsafe_code)]` in the three crate roots that lacked it.

## Why

Two entries recorded this gap in the same words:
`2026-09-16-the-site-looks-like-the-family-it-belongs-to.md` — "the landing
page's claims are not checked by anything ... Both are true today" — and
`2026-09-14-the-home-page-is-a-workbench.md`, "nothing verifies the two pages
agree". `docs.py` checks structure and `quickstarts.py` runs code; neither
reads a sentence.

The landing page is the one document a reader believes before they believe
anything else, and every number on it is a number somebody typed.

## What it found

**Four of twelve crates did not forbid `unsafe`**: `slate-sql`, `slate-wasm`,
`slate-clickbench`, and `slate-serverd` — which turned out to have it already,
making it three. There was no `unsafe` block anywhere, so the page's claim was
true. It was also unenforced: nothing would have failed if the next edit to
`slate-wasm` had reached for one, and `slate-wasm` is the crate most likely to,
because it is the one talking to a foreign runtime.

That is the shape of the whole session: a claim that is true, that nothing
keeps true, in a place a reader takes on trust.

`forbid` rather than `deny`, so a module cannot re-allow it locally; and it
compiles clean through `wasm-bindgen`, which was the one I expected to need an
exception.

## Alternatives rejected

**Check the prose with a language model, or with a keyword heuristic.** The
tempting version is a check that "reads" every sentence. It would be a check
whose failures are arguments, and this repository's guards are all decidable:
a file resolves or it does not, a name is in a list or it is not.

**Check nothing, and rely on review.** That is the status quo and it held for
nine days across two entries recording that it would not.

**Put every unverifiable sentence in `UNCHECKED`.** It would grow to the page,
and a roster that names everything names nothing. Four entries, each about a
claim of *character* rather than of fact — lineage, project status, a design
summary. The keys are distinctive phrases that must still appear, so a
rewritten claim forces a visit here rather than leaving the roster describing
sentences nobody wrote any more. Every roster in this repository has had that
failure; this one is built to have it caught, and
`a claim the page has stopped making is reported` is the case.

**`site/check/claims.py`.** Where it started, and wrong: it reads `crates/`
and `clients/`. The three files in `site/check/` confine themselves to
`site/`, and this is a repository check whose subject happens to be the site.
Moving it also made it importable by its own test — `site/check` is a `ty` root
and `scripts/` is another, and a test in one importing a module in the other
resolves at runtime and not for the type checker. That failure was CI-shaped:
`ty` flagged it here, which is the check `CLAUDE.md` says to run because CI's
environment is thinner than this one.

## Evidence

**Mutations**, eight run, eight caught, no survivors:

| mutation | caught by |
|---|---|
| the `forbid` check ignores the crate root's text | `a crate root that does not forbid unsafe is reported` |
| a crate with no root at all is skipped rather than reported | its own case |
| the `unsafe` scan matches the word instead of a use | four cases, including `the word unsafe in prose is not a use` |
| the trip count is not compared to the generator | `a trip count that drifted from the generator is reported` |
| the plan line's row count is not compared | two cases |
| the zone count is not compared to the CSV | its own case |
| a named client need not be a directory | its own case |
| the roster's phrases need not be on the page | `a claim the page has stopped making is reported` |

**The guard's own suite: 11 passed, 0 failed**, every case over a tree it
writes rather than over `slate-orm` — for the reason the other guards here
give: a check tested only against a clean tree asserts what a check that does
nothing asserts.

**Against the real page: 12 checks, all holding**, and 4 claims listed as
unchecked.

**Everything else.** `sh scripts/check.sh`: 55 passed, all of them (53 before;
the two new steps). `cargo build --workspace` after the three `forbid` lines:
clean, including `slate-wasm`. `cargo clippy --workspace --all-targets`: zero
warnings. `ruff`, both roots: clean. `ty` against the CI virtualenv: clean.
`scripts/test_check_sh.py`: 90 CI steps, all accounted for.

**The runs themselves**, as `mutate.py` recorded them:

- `ledger/mutations/20260925T232956-site-check-claims-py.json` — site/check/claims.py, the run interrupted part-way
- `ledger/mutations/20260925T233013-site-check-claims-py.json` — site/check/claims.py, the full run

## What this does not do

**It reads one page.** `site/docs/*.html` carry claims too — `limits.html`
says which clauses take `OR`, `features.html` lists computed calls — and
nothing checks those either. The landing page was chosen because it is the
first thing read and the least likely to be re-read; the docs pages are a
larger surface and a separate change.

**"Nothing verifies the two pages agree" is only half closed.** The numbers
are now checked against the repository, which is stronger than checking them
against each other — two pages can agree and both be wrong. What is *not*
checked is that the prose of `docs/index.html` and the landing page tell the
same story, which is the unverifiable kind.

**It cannot notice a claim that was never made.** A feature that shipped and
the page does not mention is invisible here.
`ledger/2026-09-23-five-built-features-the-docs-never-mentioned.md` is about
that direction
and this does nothing for it.

**`UNCHECKED` has four entries and the page has more than four unverifiable
sentences.** The four are the ones a reader would most likely take as fact. The
rest are plainly opinion, and drawing that line is a judgement nothing here
enforces.
