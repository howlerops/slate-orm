# Go and TypeScript now prove their disjunction builders against a real node, and the conformance runner asks for one un-negated. Chasing the caveat found a third wrong claim in the same entry that withdrew the first two.

- **Date:** 2026-09-25
- **Author:** Claude Code, working the open-caveat backlog
- **Touches:** `clients/go/slate/disjunction_test.go`, `clients/typescript/test/disjunction.test.ts`, `examples/explorer/conformance/conformance.py`, `clients/python/tests/test_disjunction.py`, `clients/python/src/slate/schema.py`, a correction in `ledger/2026-09-25-the-clients-could-always-send-a-disjunction.md`
- **Kind:** feature (tests), and withdrawing a wrong claim

## What changed

Two new live suites, one per client, each sending a disjunction through a
real `slate-serverd` in a filter and in a `HAVING`, plus the identity cases
(`Or()` is `False`, `And()` is `True`). One new conformance case, `a
disjunction`, whose answer is neither empty nor everything.

And a correction: the caveat that sent me here said the conformance runner
had no case using `Or` or `or`. It has had one since 2026-09-13.

Two lint reds are also fixed, both mine and both from yesterday's and today's
commits rather than from this change — see **Two reds I had already pushed**
below.

## Why

`ledger/2026-09-25-the-clients-could-always-send-a-disjunction.md` withdrew
two claims that no client could send a disjunction, and recorded as the
accurate remainder that Go and TypeScript had the builders and no live test.
That remainder was half right. Its first half was the work; its second half
was another inference from a layer I had not opened, and going to close it is
what found that out.

The value of the two new suites is not that `Or` works — the conformance
runner has been proving that for twelve days. It is that a *client's own
suite* is where a client developer looks, and where a reader of that client
goes to answer "can I send this?". Neither Go's nor TypeScript's said
anything, which is how three people in a row — all of them me — concluded the
capability was absent.

## What I got wrong, and how (the third time)

The pattern is now measurable rather than anecdotal. Three claims, all of the
same shape:

1. "No client can send one." Written from inside `slate-sql`.
2. "The gRPC `Query` has no equivalent." Written from inside `slate-sql`.
3. "The conformance runner has no case that uses either." Written from inside
   `clients/python`, having just added a Python test — one directory away
   from `examples/explorer/conformance/conformance.py`, and I did not open
   it.

Each is a statement about a file I was *near* and had not read, phrased with
the confidence of a statement about the diff in front of me. The distance
shrank each time and the error did not. Whatever instinct says "I know what
is over there", it is not calibrated, and the cost of `grep` is four seconds.

The earlier entry asked readers to treat any caveat of mine describing a
layer I was not editing with extra suspicion. That advice was correct and I
did not take it on the very next caveat I wrote.

## Two reds I had already pushed

`scripts/check.sh` came back with `python-client-ty` and `python-client-ruff`
failing before this change touched either file:

- `clients/python/tests/test_disjunction.py` — three `int(row[n])` calls.
  `Row.__getitem__` returns the whole `PyValue` union, which includes `None`
  for a column outside a projection, so `int()` on it is an error. The
  package already has `conftest.as_int`, written for exactly this and
  documented as asserting rather than casting. I did not run `ty` before
  committing that file. The same pass removed `_ids`, a helper I wrote and
  never called.
- `clients/python/src/slate/schema.py` — `-> "Table"` under
  `from __future__ import annotations`, so the quotes do nothing and `RUF`'s
  `UP037` says so. From yesterday's view entry.

Neither is interesting in itself. Both are worth writing down because the
mechanism is the same one this entry is about: I ran the suite, which passed,
and did not run the check that reads the *types*. `CLAUDE.md` says to start
with `sh scripts/check.sh`; I started with `pytest`.

## Alternatives rejected

**Add a `MUST_DIFFER` pair for `a disjunction` against `a conjunction`.** I
wrote it, then checked whether it would fire, and it would not. The two cases
as written are complements over `year` — eight books against three — so an
adapter that read `or` as `and` returns *nothing* for the disjunction, which
differs from eight perfectly well. The pair would have passed with the bug
live: a check that cannot fail, which is worse than no check because it reads
as coverage.

Pairing on the *same* two arms would fix that and cost more than it buys: the
disjunction would have to be `year >= 1960 OR year < 1990`, a tautology whose
answer is the whole table, and which therefore cannot distinguish a working
`or` from a filter dropped on the floor. The runner's own charter for that
list says it is for request fields every client might drop, and a connective
is not one — three adapters disagreeing is the check, and it works. The
rejected pair and both reasons are written into the case's comment, because
the next person to notice the missing pair will have the same idea.

**Skip the conformance case, since `a negated disjunction` already covers the
arm.** It does cover the arm — the mutation below is caught by both. What the
new case adds is an answer that is neither empty nor the whole table under an
`or` with nothing wrapped around it, and it cost four lines.

**Test only Go, or only TypeScript.** The claim was about both, and the
Python file it mirrors already exists. Two suites that differ only in
language are cheap; a third round of "the accurate version of the claim is
smaller" is not.

**Put the `HAVING` disjunction in the conformance runner too.** The three
adapters expose `having` as `{minCount: number}`, not as an expression, so it
would mean widening the contract in three backends to carry one case that two
client suites now cover directly. Left alone; noted below.

## Evidence

**Suites**, all after the change, all by exit code and not by grepping for
`FAILED`:

- `sh scripts/check.sh`: **53 passed, all of them** (51 passed with 2 failing
  before the lint fixes above).
- Go `go test ./...` in `clients/go`: **ok**, exit 0 — the disjunction file's
  3 tests and the rest of the client's.
- TypeScript `npm test`: **192 tests, 192 pass, 0 fail**.
- Python `pytest -q` in `clients/python`: **331 passed**, unchanged in count
  by the `as_int` edit, which is the point of it.

Conformance:
**131 cases, the three SDKs agree on all of them, 131 passed 0 failed** — 130
before this change. `test_conformance.py`: 52 passed, 0 failed (54 with the
rejected pair in place, which is how its roster test confirmed the pair's case
names resolved — before I removed the pair for the better reason above).

**Mutations**, all via `scripts/mutate.py`, five run, five caught, no
survivors:

| mutation | caught by |
|---|---|
| `clients/go/slate/query.go`: `Or` builds `Expr_Conjunction` | `TestAClientSendsADisjunctionInAFilter`, `TestAClientSendsADisjunctionInAHaving` |
| `clients/go/slate/query.go`: empty `Or` returns `True()` | `TestOrWithNoPartsIsFalseAndAdmitsNothing` |
| `clients/typescript/src/query.ts`: `or` emits `conjunction` | `a client sends a disjunction in a filter`, `a client sends a disjunction in a having` |
| `clients/typescript/src/query.ts`: empty `or` is `alwaysTrue()` | `an empty or is false and an empty and is true` |
| `examples/explorer/backends/go/query.go`: the `or` arm returns `slate.And` | `a disjunction (app)`, `a negated disjunction (app)` — both report the adapters disagreeing |

Each replacement changes behaviour rather than spelling — a different proto
oneof variant, a different identity, a different builder — so none is the
equivalent-mutation class `CLAUDE.md` warns about.

The last one is also the demonstration that the withdrawn claim was wrong:
`a negated disjunction` catches the broken adapter, so it was exercising `or`
all along.

**What the fixtures are built to prove.** Six `docs` rows where `size > 20`
and `kind = "note"` overlap on exactly one and each admits one the other does
not, so the union and the intersection differ; both tests assert the arms
discriminate *before* comparing, so a fixture that stopped discriminating
fails loudly rather than passing vacuously. Counts note=3, memo=2, sheet=1,
so an ORed `HAVING` of `> 2` and `< 2` leaves memo out — without which the
assertion would hold against a server that ignored `HAVING` entirely.

## What this does not do

**No disjunction in a conformance `HAVING`.** The three adapters model
`having` as `{minCount: number}`; carrying an expression there means widening
the shared contract in all three, which is a bigger change than the gap
deserves while both new client suites test exactly that path against a real
server.

**The two new suites are one fixture each, not properties.** Six rows, chosen
so the arms differ. The union law holds for any data where they do, which the
tests check, but nothing generates.

**The remaining ~28 caveats I wrote today are still unaudited for this
class.** Three checked, three wrong. That is now a rate rather than an
anecdote, and it argues the sample is not unrepresentative — but I have not
read the rest, so I do not know.

**Nothing checked the other suites for the `as_int` class.** I fixed the
three calls `ty` named in the file I wrote yesterday. Whether other test files
in this package cast a `PyValue` where they should assert, I did not look —
`ty` is clean, so any that exist are ones it cannot see.

**Nothing stops the fourth one.** The error is a habit, not a missing check,
and no test in this repository can fail when I state something about a file I
did not open. The nearest thing to a guard is that the claims are written
where a tracker reads them back, which is what caught all three.
