# Finding 6 was fixed in one of the two authenticators

- **Date:** 2026-09-20
- **Author:** Claude, sweeping findings 2–8 for the bypass shape found in finding 1
- **Touches:** `crates/slate-serverd/src/auth.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

`TokenIdentity` refuses a request carrying more than one `authorization`
header, instead of taking the first. That is what `MetadataIdentity` was
changed to do when security finding 6 was closed; `Authenticator` has two
implementations and the fix reached one.

## Why

The previous entry closed with: *"It does not re-examine findings 2 through 8
for the same class of bypass."* This is that sweep, and it found one.

Finding 6 is "the client's copy of an identity header wins", and its argument
is general: `metadata.get` returns the **first** value for a repeated key,
which is the caller's copy exactly when the proxy appends rather than replaces,
and *"resolving it either way would be a guess about a proxy this server cannot
see"*. The fix went into `slate_server::auth::text`, which
`MetadataIdentity` uses. `TokenIdentity` — the daemon's token mode, a different
crate — went on calling `metadata.get(AUTHORIZATION)`.

Two tokens, two principals, the caller's header first and the proxy's appended
after it:

```
a duplicated authorization header must not be resolved:
  SecurityContext { principal: Principal { id: U64(7), ... } }
```

Principal 7 is the caller's. No error, no ambiguity reported, the caller's copy
silently wins.

**Its severity is lower than the header mode's, and saying "same shape" is not
saying "same severity".** A bearer token is checked against the configured
list, so a caller needs a valid token either way. In the ordinary arrangement
the result is that a caller is resolved to *themselves* rather than to the
service principal the proxy intended — a downgrade. **No privilege escalation
was demonstrated and I do not claim one.** What it does defeat is a proxy that
*downscopes*: one that replaces a caller's broad token with a narrower one for
the request. Append instead of replace there and the caller keeps the broad
token, which is escalation relative to the deployment's intent. That is a real
arrangement and a contrived-sounding one, and stating it that way is the honest
width of the finding.

The reason to fix it anyway is the one finding 6 already argued: there is no
safe way to pick, so picking is the bug regardless of who currently benefits.

## Alternatives rejected

**Share `slate_server::auth::text` between the two.** The obvious
deduplication, and it does not fit: `text` returns a `String` for an identity
header, refusing non-ASCII with a message naming the key, where this needs the
raw value, a `Bearer ` prefix strip and a constant-time comparison. Exporting
it would mean either exporting a function that does half of what the caller
needs, or widening it until it does both jobs badly. Six lines of duplicated
`get_all` with the reasoning written once and cross-referenced is the smaller
cost.

**Refuse duplicates in the tonic layer, for every metadata key.** Closes this
class permanently rather than one instance, and it is the wrong layer: a
duplicated `grpc-accept-encoding` or a repeated trace header is ordinary HTTP/2
and refusing it would break clients over a rule that only matters for keys
carrying identity. The check belongs where a key is read *as* an identity.

**Leave it, since no escalation was demonstrated.** Defensible on impact and
wrong on principle. The whole content of finding 6 is that first-copy-wins is
not a safe rule to rely on; leaving it in a second authenticator means the
codebase argues both sides.

**Take the last copy instead, matching a proxy that appends.** Rejected for the
reason the original finding gives: it trusts a proxy that appends, and the
server cannot tell which kind it is behind. Refusing costs a misconfigured
deployment a clear error and costs an attacker the ambiguity.

## Evidence

**The gap, measured before the fix**, with two valid tokens naming principals 7
and 9 — the panic output above. `a_single_authorization_header_still_authenticates`
is the control: one copy authenticates, as principal 9, so the new test cannot
pass for an authenticator that refuses everything.

Mutation-tested, four mutations, all caught:

```
ok  the duplicate check is removed
ok  the refusal stops naming the duplicate
ok  the refusal echoes the token it rejected
ok  a single header is rejected too, so the control matters
```

The third is there because a refusal that quotes the header would put a bearer
token into whatever collects this server's errors. The fourth catches a fix
that refuses everything.

**The mutation harness caught my own bad anchor** on the first run — the
message string had been reflowed and the patch matched zero times, which it
refused rather than running the suite against unmutated code. That is the
failure it was built for, three commits ago, working on its author.

`cargo test -p slate-serverd -p slate-server --no-fail-fast`: 32 suites, no
failures. `cargo clippy --workspace --all-targets` clean.
`cargo fmt --all -- --check` clean. `scripts/check.sh` 23/23.

**The sweep's other results, which were null.** Findings 2, 3, 5, 7 and 8 were
checked for the same shape — a fix that only takes effect on one of several
paths:

- **2** (`write_many` existence oracle) — the check is hoisted inside
  `write_many` itself, which every bulk path goes through. One path.
- **3** (`EXPLAIN` histograms) — gated by `Action::Explain` being excluded from
  `Action::ALL`, which is structural rather than a call somebody must make. The
  reopening knob is an explicit grant, and the review records what granting it
  costs.
- **5** — the read side of finding 1, closed by the same catalog refusal, which
  the previous commit extended to the second constructor.
- **7** (unbounded work) and **8** (schema disclosure) — read, not probed.

Four of those are reading rather than running, which is the same standard that
missed this one. Stated below rather than counted as covered.

## What this does not do

**It does not probe findings 7 and 8.** I read their fixes and judged them
single-path; I did not build a case that reaches them another way. Finding 6
looked single-path too, from reading.

**It does not check the third `Authenticator`.** `DenyEveryone` refuses
everything and has no header to resolve, so there is nothing of this shape in
it — but that is the whole of my analysis of it.

**Nothing stops a fourth implementation repeating this.** The trait says
nothing about duplicated keys, and a new `Authenticator` reaching for
`metadata.get` would be as wrong as these two were, with no check to say so. A
shared helper or a trait-level test would close that and neither exists; the
deduplication rejected above is the obvious start and I rejected it on the
wrong grounds to also solve this.

**No test exercises a duplicated header over a real connection.** Both tests
call `authenticate` with a hand-built `MetadataMap`. Whether a client or proxy
can actually get two `authorization` headers to the server through tonic's
stack is untested here — it is ordinary HTTP/2 and I expect so, but expecting
is what this entry is about.

**The clients were not checked for sending it twice.** If one did, this change
would break it. The three-SDK conformance run and the deployed harness in CI
exercise every client against a real server, so a break would surface there
rather than here — which is a reason to trust CI, not a check I ran.
