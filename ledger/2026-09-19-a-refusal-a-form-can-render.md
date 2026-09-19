# A refusal a form can render

## What changed

`SlateError.violations`: every `CHECK` a refused row broke, typed, on the
exception a Python caller already catches. Each entry carries the constraint's
name, the column it is about and the sentence to show.

## Why

Three entries built toward this and none of them arrived at it. V1 made a check
carry a `column` and a `message`. V2 made the server report every failing check
rather than the first. V3 published them in the catalog and generated them into
three languages. And then, as the V3 entry admitted: *"nothing reads `check.N`
into a structured per-field error yet."*

So the whole point — a form putting a message beside a field — still required
the caller to parse it out of the status text, which is precisely the contract
V1 exists to avoid inventing. It lasts until somebody rewords a sentence.

## Overruling a comment, and not quite

`_details.py` deliberately did not surface `ErrorInfo.metadata`, with a reason:

> surfacing it means deciding what a client promises about keys that vary per
> variant.

That was right when every variant invented its own keys, and V1/V2 changed the
premise for exactly one of them: the check-violation keys are specified —
`violations` is a count, `check.N`/`column.N`/`message.N` are indexed from zero.

So the map is now parsed and **still not exposed raw**. `check_failures_of`
reads the one documented shape into typed values; no caller gets the
dictionary. The original objection is answered rather than overruled — a key
this client has no contract for still reaches nobody.

## Alternatives rejected

**Return the metadata dict.** One line, and it makes every variant's private
payload a public contract by accident. The next person to add a key to a
unique-violation would be changing an API without knowing it.

**Read the unindexed `check`/`column`.** They are there and they are the
*first* failure. This is the call that exists to return all of them.

**Walk the map for `check.` keys instead of counting.** Simpler, and wrong at
eleven failures: the metadata is string-keyed, so `check.10` sorts between
`check.1` and `check.2`. Reading `violations` and counting up makes the bug
unreachable rather than unlikely, and a mutation to the map walk is caught.

**Return the prefix when the count and the keys disagree.** A caller shown two
failures for a row that broke three fixes two fields, resubmits and is refused
again — the round-trip-per-field behaviour this feature exists to remove. It
returns nothing, so the disagreement is visible.

**Do all three clients.** Go and TypeScript have the same gap and are not done
here. Python has the richest error surface and doing one properly with a real
captured fixture beats three transcriptions — and the fixture is now emitted by
a Rust test the other two can use.

## Evidence

The fixture is a **real** `grpc-status-details-bin` for a three-check
violation, printed by the server's own encoder. `status.rs` gained an
`#[ignore]`d test that emits it, kept rather than deleted so the artefact can be
regenerated when the shape changes — the alternative is a fixture a client
encoded itself, which agrees with that client's idea of the wire format and
proves nothing. `test_details.py` opens by saying so.

Three failures, the third with neither column nor message, because
`discount_under_price` spans two columns. One failure, or three identical ones,
would not separate "reads the list" from "reads the first" or from "assumes
every failure has a column".

Seven mutations, all caught:

| mutation | caught by |
| --- | --- |
| only the first failure is read | `…comes_back_typed` |
| the map's key order is used | `…order_is_the_schemas_and_not_the_maps` |
| the count is ignored, the map walked | the same |
| a missing column becomes `""` | `…comes_back_typed` |
| a gap yields the prefix | `a_count_the_keys_do_not_match_yields_nothing` |
| any reason is treated as a check violation | `check_shaped_metadata_under_another_reason…` |
| the error never asks for them | `the_error_a_caller_catches_carries_them` |

The last two needed tests that did not exist, and both survived a first pass.
The real `BLOB` cannot show the reason guard working: it carries no
`violations` key, so a decoder missing the check falls to the same empty answer
by accident. Both now use a hand-encoded decoy — the technique this file
already uses for its type-URL check, with its own negative control so the
decoy's own validity is established rather than assumed.

21 tests in `test_details.py`. `ruff`, `ty` against a clean virtualenv, and
`cargo test -p slate-server --test status` all clean.

## What this does not do

**Go and TypeScript still parse nothing.** Their errors carry the reason token
and the status text, as before. The captured fixture is language-neutral and
the Rust emitter is committed, so the work is transcribing one decoder twice —
it is not done, and the three-client parity this repository usually holds to is
broken here until it is.

Nothing in the demo or the conformance corpus catches a check violation and
renders it. The seam is tested against a captured blob rather than a live
refusal, so what is proved is that the client reads what the server sends —
not, end to end, that a form works.

No client validates against the published checks before sending. That remains
what the design note declined, for the reason it gave.
