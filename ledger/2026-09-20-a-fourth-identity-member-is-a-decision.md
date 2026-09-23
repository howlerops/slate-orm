# Two clients cannot hold a credential today, and now have to say if that changes

- **Date:** 2026-09-20
- **Author:** Claude, closing the caveat the previous entry closed with
- **Touches:** `scripts/check_client_identity.py`, `scripts/test_check_client_identity.py`, `scripts/check.sh`
- **Kind:** security

## What changed

A check that no client's `Identity` declares a member beyond the caller's
principal, tenant and roles. No client code: Go and TypeScript are correct and
Python's `extra` is handled where it lives, by redacting its value.

## Why

The previous entry fixed security finding 10 — the Python client's `repr`
printing the bearer token in `Identity.extra` — and closed with this:

> **Go and TypeScript were read, not tested.** Their `Identity` types cannot
> carry a credential today. Nothing stops one gaining an `extra` field
> tomorrow, and there is no check across the three clients for this property.

That is the exact sentence five other findings this session were written under,
and in each case the second path arrived. Here the second and third paths are
two other languages, maintained separately, whose authors have every reason to
add the same escape hatch Python has — it is the only way to authenticate
against a deployment that is not using the shipped `MetadataIdentity`, and Go
and TypeScript callers currently have to write an interceptor instead.

So the rule is not "no client may carry a credential". It is: **a fourth member
of `Identity` is a decision, and somebody has to make it.** The cost of adding
one is an entry in `ALLOWED` with a reason it cannot hold a secret, or a
redacting formatter and a test like the Python one.

## Alternatives rejected

**Flag members whose names look sensitive** — `token`, `secret`, `auth`. The
proxy-versus-hazard mistake that cost 47 and then 59 false positives on the
converter rule this morning, and it fails on the very case that motivated the
rule: Python's is spelled `extra`. A new metadata slot is the hazard whatever
it is called.

**Forbid a fourth member outright.** Simpler, and wrong: `extra` is why the
Python client works against token-mode and mesh deployments, and a rule that
forbade the Go and TypeScript equivalents would be arguing against a feature
rather than against a formatting bug.

**Parse the languages properly** — `go/ast`, a TypeScript compiler API. Neither
toolchain is required to run `scripts/check.sh`, which is what makes that
script worth running before every commit; adding a dependency on `go` and
`tsc` to a lint that currently needs neither is a bad trade for two type
declarations.

**Check the formatters instead of the members.** The hazard is a formatter
printing a secret, so checking for `String()` on Go's `Identity` and
`toString()` on TypeScript's looks closer to it. It is closer and it is later:
a member that can hold a credential is the thing that makes a formatter
dangerous, and the member arrives first. The formatter case is kept as a
secondary rule that fires only when `ALLOWED` is non-empty — a formatter *and*
a member that can hold a secret is finding 10 exactly.

**Cover Python here too.** Its `Identity` takes `extra` by construction, so the
"no fourth member" rule would fail on it forever. Python's half is asserted
where it belongs, by `test_identity_repr.py`'s roster case, and this file names
that so the set of clients accounted for is visible rather than implied.

## Evidence

Eight cases in `test_check_client_identity.py`, over files the test writes, all
passing: the clean case, a fourth field in Go, a fourth member in TypeScript, a
member that disappeared, no `Identity` at all in either language, an unexported
Go field (not a member — it never reaches the wire), and a method on the struct
(not a member either).

Six mutations, all caught:

```
ok  an extra member is not reported                 -> two cases
ok  a departed member is not reported               -> a member that disappeared is reported
ok  an absent Identity is read as one with no members -> both never-fires cases
ok  the Go field pattern accepts an unexported field -> an unexported Go field is not a member
ok  only Go is checked, not TypeScript              -> two cases
```

**A seventh was invalid and I wrote it**, for the third time today: replacing
`if found is None:` with `if False:` leaves `found` as `None` and the next line
raises `TypeError`, so the harness reported `NOTHING RAN` rather than scoring
it. The behavioural version — `members()` returning `[]` instead of `None` — is
a real change and is caught. Three invalid mutations in one session is a
pattern worth naming: **disabling a guard clause is usually not a mutation, it
is a crash**, and the thing to mutate is what the clause guards *against*.

Both never-fires branches are covered, and this rule needs them: `Identity` is
matched by a regex over two languages' syntax, so a struct reformatted or an
interface renamed would leave it matching nothing and printing `ok`.

`scripts/check.sh`: 29/29.

## What this does not do

**It is a regex over two languages, not a parser.** A Go `Identity` declared
with embedded fields, or a TypeScript one built by `extends`, would be matched
incompletely — the member list would be short and the rule would report
*missing* members, which is loud rather than silent, but it would be reporting
the wrong thing. Neither client does this today.

**It checks declarations, not what the clients send.** A Go caller can attach
any metadata it likes through a gRPC interceptor without touching `Identity`,
and this sees none of it. That is the caller's own credential handling, outside
the library; what the rule covers is the library offering a slot for one.

**Nothing checks the *formatter* side in Go or TypeScript.** The secondary rule
only fires when `ALLOWED` is non-empty, which is never today. A `String()`
added to Go's `Identity` printing the three identity members would pass, and
should — they are not secrets — but the rule is not asserting that it stays
that way, only that the combination of a formatter and an exempted member gets
argued for.

**The three-SDK conformance runner still compares behaviour, not formatting.**
That would be the general answer — every client asked to format an identity
holding a sentinel, and the sentinel asserted absent — and it needs a fixture
in three languages for a property two of them cannot currently exhibit. I chose
the cheaper check and am recording what it is cheaper than.
