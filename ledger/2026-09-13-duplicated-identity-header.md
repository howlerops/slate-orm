# Refusing a repeated identity header instead of picking one

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `slate-server` — `auth.rs`, `tests/security_probe.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

`MetadataIdentity`'s metadata lookup reads `get_all` rather than `get`, and a
request carrying `slate-principal`, `slate-tenant` or `slate-roles` more than
once is refused with `UNAUTHENTICATED` instead of resolved. Six unit tests,
the review's probe inverted from asserting the defect to asserting the refusal,
and the type's own documentation corrected.

## Why

Security review finding 6. `get` returns the **first** value for a repeated
key, and the first is the client's exactly when the proxy appends its headers
instead of replacing them — `add_header` written where `proxy_set_header` was
meant.

That is not graceful degradation. `MetadataIdentity` is documented as correct
behind a proxy that "sets these three headers itself, and strips any copies the
client supplied", and under the one misconfiguration of that arrangement a
caller chose their own principal, their own tenant, and their own roles. The
deployment looks like it is working, because the proxy's headers are present
and are simply never read.

## Alternatives rejected

**Take the last copy instead of the first.** The obvious one-character fix, and
wrong: it trusts a proxy that appends, and breaks a proxy that replaces — and
the server cannot tell which it is behind. It swaps one silent assumption for
another rather than removing the assumption. Refusing is the only answer that
does not require knowing something the server does not know.

**Treat identical duplicates as benign** and refuse only conflicting ones. This
was tempting enough to write a test against: it would make the check pass or
fail depending on what the *attacker* chose to send, and an attacker who echoes
the proxy's value exactly would sail through. It also fails to flag the
misconfiguration in the case where nobody is attacking yet.

**Refuse at the proxy layer / document it harder.** The mode is already
documented at length and the constructor is called
`trusting_the_caller_completely()`. Documentation did not prevent this, because
the failure is invisible from inside the deployment.

**Strip duplicates and carry on.** Same defect as picking one, with the
evidence destroyed.

## Evidence

Six tests, and two mutations, both killed:

- Disabling the duplicate check (reverting to first-copy-wins) fails five of
  the six tests.
- Relaxing it to allow *identical* duplicates fails exactly
  `two_identical_copies_are_still_refused` — the test written for that case,
  and nothing else, which is what makes it worth its place.

The inverted probe also asserts the refusal message does not echo the identity
the caller tried to claim (`666`), so a rejected impersonation attempt is not
reflected back. 358 tests pass across `slate-server` and `slate-serverd`;
clippy clean under the crate's test lint convention.

## What this does not do

It closes the one part of the trusted-header arrangement the server can check
by itself. Everything else remains the deployment's to get right: a proxy that
authenticates nobody, or authenticates and then sets the wrong principal, is
indistinguishable from a correct one from in here.

It does not add a mode that verifies the identity independently. That would be
a second identity system inside a head node that exists to sit behind a mesh
that already has one.
