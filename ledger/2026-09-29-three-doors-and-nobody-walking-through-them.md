# Three clients had a negation helper and no test used it

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `clients/go/slate/not_in_test.go`,
  `clients/python/tests/test_not_in.py`,
  `clients/typescript/test/notIn.test.ts`, `docs/orm-comparison.md`
- **Kind:** test

## What changed

Seven cases, two or three per client, exercising `Not(In(...))` against a real
head node. Go 2, Python 3, TypeScript 2. Plus the composition written down in
`docs/orm-comparison.md`, where the claim that started this lives.

## Why

`ledger/2026-09-18-not-in-was-refused-on-a-claim-that-does-not-hold.md` said:

> **No client can express it.** `notIn` is a front-end operator; the wire
> carries `Expr`, and the three SDKs build `Expr::In` with no negation helper.
> A client that wants this builds the `Not` itself, which is possible and
> undocumented.

Two sentences that contradict each other, in the same bullet, for eleven days.
The second is true: `slate.Not`, `slate.not_` (and `~`), and `not` have all
been exported the whole time, so `Not(In(...))` is the expression.

Re-reading it today
(`ledger/2026-09-29-three-could-be-anothers-counted.md`) withdrew the first
half and found the thing the caveat had not looked for. Grepping all three test
trees for any use of the negation helper returned **nothing**. Not one case, in
any language. Three doors and nobody walking through any of them — which is
exactly how a wrong claim about them survives: somebody reads the front end,
generalises, and nothing runs to disagree.

The same shape has now produced two wrong caveats in this repository. The
disjunction pair — *"no client could send one"*, also false, also about a
builder that existed — was withdrawn on 2026-09-25 and answered with three
tests, one per client. This is the second instalment and the files are
deliberately shaped like the first.

## Alternatives rejected

**Add a `notIn` helper to the three clients.** The obvious reading of "no
`notIn` convenience", and the wrong one. It is a second name for a composition
that already works, in three languages, and each would need its own tests,
its own place in the generated surface and its own line in the conformance
runner — for a caller who can write six more characters. The gap was never
expressibility; it was that nothing proved it and nothing said so.

**Document it and write no test.** Cheaper, and it is what the caveat literally
asks for ("undocumented"). Rejected because a documented composition nothing
executes is the same object as the claim being corrected: a statement about the
clients that nobody has run. `docs/correctness.md` opens by arguing this at
length.

**One test, in one client.** The conformance runner exists because one client
being right says nothing about the other two, and because two clients wrong the
same way is the failure mode nothing else catches. `NOT IN` goes through each
client's own expression builder and its own protobuf encoder, so three are
three different claims.

**Assert `NOT IN` against a hand-written list of ids.** Simpler to read and
weaker: it passes if `IN` drifts, and it says nothing about the relationship a
caller relies on. The oracle here is that `IN` and `NOT IN` are disjoint and
together cover every seeded row, read off the fixture rather than transcribed,
so a row added to the fixture is covered rather than silently outside the claim.

**Cover a nullable column too.** The complement property is *not* SQL's
`NOT IN`: SQL's is three-valued, so a null in the list makes the answer unknown
and admits nothing, and `Expr::In` has the same rule. These fixtures have no
nulls, which is why the complement holds at all. A nullable case is a different
test with a different oracle and is recorded below rather than half-written.

## Evidence

- `go test ./slate -run 'NotIn|NegationIsNot'`: **2 passed.** Whole Go suite:
  `ok github.com/howlerops/slate-orm/clients/go/slate 11.611s`.
- `pytest tests/test_not_in.py`: **3 passed.** Whole Python suite: **354
  passed in 113.92s**.
- `npm test` in `clients/typescript`: **206 pass, 0 fail**, the two new ones
  reported by name (`ok 111 - not in is the complement of in`,
  `ok 112 - negation is not ignored on a single comparison`).
- **The mutation that matters, against the real server**, recorded in
  `ledger/mutations/20260929T175858-crates-slate-server-src-convert-rs.json`:
  `convert.rs`'s `Node::Negation(inner) => Expr::Not(...)` rewritten to drop
  the `Expr::Not`, so the wire silently discards every negation. **Caught by
  both Go cases.** That is the point of the whole change — before today, that
  mutation would have passed every test in every client, in every language.
- The grep that found the gap, over `clients/python/tests`, `clients/go/slate`
  and `clients/typescript/test`, for `not_(`, `slate.Not(` and `not(`:
  **nothing**, before these files.
- `sh scripts/check.sh`: 87 passed, all of them.

## What this does not do

**No nullable column, so nothing here is SQL's `NOT IN`.** The complement
property these tests assert holds precisely because the fixtures have no nulls.
The three-valued behaviour — a null in the list admitting nothing — is covered
for the *kernel* in `docs/correctness.md` and is uncovered from a client, in
all three languages. That is the sharpest remaining edge and it is the one a
caller is most likely to be surprised by.

**It is `Not` over `In`, not `Not` over everything.** A negated join predicate,
a negated `contains`, a negated window condition: each goes through the same
`Node::Negation` arm, so the mutation above would have caught any of them — but
only `In` and a single comparison are exercised. The arm is proven live; its
generality is inferred.

**The conformance runner does not compare the three.** Each client is checked
against the server independently, which is what catches one client being wrong
and cannot catch two being wrong the same way. That is the exact objection this
repository raises against per-client assertions, and a `NOT IN` case in
`examples/explorer/conformance` is the answer; it is not written.

**No `notIn` helper, by choice, and nothing checks the choice stays deliberate.**
A fourth client, or a rewrite of one of these three, could ship a `notIn` that
diverges from `not(isIn(...))` and nothing would notice. `check_transport_door.py`
is the shape of guard that would, and this has none.
