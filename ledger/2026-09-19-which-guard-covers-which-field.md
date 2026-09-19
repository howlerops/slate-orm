# Which guard covers which field

## What changed

Two things, one story.

`include_deleted` on a join input is now tested — it was designed for that and
nothing checked it. And `MUST_DIFFER` gained a `returning` pair, after a probe
showed `returning` has the blind spot and, **contrary to what I wrote an hour
ago, `paged` does not**.

## Why

Both halves are a claim I made and had not checked.

The `include_deleted` entry asserted the flag works on a join input — that is
why it lives on the shared base of the query builders — and tested it nowhere.
The same entry listed `paged` as having the conformance suite's blind spot, as
a worked example, on reasoning alone.

An untested design claim and an unprobed example are the two kinds of thing
that get repeated until someone depends on them. One turned out true and one
turned out false.

## The correction

The `include_deleted` entry said:

> `MUST_DIFFER` holds one pair. Several other flags have the same weakness —
> `paged`, and any hint — and nothing checks them.

The first half is right and the example is wrong. Dropping `paged` in all three
clients does not produce a green run: it breaks three cases in
`EXPECTED_REFUSALS`, because the server *refuses* a paged read with no limit, a
projection that drops a key column, or a sort the key does not give. Without
the flag those refusals become answers, and the runner says the list is stale.

So the suite has **two** guards against a unanimously-dropped field, not one,
and the second was already there. That narrows the blind spot considerably: it
is only a field that changes an *answer*, with no refusal depending on it and
no pair comparing it.

`returning` is exactly that, and the probe is unambiguous: every `Returning` in
all three clients set false — twelve call sites — and the run was **96 cases,
the three SDKs agree on all of them**. RETURNING could stop working everywhere
and CI would be green.

The two cases needed to catch it were already in the corpus, adjacent, their
comments describing each other. Nothing compared them.

## A probe that lied first

The first `returning` attempt reported "caught". It was wrong: I had mutated
one Go call site of six, so Go still asked for rows and the disagreement was my
own half-applied edit rather than a guard. Counting the sites first and using a
regex over all of them turned "caught" into "survived".

Worth recording because a mutation that is *partially* applied is the one way
mutation testing lies in the reassuring direction, and the symptom — a
disagreement rather than a silent pass — looks exactly like success.

## The join input

`include_deleted` sits on the shared base of the query builders specifically so
a join input can set it, and that claim was untested. It could have been false:
a join input goes through a different conversion from a plain read, and that
conversion *refuses* several fields it considers meaningless on an input.

It works. The proof is four self-joins over four rows, two retired, `kind`
unique per row so each visible row pairs with itself:

| input 0 | input 1 | rows |
| --- | --- | --- |
| live only | live only | 2 |
| all | all | 4 |
| all | live only | 2 |
| live only | all | 2 |

The asymmetric pair is the evidence. If the flag were honoured once for the
whole join rather than per input, those two would not both be 2.

## Alternatives rejected

**Add pairs for every flag on the protocol.** The instinct after finding one
hole, and it would add noise: most fields are covered by a refusal case, and a
pair for those asserts something already asserted. The runner now documents the
three possibilities — refusal case, pair, or nothing — so the next person adds
the right one.

**Assume, rather than probe.** That is what produced the wrong `paged` claim.
Each probe costs one four-minute conformance run, which is cheap against
writing a guard for a hole that is not there.

**A join input test in the kernel instead of over the wire.** Cheaper, and it
would have missed the thing most likely to break: the *conversion* for a join
input is separate code with its own refusals. The mutation that proves this —
`refuse_unused` rejecting `include_deleted` — is caught only by a wire test.

## Evidence

Six mutations, all caught:

| mutation | caught by |
| --- | --- |
| a join input's `include_deleted` is refused as meaningless | the four-way self-join |
| the flag is dropped inbound | the same |
| the grant is never checked | `…needs_the_grant_too` |
| `paged` dropped in all three clients | `EXPECTED_REFUSALS` (pre-existing) |
| `returning` dropped at all twelve sites | the new pair |
| `include_deleted` dropped in all three | the existing pair |

`./run.sh --conformance`: 96 cases agreeing. `cargo test -p slate-server --test
purge_wire`: 9 passing. `ruff` clean.

## What this does not do

The probe covered `paged` and `returning`. Every other field — the access
hints, `build_limit`, the freshness token — is unprobed, and the honest
position is that I do not know which guard covers them. The runner says how to
find out; I did not do it for all of them, because each answer costs a full run
and the two I picked were the two I had claimed something about.

`MUST_DIFFER` still compares whole answers. Two cases that differ for a reason
unrelated to the field under test would satisfy it, and nothing checks that the
difference is the *right* difference.

The join tests use a self-join on one table, because the fixture has exactly
one that soft-deletes. A join between two different soft-deleting tables — where
each side's flag could be crossed with the other's — is untested.
