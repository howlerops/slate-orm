# The invariant three fixes got wrong, as a check instead of a habit

- **Date:** 2026-09-20
- **Author:** Claude, after the third commit in a row fixing one path of several
- **Touches:** `scripts/check_handlers.py` (new), `scripts/test_check_handlers.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`, `docs/security-review.md`
- **Kind:** process

## What changed

`scripts/check_handlers.py` fails if a handler in `slate-server`'s `service.rs`
resolves a table with the bare resolver, or fingerprints one without an
authorisation above it. Two exemptions, each with a reason. `check.sh` goes 23
to 25 checks. Also a miscount, corrected: thirteen `fingerprint::check` sites,
not fourteen.

## Why

Three commits in a row today fixed a security finding whose earlier fix had
covered one path of several:

| finding | fixed in | still open in |
| --- | --- | --- |
| 1, cross-tenant cascade | `Catalog::from_tables` | `Catalog::insert` |
| 6, duplicated identity header | `MetadataIdentity` | `TokenIdentity` |
| 8, schema disclosure | the handlers that fingerprint | `query`, `explain`, `related` |

Each fix was right for the path its finding was written about. Each time the
remaining paths were judged equivalent by reading, and each time the reading
was wrong. The third one gave up a table's exact column count, in one request,
to a caller granted nothing.

The pattern is not carelessness, it is structural: **a finding names the path
where it was found, and the fix silently inherits that scope.** Nothing in the
codebase states the invariant the fix was serving, so nothing can notice a new
path that violates it. `service.rs` has thirteen `fingerprint::check` calls
where the finding was written about four, and every one of them is correct
today because somebody looked — which is exactly the arrangement this
repository has a name for.

So the invariant is written down as a check. Two rules:

1. A bare `self.table(..)` — the resolver that does not authorise — is either
   inside `authorized_table` or in `UNAUTHORIZED` with a reason.
2. A `fingerprint::check` has an `authorized_table` within four lines above it.

Adding a handler that breaks either fails the check until somebody chooses,
which is `EXPECTED_REFUSALS`, `MUST_DIFFER` and `test_check_sh.py`'s
`ELSEWHERE` again: a list you are forced to edit is a list that stays true.

## Alternatives rejected

**Make the bare resolver private, or delete it.** The structural fix — if the
only way to get a `&TableDef` authorises, no handler can get it wrong. It
cannot be done: `authorized_table` is built *on* the bare one, and
`resolve_relation` genuinely needs a table before there is an action to check
against. Both would have to stay, so the question is only whether anything
notices a third.

**A `clippy` lint.** The right tool, and writing a `dylint` driver for one
crate's internal convention is a build-system change and a second toolchain on
a container that cannot fit `cargo test --workspace`. A forty-line script in
the language the other four guards are written in costs a tenth as much.

**Parse the Rust rather than match lines.** `syn` would let the check know that
an authorisation and a fingerprint are in the same function and about the same
table, which the line-distance heuristic only approximates. Rejected for now
because the approximation is calibrated against a real file — four lines is
more than every current site needs and less than any two adjacent handlers —
and because a precise check nobody can read is worse than a blunt one with its
threshold argued in a comment. If it produces a false positive, that is the
moment to reach for a parser.

**Write it as a Rust test in `slate-server`.** Closer to what it checks and it
would run with `cargo test`, which is a suite this container struggles to run
whole. The Python guards run in `scripts/check.sh` in milliseconds with no
toolchain, which is what makes anyone run them before committing.

**Nothing, and rely on the security review.** The review now describes all
three fixes accurately. A document is not a check — that is the lesson of this
morning's other entry, where the same review cited four tests that had not
existed for weeks.

## Evidence

**Against the real file**: 2 bare resolutions, both accounted for; 13
fingerprint checks, all authorised first.

**Five tests, over files the test writes** rather than against `service.rs`,
where a check that had stopped checking would pass for as long as `service.rs`
stayed correct:

```
ok    a handler that authorises before fingerprinting passes
ok    a handler using the bare resolver fails and names itself
ok    a fingerprint with no authorisation above it fails
ok    an authorisation too far above the fingerprint does not count
ok    an exemption for a function that no longer exists fails
```

The last is the one most guards leave out. A stale exemption reads as a live
hazard somebody accepted, so the next person weighs a decision nobody is
making.

**Mutation-tested through `scripts/mutate.py`**, five mutations, each caught by
the test written for it:

```
ok  the bare-resolver check is dropped        -> a handler using the bare resolver fails and names itself
ok  the fingerprint ordering check is dropped -> a fingerprint with no authorisation above it fails, ...
ok  the reach becomes unlimited               -> an authorisation too far above the fingerprint does not count
ok  a stale exemption is no longer reported   -> an exemption for a function that no longer exists fails
ok  a finding no longer exits non-zero        -> (all four)
```

**A correction to the previous entry and to the review.** Both said "fourteen"
`fingerprint::check` sites. There are thirteen; I counted a grep's output by
eye rather than with `-c`, which is how the number got into a commit message
and a document in the same hour. The review is fixed. The entry stands as
written with this correction beside it, per `ledger/README.md`: an entry is a
record of what was known when, and rewriting it would hide that the number was
wrong rather than that it changed.

`scripts/check.sh` 25/25; `scripts/test_check_sh.py` accounts for the two new
CI steps; `ruff` and `ty` clean over both files — the latter after a real
failure, since `ty` is stricter about a heterogeneous list literal than the
local `ruff` is.

## What this does not do

**It is two greps, and it says so.** A handler can take a `&TableDef` from one
of the two accounted sites and pass it anywhere; a handler can authorise one
table and fingerprint another more than four lines later. Neither is caught.
What is caught is a *new* unauthorised resolution, which is the specific thing
that happened three times.

**It covers one file.** `service.rs` is where the handlers are, and the check
hard-codes it. A second service, or handlers moved to a module, would be
outside it silently — the check would keep passing over a file with nothing in
it, which is the "a check that never fires" failure in miniature. Nothing
asserts the file is non-empty or still the right one.

**It does not generalise the lesson.** The table above is three instances of
"the fix inherited the finding's scope", and this guards one of them. The
catalog constructor and the `Authenticator` implementations have no equivalent
check; both are now correct, and both would go wrong the same way. A general
answer — something that asks "what else implements this trait" or "what else
constructs this type" — is not attempted here and I do not know its shape.

**The four-line reach is calibrated, not derived.** Every current site has the
authorisation adjacent or one line away, and I chose four to leave room without
reaching the next handler. A site that legitimately needs five fails, and the
fix is to widen the constant deliberately — which is the intended behaviour and
also, honestly, an untested claim about a situation that has not arisen.
