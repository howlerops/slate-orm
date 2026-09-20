# The Python client printed the bearer token the server end hides

- **Date:** 2026-09-20
- **Author:** Claude, on the last of the review's four unexamined areas
- **Touches:** `clients/python/src/slate/client.py`, `clients/python/tests/test_identity_repr.py`, `scripts/mutate.py`, `scripts/test_mutate.py`, `docs/security-review.md`
- **Kind:** security

## What changed

`Identity.__repr__` prints the three identity keys' values and redacts every
other value, keeping its key. `scripts/mutate.py` gains a `pytest` dialect,
because it could not read the Python client's suite at all.

## Why

The last row of the review's "not examined in depth" list is *the Python
client*. Two things there are worth a look: how it reaches the server, and what
it holds.

The first is fine and deliberate. `grpc.insecure_channel` is the default, with
a comment saying why — TLS belongs to whatever fronts the server, and a second
place to configure it makes the deployment worse — and `channel=` is the
documented way to pass a secure one.

The second is not. `Identity.extra` is the client's only way to authenticate
against a deployment not using the shipped `MetadataIdentity`, and the class's
own docstring says what goes in it: *"a bearer token, a mesh header"*. The
`repr` printed the whole metadata dict:

```
Identity({'slate-principal': 'u64:1', 'slate-tenant': 'u64:2',
          'slate-roles': 'app', 'authorization': 'Bearer zzSECRETzz…'})
```

A `repr` reaches further than it looks: a traceback that formats locals, a
structured log, a debugger, a failed assertion, `print`. And the thing that
makes this more than a paper cut is the symmetry. Hours earlier I strengthened
the redacting `Debug` on `slate-serverd`'s `TokenIdentity` and
`slate-slatedb`'s `Credentials`, both of which exist precisely so a credential
does not reach a log nobody classified as sensitive. **The server end of this
wire redacts the token; the client end printed it.**

**Only Python**, and I checked rather than assumed — that is the question this
session has been wrong about five times. Go's `Identity` is
`{Principal, Tenant, Roles}` with no `String()`; TypeScript's is an interface
with `principal`, `tenant`, `roles`. Neither can hold a credential and neither
has a formatter to leak one. Python's `extra` is the only first-class place a
credential lives in any of the three clients, and it was the one that printed
it.

## Alternatives rejected

**Redact every `extra` value and drop the key too.** Tidier output, and it
answers the wrong question. "Is my `authorization` header set at all" is what a
caller debugging this actually needs, and a repr that hid the entry would say
"no" by omission. The key is the useful half; the value is the secret half.

**Redact by key name** — anything matching `authorization`, `*token*`,
`*secret*`. A proxy for the hazard rather than the hazard, which cost 47 and
then 59 false positives on the converter rule this morning. The real property
is "this client did not put it there as an identity", and the three identity
keys are a closed set, so the complement needs no pattern.

**Drop `extra` from `Identity` and make callers use a gRPC interceptor**, as Go
and TypeScript callers must. It would remove the credential from this class
entirely. Rejected because `extra` is the only reason the Python client works
against a token-mode or mesh deployment at all, and pushing every such user to
write an interceptor to avoid a formatting bug is a worse trade than fixing the
formatter.

**A `Secret` wrapper for the value.** The same argument I rejected it under on
the Rust side this morning, and weaker here: `extra` takes `(str, str)` pairs
from the caller, so a wrapper would be a new type in a public signature to
protect one method.

## Evidence

**The leak, before the fix**, quoted above, produced by constructing an
`Identity` with `extra=[("authorization", "Bearer zzSECRETzz…")]` and calling
`repr`.

Four cases in `clients/python/tests/test_identity_repr.py`: the redaction, the
control that identity values still print, a roster assertion on `_PRINTABLE`,
and the empty identity. The control is not decoration — a `repr` returning a
constant would satisfy the redaction test and be useless.

Four mutations, all caught:

```
ok  the repr prints every value again, as it did      -> test_an_extra_headers_value_is_redacted
ok  the repr redacts everything, including the identity -> test_the_three_identity_values_are_still_printed
ok  an extra key is dropped rather than redacted      -> test_an_extra_headers_value_is_redacted
ok  authorization is quietly added to the printable set -> two tests
```

**The first attempt at those mutations could not be scored, and the harness
said so.** `mutate.py` knew two output dialects, libtest's and this
repository's own `scripts/test_*.py` style, and pytest matches neither: `4
passed in 0.01s` is not `4 passed, 0 failed`. It printed `the command reported
no test results at all` rather than reading a clean run as a survivor — the
second failure mode in its own docstring, met while using the tool written for
it. A `pytest` dialect is two lines, and `test_mutate.py` now has a case for
it plus one asserting that reading pytest's output under the *wrong* dialect
reports nothing rather than passing. 10 cases there, all green.

`clients/python`: **299 passed**, the whole suite against a real
`slate-testserver` over a socket, not the one file. `scripts/check.sh`: 27/27,
after `ruff` caught a Yoda condition in the new test that local habit had
written the other way round.

## What this does not do

**It covers `repr`, not every way an identity can escape.** `Identity` has no
`__str__`, no `__format__`, no serialisation and is not a dataclass, so `repr`
is the formatter — but the metadata itself is reachable through the public
`.metadata` property, and a caller who logs that gets everything. That is a
caller's decision about their own credential; the library printing it by
default was not.

**It does not audit what the *client* logs.** Nothing in `slate` calls
`logging`, which I checked by grep, so there is no log line of its own to leak
into. A caller's logging framework formatting a `Client` gets Python's default
`<slate.client.Client object at …>`, which is safe — but that is the default
repr, not a decision anybody made, and adding one later would reintroduce this.

**The insecure channel default was accepted, not re-argued.** It is reasoned in
a comment and has an escape hatch, and a bearer token over `insecure_channel`
off loopback is exactly what `slate-serverd` forces an operator to write
`transport = "tls-terminated-upstream"` to acknowledge. Whether the *client*
should also refuse to send an `authorization` header over an insecure channel
is a real question and I did not answer it — it would need a way to ask a
`grpc.Channel` whether it is secure, which the API does not obviously offer.

**Go and TypeScript were read, not tested.** Their `Identity` types cannot
carry a credential today. Nothing stops one gaining an `extra` field tomorrow,
and there is no check across the three clients for this property — the
three-SDK conformance runner compares behaviour, not formatting.
