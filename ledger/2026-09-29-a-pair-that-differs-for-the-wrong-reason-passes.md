# A pair that differs for the wrong reason passes, and a renamed plan is invisible

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `examples/explorer/conformance/conformance.py`,
  `examples/explorer/conformance/test_conformance.py`
- **Kind:** fix

## What changed

Two gaps in the three-SDK conformance runner, both in what it compares rather
than in what any client does.

**Every `MUST_DIFFER` pair now names the field the difference has to be in.**
The check was "the two rendered answers are not byte-identical", which two
cases satisfy for reasons that have nothing to do with the field under test. A
pair is a triple now — `(quiet, loud, field)` — and a pair whose field is
missing from either answer is reported rather than comparing `None` to `None`.
Fourteen pairs, each naming `rows`, `access` or `calls`.

**`EXPECTED_ACCESS` rosters the plan every case that reports one must carry.**
`MUST_DIFFER` compares two plans to each other, so a node that renamed both —
`Table Scan` to `Seq Scan` — keeps every pair differing and every adapter
agreeing. Twelve cases, both directions: an answer carrying a plan nobody
rostered fails, and a roster line whose case stopped reporting one fails as
stale.

Two things came out of the probing rather than the plan. A comment said a
reader's search cuts *five* rows to four; it is six to five, and `CONTRACT.md`
already said six — the runner's copy of the number was written before the
corpus grew and never re-read. And `test_conformance.py` died with a traceback
rather than a summary line when a check group raised, which is
`scripts/mutate.py`'s second lie; a raised group is a failing check now.

## Why

Two caveats, both from entries that named the gap and left it:

> `MUST_DIFFER` still compares whole answers. Two cases that differ for a
> reason unrelated to the field under test would satisfy it, and nothing
> checks the difference is the right one.
> — `ledger/2026-09-19-which-guard-covers-which-field.md`

> `access` is compared only for equality between two cases; nothing asserts its
> content, so a server that renamed every plan would keep the corpus green.
> — `ledger/2026-09-21-a-search-endpoint-the-demo-can-serve-two-ways.md`

Both are about the same shape: the runner compares *relationships* and the
relationship holding is weaker than it reads. `MUST_DIFFER` exists because
three clients agreeing cannot see a field all three dropped — but a pair
satisfied by an unrelated `servedBy` is a test of nothing that reads as a test
of something, which is worse than no pair at all, because the roster is where
somebody looks to find out what is covered. And the plan text is the one thing
in this corpus that is *the server's* words rather than a value: the rows are
identical by construction on both access paths, so `access` is the entire
content of the search comparison, and nothing had ever asserted what it says.

## Alternatives rejected

**A JSON path rather than a top-level field.** `rows[*].windowed`, say, would
let a window pair insist the difference is in the windowed values and not in
the rows. Every pair in the file today is satisfied by a top-level key, so a
path syntax would be a small expression language written for no current
caller, and the first thing to go stale when somebody needs the second
feature of it. A top-level name is one dictionary lookup and no parser.

**Leave the field optional, defaulting to the whole answer.** Fewer lines
changed and the existing pairs keep working. Rejected for the reason the
repository gives about every roster it has: a list you are forced to edit is a
list that stays true. An optional field is a field nobody fills in, and the
pairs that most need one are the ones somebody adds in a hurry.

**Assert the access path in each client's own suite instead.** They could, and
two of them do assert *an* index is used. It would not close this: the claim
here is that the three SDKs and the server agree about a string that crosses
the wire, and three suites asserting it separately is exactly the shape
`ledger/2026-09-28-the-go-client-was-already-counting.md` argued against for
round-trip counts. One roster, compared against what all three received.

**Roster only the searches.** The caveat is about `/api/search`. Rejected
because `/api/explain` answers carry an `access` too and have the identical
weakness, and a roster that covered one endpoint would need a rule for why —
"every case whose answer has the key" needs no rule and forced two more lines.

**Make the runner tolerate a missing field with `.get`.** It would have let
the deleted-guard mutations fail cleanly instead of crashing the harness. It
is dead code standing in for a test: the guard is what makes `[field]` safe,
and adding a fallback so the guard can be mutation-tested is testing the
fallback. The harness catching a raised group is the fix, and it is worth
having for every future check group too.

## Evidence

- `examples/explorer/run.sh --conformance`: **137 cases, the three SDKs agree
  on all of them. 137 passed, 0 failed** — with the field check and the roster
  both live.
- `python3 examples/explorer/conformance/test_conformance.py`: **88 passed, 0
  failed**, up from 79. Nine new checks — four on `access_findings`, two on the
  pair check's new arms, three on the harness itself.
- The twelve rostered plans, read off a running stack rather than written from
  the planner's source: nine `Index Scan using by_title_text` or `Table Scan`
  across the six searches and the three no-row cases, `None` for the reader's
  search (no `explain` grant), `Table Scan` for `a plan` and `Point Get` for
  `a plan under a filter`.
- **Eight mutations. Seven caught on the first attempt, one survived and is
  caught now.** Five against `test_conformance.py`
  (`ledger/mutations/20260929T021617-examples-explorer-conformance-conformance-py.json`):
  the pair check comparing whole answers again → *a pair that differed
  somewhere else is still reported*; the absent-field guard deleted → *the
  pairs checks build without raising*; the roster's stale half looping over
  nothing → *a roster line for a case that reported no plan is reported*; the
  value comparison disabled → *a renamed plan is reported*; the unrostered
  arm disabled → *the access checks build without raising*. And one against
  the live run
  (`ledger/mutations/20260929T021627-examples-explorer-conformance-conformance-py.json`):
  rostering `Table Scan` for the case that hints the index → *a search for
  'the' by index: the plan's access path changed*.
- And two more against the harness
  (`ledger/mutations/20260929T022241-examples-explorer-conformance-test-conformance-py.json`).
  The first of them **survived** on the first attempt
  (`ledger/mutations/20260929T022209-examples-explorer-conformance-test-conformance-py.json`)
  and the survivor was right: emptying the `except` arm changes nothing,
  because nothing on a clean tree raises. The arm exists only for a tree
  somebody has broken, which is the tree `mutate.py` builds — so the test is
  one that breaks a group on purpose, and both mutations are caught with it.
- The first attempt at those five
  (`ledger/mutations/20260929T021527-examples-explorer-conformance-conformance-py.json`,
  outcome `problems`) is why the harness changed: two of the five reported
  `NOTHING RAN` because deleting a guard turned the next line into a
  `KeyError` and the file died before printing a summary. Three caught, two
  unscoreable — recorded rather than quietly re-run.
- `python3 scripts/caveats.py`: 1627 caveats, 111 open, 72 narrowed, 402
  closed, 909 deliberate, 0 untriaged.
- `sh scripts/check.sh`: 72 passed, all of them.

## What this does not do

**A top-level field is not a path.** A window pair insists the difference is
in `rows`, which is the whole answer for that endpoint — so a client that got
the *row* right and the windowed value wrong, in a way that still differed,
satisfies the pair. Narrower than "not byte-identical" and wider than the
claim each pair's comment makes.

**The roster is a snapshot of this planner on this fixture.** Eleven books and
one text index: `Point Get` and `Table Scan` are what the demo's data
produces, and a change to the seed that moved a plan would fail these lines
correctly and tell a reader nothing about whether the planner or the fixture
moved. The failure names both strings so the diff is readable; deciding which
one is right is still the reading.

**Nothing rosters the rest of an `EXPLAIN`.** `indexOnly`, `sorts`, `residual`
and `display` are compared across the three adapters and asserted against
nothing, exactly as `access` was until now. The same argument applies to each
of them and the same roster would hold them; this stopped at the field the
caveat named rather than widening on its own.

**The other field in that entry's pair of caveats is untouched.** "The probe
covered `paged` and `returning`. Every other field — the access hints,
`build_limit`, the freshness token — is unprobed, and each answer costs a full
run." Still true, still open, and this makes the pairs that exist stronger
rather than adding one.

**The harness change is about reporting, not about correctness.** A check
group that raises is a failing check now, which is what `mutate.py` needs to
score it — but the exception's message is all the diagnosis a reader gets, and
the traceback is gone. `python3 -X dev` and running the group directly is the
way back to it.
