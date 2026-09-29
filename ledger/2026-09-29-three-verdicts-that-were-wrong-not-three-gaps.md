# Three verdicts that were wrong, not three gaps

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`, `scripts/check_closed_caveats.py`
- **Kind:** process

## What changed

Three open caveats re-read against the tree and found to be stale — the work
that answers them landed after they were written and nobody moved the verdict.
Open 124 → 121.

- **`No live-row case for authors, sales or editions in any language.`** →
  **closed.** All three demo adapters' `/api/typed` read those three tables and
  decode each with the generated row type, and the conformance runner compares
  the three.
- **`The decoders still never see a row the server sent.`** → **narrowed.** The
  same handlers do exactly that, for all five tables. What remains true is the
  sentence's own subject: the three decoder *test files* still build values by
  hand.
- **`No tracing.`** → **deliberate.** `crates/slate-serverd/src/observe.rs`
  carries a *Why not `tracing`* section: without a subscriber it logs nothing,
  so adopting it means adopting a subscriber and its configuration surface, and
  the file names the condition under which that becomes the right answer. A
  decision with its reasoning written down is not a gap.

## Why

The tracker's whole claim is that a verdict is re-derivable. A caveat that is
`open` while the thing it asks for exists is the same failure the tracker was
built to stop, pointed inward — and two of these had been answered for nine
days.

It also answers a question this session kept running into: the open count rises
because every entry adds caveats, so the only way it falls is either building
features or finding verdicts that are wrong. This is the second kind, and it is
worth doing before the first, because building a feature that already exists is
the most expensive way to discover a stale verdict.

## Alternatives rejected

**Re-read all 79 open caveats last checked on 2026-09-28.** The right sweep and
not what happened: a heuristic surfaced eleven that name an identifier the tree
still has, and of the eleven, one was genuinely stale. The other ten named
common words — `Value`, `message`, `returning` — and the grep says nothing. The
two extra closures here came from reading the decoder cluster directly, not from
the heuristic. So the sweep was not done; three were.

**Close the second one rather than narrow it.** The gap it names — "the
decoders never see a row the server sent" — is false. Its literal subject, the
test files this entry's parent added, still build by hand. Closing it would
lose that, and narrowing it keeps both halves true.

**Leave `No tracing.` open, since `tracing` is still absent.** The verdict
vocabulary distinguishes a gap from a decision precisely so a reader can tell
"nobody has done this" from "somebody decided against this, here is why". An
`open` verdict over a written rationale mis-sorts it into the backlog.

## Evidence

`python3 scripts/caveats.py` reports **`1757 caveats: 121 open, 94 narrowed,
439 closed, 964 deliberate, 0 untriaged`**, from 124 open before.

The live-row coverage, read rather than assumed —
`examples/explorer/backends/python/adapter/__main__.py` reads `authors` 1,
`sales` 100 and `editions` 500 and calls `Authors.from_row`, `Sales.from_row`,
`Editions.from_row`; `backends/go/handlers.go` calls `schema.ScanAuthors`,
`ScanSales`, `ScanEditions` on rows from `session.Get`; `backends/node/src/main.ts`
calls `decodeAuthors`, `decodeSales`, `decodeEditions`. The conformance runner
carries `("two rows through the generated decoders", "/api/typed", {}, "app")`.

The `tracing` rationale is at `crates/slate-serverd/src/observe.rs:15`, headed
`# Why not `tracing``.

One candidate was checked and left open: `2026-09-21-a-window-crosses-the-wire-in-its-own-list.md`'s
`COPIES is still one pair and still has no completeness check`.
`check_proto_copies.COPIES` is still exactly one pair, so it stands.

`sh scripts/check.sh` reports `85 passed, all of them`.

## What this does not do

**Seventy-six open caveats last checked on 2026-09-28 were not re-read.** Three
were, because a cluster of them came up while looking for work. The honest rate
so far is three stale in the handful examined, which says nothing reliable about
the rest.

**The heuristic that surfaced candidates is nearly useless.** Grepping for
backticked identifiers from a caveat's text finds `Value` in 352 files. A sweep
worth running would compare each caveat's date against the git log of the files
its entry touches, which is a tool nobody has written.

**Nothing stops this recurring.** A verdict goes stale the moment work lands
elsewhere, and the tracker has no rule that says "this caveat's subject was
touched since it was checked". `checked` is a date a person writes, not one
anything derives.
