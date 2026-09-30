# A getter that advances answered about the next row, and the full-join hypothesis was wrong

- **Date:** 2026-09-28
- **Author:** Claude, continuing the pass over what the caveat tracker still listed
- **Touches:** `clients/go/slate/client.go`, `clients/go/slate/scalar_test.go`
- **Kind:** fix

## What changed

`RowStream.Row`, `JoinStream.Row` and `GroupStream.Group` no longer advance the
cursor. `Next` does, which is the method whose name says so. The getters are
idempotent and `Computed` / `InputComputed` read the same value either side of
them.

And `TestBothUnmatchedSidesOfAFullJoinComputeNothing`: a full outer join over
the library fixture, which has an author with no books *and* a book with no
author, so both unmatched directions occur and each must compute nothing.

## Why

Two things, and the order matters because the second is why the first exists.

**The residual's hypothesis was wrong, and this disproves it.**
`ledger/2026-09-27-the-unmatched-side-computed-nothing-and-nothing-said-so.md`
recorded: *"a server that filled an unmatched input differently for a full
outer join — an empty row rather than no row — would pass both tests and be
wrong in the clients."* It does not. Printed row by row, a full join gives
`a=[3 cy UK] b=[] | ca=[30] cb=[]` and `a=[] b=[14 9 orphan 2020] | ca=[]
cb=[2020]`: each unmatched side is absent and computes nothing, in both
directions. The new test pins it; the hypothesis is withdrawn.

**Writing that test found the real defect.** `Row` did the `at++`, so reading a
computed value *after* it answered about the next row — silently, with a
plausible number. The first version of this test read `Row` first and produced:

```text
a=[1 ada UK] b=[10 1 a-one 2001] | cb=[1990]    (2001 -> 2000 is right)
a=[3 cy UK] b=[]                 | cb=[2020]    (no book, and a value)
```

That second line is the shape the residual was worried about, produced by the
*client* rather than the server. Only `RowStream.Row` documented the ordering
requirement; `JoinStream.Row` and `GroupStream.Group` did not, and
`JoinStream.Row`'s comment said the row was the one "Next advanced to", which
reads as a promise that it does not advance further.

An API where two getters must be called in an undocumented order, and give
plausible wrong answers otherwise, is a defect whether or not it has bitten
anyone. It bit within a minute of being met.

## Alternatives rejected

**Document the order on the two getters that lacked it.** One line each, no
behaviour change, no risk to any caller. Rejected because the failure is
silent: the reader who needs the warning is the one not reading the doc, and
the wrong answer is a number of the right type in the right range. The third
getter *had* the warning and the mistake was still made.

**Make the getters return `(value, ok)` or panic after a second call.** Would
turn the silent wrong answer into a loud one without changing the cursor.
Rejected as a bigger break than the fix: every call site changes, for a
worse outcome than simply making the second call correct.

**Leave `Collect` as the supported path and deprecate the getters.** `Collect`
is already correct and already what most callers use. But it materialises the
whole result, which is the thing streaming exists to avoid, and the getters are
how a computed value is read at all — `Collect` does not return them.

## Evidence

The behaviour was **printed, not reasoned about**: a scratch test dumped every
row of four join shapes with both orders of the two calls. That is what
separated "the server sends an empty row" from "the client reads the next
row's value", and the two look identical from an assertion.

**Five mutations, all caught** —
`ledger/mutations/20260928T191941-clients-go-slate-client-go.json` and
`ledger/mutations/20260928T192244-clients-go-slate-client-go.json`:

| mutation | caught by |
|---|---|
| `JoinStream.Row` advances again, as it used to | 4 tests |
| `RowStream.Row` advances again, as it used to | 4 tests |
| the join cursor advances two rows per `Next` | 4 tests |
| the row cursor advances two rows per `Next` | 4 tests |
| `Next` never primes, so it never advances | **unreadable** |

The last is worth naming rather than hiding: removing the prime makes `Next`
loop on row 0 for ever, the suite never finishes, and `mutate.py` reports
`UNREADABLE — the command exited 1, something reported, and no failing test was
named`. A hang is not a failure it can score. It was replaced by the
`+= 2` mutation above, which terminates and is caught.

`TestOnlyNextMovesTheCursor` is the test the fix needed: `Row` twice returns
the same row, the computed value matches on both sides of it, and the row
*count* is right — a cursor advanced twice per iteration returns three of five
books, which the first two mutations show.

`go test ./slate` — the whole suite green, 12s. `gofmt -l` clean.

## What this does not do

**The other two clients cannot have this defect, and that is the design
lesson.** They were checked rather than assumed: TypeScript's `withComputed`
iterators yield a `ComputedRow` / `ComputedJoinedRow` carrying `values` and
`computed` together, and Python's `Row` holds its own computed values while a
`JoinedRow` holds one `Row` per input. Neither exposes a cursor two getters
can disagree about, so there is no order to get wrong. Go alone split the row
and its computed values across two calls against a shared cursor — the fix
makes that split safe rather than removing it, because removing it is a
different API.

**Nothing forbids a future getter from advancing again.** The three are fixed
and tested; a fourth stream type added later starts from whatever its author
copies. A guard would have to read the client's source for `at++` inside a
method that is not `Next`, which is a grep with a false-positive problem and
is not written.

**`GroupStream.Group` is fixed but has no idempotence test of its own.**
`TestOnlyNextMovesTheCursor` covers the row and join streams, where a computed
value made the wrongness visible; a group has no second accessor to disagree
with, so the only assertion available is that `Group` twice is the same group.
It is fixed for consistency and covered only by the suite continuing to pass.
