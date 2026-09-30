# A refused relation path reports its limit as a value, not as a substring of a sentence

- **Date:** 2026-09-30
- **Author:** Claude Opus 5
- **Touches:** `crates/slate-server`, the three clients, their detail tests
- **Kind:** feature

## What changed

`status::refused` — a handler's own refusal with a stable reason token and a
machine-readable payload, beside `from_kernel`, which only covers what the
kernel raises. The relation-depth refusal is its first caller: it now carries
`RELATION_DEPTH_EXCEEDED` with `limit` and `asked` in `ErrorInfo.metadata`.

All three clients surface the whole map: `error.details` in Python and
TypeScript, `Error.Details` in Go. `violations` stays what it is — the one
part of that map with a shape of its own, parsed rather than left as strings.

## Why

`ledger/2026-09-17-a-path-of-relationships-on-the-wire.md` recorded the gap as
a caveat: a refused path reports `max_relation_depth` in prose and
`INVALID_ARGUMENT` as a token, *"which is the string-matching this repository
otherwise avoids, and it is the reason the metadata map is worth surfacing
eventually"*. It judged the change cross-cutting and deferred it.

The audit in
`ledger/2026-09-30-thirty-more-deliberate-verdicts-and-two-stale-premises.md`
found the caveat's premise had moved. All three clients already decode
`ErrorInfo.metadata` for `CHECK_VIOLATION`, so the map is one they read and a
key is one they cannot yet see. What was a protocol change in September is a
field and three accessors now.

A caller that wants to shorten a path and retry needs the number. Reading it
out of the sentence is a contract nobody agreed to and one a reworded message
breaks silently.

## Alternatives rejected

**A typed `limit` on the exception**, rather than the whole map. Cheaper for
this one refusal and wrong for the next: every future `status::refused` would
want its own field, and the caveat asked for the map. `violations` is the case
for a typed accessor — a shape with indexed keys and optional fields — and a
flat `{limit, asked}` is not that.

**Putting the numbers only in the trailers**, as `slate-leader` does for
`NOT_LEADER`. That trailer predates the details blob and is kept for
compatibility; adding a second mechanism for new payloads would mean two
places to look. The blob is where `reason` and `violations` already are.

**One decoder returning `(reason, details, violations)`** instead of three
passes over the same bytes. Rejected for the reason the second pass was:
most failures carry no `ErrorInfo` at all, so every failure would pay for the
walk, and a failure path is not where microseconds are won. Three cheap
decodes of a usually-absent blob beat one tuple every caller unpacks.

## Evidence

`cargo test -p slate-server --test related_path`: 13 passed. The depth test
now reads the `ErrorInfo` the way a foreign client would — parse the details
as a `google.rpc.Status`, find the `Any`, check its type URL — rather than
matching `message()`, which would be the same contract written down as if it
were one.

Each client has a case over the **same** fixture, a real blob from
`cargo test -p slate-server --test status -- --ignored --nocapture
emit_a_relation_depth_blob`. It is a *flat* map, unlike the check-violation
fixture's indexed one, so it tests reading keys the decoder was not written
against: `clients/python/tests/test_details.py`, 28 passed;
`clients/go/slate` `go test ./slate`, ok; `npm test`, 208 passed.

Five mutations over four runs. Four were caught first time; the fifth survived, which is the row below that matters:

| mutation | caught by | run |
| --- | --- | --- |
| `limit` reports the asked-for depth instead | `a_path_deeper_than_the_limit_is_refused` | `ledger/mutations/20260930T031327-crates-slate-server-src-service-rs.json` |
| the reason token is misspelled | same | same run |
| the refusal goes back to a bare `Status` with no details | same | `ledger/mutations/20260930T031355-crates-slate-server-src-service-rs.json` |
| `details_of` returns the reason instead of the map | `a_refusals_numbers_come_back_without_parsing_prose` | `ledger/mutations/20260930T031418-clients-python-src-slate-details-py.json` |
| `details_of` reads a detail of any type | **survived**; then `a_detail_of_another_type_yields_no_metadata` | same run, then `ledger/mutations/20260930T031503-clients-python-src-slate-details-py.json` |

A fifth was run and was not a change: the first attempt at "goes back to a
bare `Status`" replaced only the call's head, which does not compile, and
`mutate.py` reported `NOTHING RAN` rather than a survivor. Re-run against the
whole call, it is the third row above.

**The survivor was a real missing test**, and the shape of it is worth
recording. `reason_of` had a decoy case and a negative control for it; the new
`details_of` had neither, because the existing decoy carried a reason and no
metadata pair — so it read `{}` whether or not the type check was there, and a
decoy case written against it would have passed for the wrong reason. The
decoy now takes an optional `metadata` entry, and both the case and its
control are there.

## What this does not do

**One refusal uses it.** `status::refused` is general and every other
hand-built `Status::new` in `service.rs` still carries prose alone. Nothing
sweeps for them, and a caller cannot tell from the outside which refusals have
a payload except by asking for one and finding `{}`.

**No client validates keys against the reason.** `details["limit"]` on a
`CHECK_VIOLATION` returns nothing, silently, rather than saying the caller has
read the wrong failure's map. The docstrings say to branch on `reason` first;
nothing enforces it.

**`asked` is redundant** — the caller knows how many steps it sent. It is
there because a caller that built the path from data does not necessarily have
the count to hand at the point it catches the error, and one key costs
nothing. That is a judgement, not a measurement.

**The conformance runner has no case for it.** The three clients are tested
against the same fixture, which is stronger than three fixtures, but it is
still a decoder test rather than a live refusal through a real server in all
three languages.
