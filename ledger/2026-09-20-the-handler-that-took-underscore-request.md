# One RPC of nineteen answered anybody, and `deny-all` promised otherwise

- **Date:** 2026-09-20
- **Author:** Claude, working the review's own list of what it did not examine
- **Touches:** `crates/slate-server/src/service.rs`, `crates/slate-server/tests/security_probe.rs`, `crates/slate-server/tests/common/mod.rs`, `scripts/check_handlers.py`, `scripts/test_check_handlers.py`, `docs/security-review.md`
- **Kind:** security

## What changed

`leadership` authenticates. It took `_request` — it never looked at the
metadata — so it answered any caller who could open a socket, including under
the one configuration whose whole statement is that it refuses everybody. Rule
5 in `check_handlers.py` makes the next such handler a failing check.

## Why

`docs/security-review.md` ends with a paragraph naming four areas it did not
examine. Working the first of them — the lease and leadership protocol — the
interesting question turned out not to be the protocol but its RPC: nineteen
handlers, eighteen of which begin

```rust
let context = self.context(&request)?;
```

and one of which does not. Demonstrated against a head node built with
`DenyEveryone`, which is what `slate-serverd`'s `mode = "deny-all"` installs:

```
query      -> Unauthenticated
leadership -> standing: Leader, generation: Some(1), holder: "test-leader"
```

The banner that mode prints at startup reads *"this node authenticates nobody
and will refuse every request"*. That sentence was false. It is also the
strongest statement the configuration language can make, which is what makes
this worth more than the three fields it gives up.

Those fields are not nothing. `holder` is described in the proto as something
"a client can use to find the node that will accept its writes" — as useful to
a scanner picking the write leader out of a set of identical endpoints.
`generation` counts lease changes, so polling it reports instability nobody
chose to publish. `stepped_down_because` is free text written for an operator.

There was no comment, no test and no stated reason. This is an omission, not a
decision that turned out badly, and the difference matters for what to do about
it: a decision would need re-arguing, an omission needs closing and a check.

## Alternatives rejected

**Keep it open and narrow the response** — standing only, no holder, no
generation. Preserves an unauthenticated liveness probe. Rejected because the
useful half *is* the holder: a client asks this to find where to write, and a
response that omits it makes the RPC pointless while still answering a
stranger. It also leaves `deny-all`'s promise false, which was the sharper half
of the finding.

**Keep it open because a load balancer needs it.** The deployment answer is
`/metrics`, which `slate-serverd` already serves over plain HTTP on its own
port for exactly this. A gRPC call that reports which node holds the write
lease is not a health check; it is topology.

**Require a grant as well.** There is no table, so there is nothing for
`authorized_table` to check and no action that fits. Inventing an
`Action::Leadership` would mean a new variant, a configuration surface for it,
and every existing deployment's roles silently losing the call. Authentication
is the right bar: this is "who may talk to this server", not "who may read
this table".

**Fix it and not build rule 5.** The whole shape of today — five findings each
fixed on one path of several — argues the other way. And unlike the
`Catalog` constructor guard I withdrew this morning, this one has no structural
enforcement standing in for it: nothing in the type system says a tonic service
method must authenticate, and eighteen of nineteen doing so is a convention.

**A roster of handler names.** The `AUTHENTICATORS` idiom, and wrong here:
handlers are added often, and a list you must edit for every ordinary addition
is a list people edit without thinking. The signature is derivable.

## Evidence

**Before the fix**, `leadership_answers_a_caller_that_deny_all_refuses` passed
while asserting the standing, a present generation and a non-empty holder, with
`query` refused as `Unauthenticated` beside it as the control. It is now
`leadership_is_refused_to_a_caller_that_deny_all_refuses` and asserts the
refusal, and `an_authenticated_caller_still_learns_who_holds_the_lease` is the
other half — without it, deleting the handler's body would satisfy the probe.

Two mutations against the fix, both caught:

```
ok  leadership stops authenticating   -> leadership_is_refused_to_a_caller_that_deny_all_refuses
ok  leadership answers nothing at all -> an_authenticated_caller_still_learns_who_holds_the_lease
```

**Rule 5 derives handlers by signature**, and the criterion is the hazard
rather than a proxy for it: a method taking a `Request<pb::..>` is reachable
from the wire by definition. On the real tree that is exactly nineteen names,
the whole tonic service and nothing else. This is the lesson the converter rule
cost 47 and then 59 false positives to learn this morning, applied first time
here.

Four new cases in `test_check_handlers.py`, 23 in total, all passing — an
unauthenticated handler, one that authenticates with no table to authorise, an
internal helper that is not a handler, and a tree with no handler at all. Four
mutations, all caught:

```
ok  rule 5 is never called                     -> an RPC handler that never authenticates fails
ok  an unauthenticated handler is not reported -> an RPC handler that never authenticates fails
ok  any async method counts as a wire handler  -> four cases
ok  a tree with no wire handler is a pass      -> a tree with no wire handler at all fails
```

**No client notices.** All three SDKs call this through an authenticated client
(`c.identity.apply(ctx)` in Go, the same in Python and TypeScript) and already
send their credentials. That is a reading of three call sites; the assertion
about the *server* is the authenticated-caller test.

`cargo test -p slate-server`: no failures. `cargo clippy --workspace
--all-targets`: zero diagnostics. `scripts/check.sh`: 27/27, now reporting
`19 wire handlers all authenticating`.

## What this does not do

**It checks that a handler authenticates, not that it authenticates first.**
Rules 1 to 3 are about ordering — the check must precede the use — and this one
is about presence, deliberately: a handler that calls `self.context` anywhere
in its body has established who is asking. A handler that did something
observable *before* authenticating would pass rule 5 and be a finding-8-shaped
hole. Rules 1 to 3 cover that for the table-resolving channels; a new channel
would not be covered by any of them.

**The lease protocol itself is still not attacked.** Fencing, generation
monotonicity, and what two nodes that both believe they lead can do to each
other are judged by `tests/lease.rs` and `tests/leadership.rs` — 1,390 lines of
correctness testing — and not by an adversary. The review's row is struck
through for the RPC surface only, and the entry in the review says so. What is
reachable from the wire is one call; the protocol between a node and its object
store is not, and I did not examine it.

~~**`stepped_down_because` is free text and I did not audit what can end up in
it.** It is now behind authentication, so the question is what one authenticated
tenant learns about the node's internals rather than what a stranger does — a
smaller question, and still one I have not answered.~~

> **Audited, same day: it is not free text.** The field is filled from
> `StepDown::reason`, a `const fn` over a fieldless four-variant enum returning
> one of four `&'static str` literals — "fenced by another writer", "the lease
> was taken by another node", "resigned", "the storage backing the lease cannot
> support one". Nothing caller-supplied, nothing from the environment, no path,
> no address, no error text from the object store. The match is exhaustive and
> wildcard-free inside the defining crate, so a fifth variant cannot reach the
> wire without someone writing its string. I called it free text from its
> `String` type on the wire without reading where the string comes from, which
> is the same mistake as inferring a gap from a pattern.

~~**Two of the review's four areas remain**: the Python client, and the tuple
codec on adversarial encoded input beyond `slate-tuple/tests/untrusted.rs`.~~

> **Both done, later the same day.** The Python client produced finding 10 —
> `Identity.__repr__` printing the bearer token the server end redacts — and
> the tuple codec produced a coverage gap and no defect. All four rows of the
> review's list are now struck through.
