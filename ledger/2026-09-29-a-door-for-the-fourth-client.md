# A door for the fourth client, and a rule that reads its own roster

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/{check_transport_door.py,test_check_transport_door.py,check.sh,check_closed_caveats.py}`, `.github/workflows/ci.yml`, `docs/caveat-status.json`
- **Kind:** guard

## What changed

`scripts/check_transport_door.py`: every client under `clients/` must offer a
way to hand it a configured gRPC transport, and something must walk through it.
Nine cases in `test_check_transport_door.py`, both in `check.sh` and in CI.

The roster is **read off the filesystem**. A fourth client is a fourth
directory with a manifest in it, and one with no `DOORS` entry is an error.

## Why

Two entries recorded the same gap, a day apart:

> **Nothing checks that a new client transport keeps this door.**
> — `ledger/2026-09-28-the-third-client-counts-and-the-go-instrument-was-half-blind.md`

> **Nothing still checks that a fourth client arrives with a transport door.**
> The `grpc` re-export is one more way to reach a channel and not a guard: the
> three clients now expose it three ways and no check fails if a fourth
> arrives exposing none.
> — `ledger/2026-09-29-the-three-counts-compared-and-a-door-only-one-client-had.md`

Both are the same fact written from either side of one day's work. The
TypeScript client shipped for months with `credentials` and no `options`, so a
caller could configure the transport in two languages of three, and the way
that was found was needing an interceptor and having nowhere to put it. Nothing
made the third client's absence visible and nothing would make a fourth's.

A *transport door* is what the three now have: Python's `Client(channel=)`,
Go's `Dial(..., ...grpc.DialOption)`, TypeScript's `Client.connect(...,
options)` plus the grpc-js re-export npm makes necessary. Without one a caller
cannot instrument, cannot bring TLS, cannot set a keepalive or a message-size
limit — and cannot find out, because the client compiles and works.

## Alternatives rejected

**A hand-written roster of three clients.** The obvious shape, and the one this
rule would be useless as. It would print `3 clients, all with doors` on the day
a fourth arrived without one — the never-fires shape `caveats.py`'s own
docstring is about, where a check that cannot see a thing reports the same
clean as one that saw it and found nothing. Reading `clients/` costs eight
lines and is the entire value of the file.

**Matching a parameter *name*.** `options`, `opts`, `channel` — a name-based
rule is a proxy for the hazard rather than the hazard, the way
`check_client_identity.py` argues against flagging `token` and `secret`. The
patterns here are the parameter's *declaration*, type included, so changing
`grpc.ChannelOptions` to something that cannot carry an interceptor is caught
as well as deleting the argument.

**Checking only that the door is declared.** This was the first draft and it is
worth a paragraph, because it would have passed on the TypeScript client for
every one of the months it was broken: `credentials` was there, typed, and
documented, and no interceptor could be installed. A door nothing walks through
is a parameter with a promising name. So each entry also names the demo adapter
that configures a transport through it — all three install the round-trip
counter the conformance runner compares — and that use is checked too.

The cost is a coupling: an adapter rewritten away from interceptors turns this
red for a reason that is not about clients. That is the right red. The
demonstration is the only thing distinguishing this rule from a spell-check on
three signatures, and if it goes away the rule should say so rather than keep
passing.

**A compile-time check in each language instead.** A Go test that installs an
interceptor, a `tsc` assertion, a Python one. Stronger per client and
impossible for the case that matters: none of them runs for a language nobody
has written yet, which is exactly the fourth client.

## Evidence

- `python3 scripts/check_transport_door.py`: **ok, 3 clients**.
- `python3 scripts/test_check_transport_door.py`: **9 passed, 0 failed.**
- **Eight mutations across two runs.** Seven caught on the first attempt
  (`ledger/mutations/20260929T032704-scripts-check-transport-door-py.json`,
  outcome `problems`); the eighth survived and is caught now
  (`ledger/mutations/20260929T032725-scripts-check-transport-door-py.json`).
  Each caught mutation was matched to a *named* case: reading the roster from
  `DOORS` rather than the filesystem broke "a fourth client with no door fails
  and names it", skipping an unclassifiable directory broke its own case, and
  so on through the re-export and the empty-`clients/` never-fires arm.
- **The survivor was a real missing test.** Making `missing()` return `None`
  for a file that does not exist changed nothing, because all eight cases
  *rewrite* a file and none deleted one — so a renamed `client.go` would have
  read as a door in place. `entry_file_gone` is that case, and the mutation
  fails against it now.
- `python3 scripts/test_check_sh.py`: 113 steps accounted for, 4 passed.
- `python3 scripts/check_closed_caveats.py`: 378 of 413 closed caveats
  witnessed.

## What this does not do

**It does not check that an interceptor installed through a door actually
runs.** That is the conformance runner's round-trip case, which compares three
counts and would disagree if one adapter's counter were silently inert. This
rule checks the door exists and is used; the counts check the use works. Two
files, and neither says so in the other — the connection is here and nowhere
else.

**A fourth client can satisfy it with a door and a fake walker.** `through` is
a regex over the adapter's source, so a client could pass by adding a line that
matches. The rule makes the omission loud, not impossible, which is the most a
grep-shaped guard can do and is the same bargain every other guard in
`scripts/` strikes.

**The three patterns are three hand-written regexes.** They encode today's
spelling of each signature, and a refactor that renames `Dial` will turn this
red for the right reason with the wrong message. `MANIFESTS` has the same
shape: a fifth language brings a fifth manifest name, and until somebody adds
it the directory reports as unclassifiable rather than as a client — loud, and
still a list maintained by hand.

**Nothing checks the adapters are the only callers.** If the demo adapters were
deleted the rule would go red even though the clients were fine. Pointing it at
a test inside each client instead would be more robust and would not exist for
a language nobody has written, which is the trade the "compile-time check"
paragraph above makes.
