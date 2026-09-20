"""An `Identity`'s `repr` never prints a credential.

`Identity.extra` is documented as carrying "a bearer token, a mesh header" —
it is this client's only way to authenticate against a deployment that is not
using the shipped `MetadataIdentity`. The `repr` printed the whole metadata
dict, so the token went into anything that formats an object: a traceback with
locals, a log line, a debugger, a failed assertion.

That is the same hazard `slate-serverd`'s `TokenIdentity` and
`slate-slatedb`'s `Credentials` both hand-write a redacting `Debug` for, on the
other end of the same wire. The client holding the secret printed it.

Needs no server: this is about a `repr`, and starting one would only make the
test slower and able to fail for unrelated reasons.
"""

from __future__ import annotations

from slate import Identity
from slate.client import PRINCIPAL_KEY, ROLES_KEY, TENANT_KEY

#: Distinctive, so an assertion cannot pass because the secret happened to be a
#: substring of something else in the output.
SENTINEL = "zzSECRETzz-must-not-be-logged-zz"


def test_an_extra_headers_value_is_redacted() -> None:
    printed = repr(
        Identity(
            "u64:1",
            tenant="u64:2",
            roles=["app"],
            extra=[("authorization", f"Bearer {SENTINEL}")],
        )
    )

    assert SENTINEL not in printed, printed
    # The key survives, and that is deliberate rather than an oversight: "is my
    # authorization header set at all" is the question a caller debugging this
    # actually has, and dropping the whole entry would answer it wrongly while
    # looking tidy.
    assert "authorization" in printed, printed
    assert "<redacted>" in printed, printed


def test_the_three_identity_values_are_still_printed() -> None:
    """The control.

    Without this, a `repr` that redacted everything — or returned a constant —
    would satisfy the test above and be useless. The caller's own principal,
    tenant and roles are not secrets, and they are the whole reason to look at
    an `Identity` in a debugger.
    """
    printed = repr(Identity("u64:1", tenant="u64:2", roles=["app", "reader"]))

    assert "u64:1" in printed and "u64:2" in printed, printed
    assert "app,reader" in printed, printed
    assert "<redacted>" not in printed, printed


def test_every_key_the_repr_prints_in_full_is_an_identity_key() -> None:
    """The roster, so a fourth printable key has to be argued for.

    `_PRINTABLE` is what decides which values survive. If a key were added to
    it — a fourth identity header, or an `extra` somebody judged harmless —
    this is where that decision gets made rather than noticed later.
    """
    assert frozenset({PRINCIPAL_KEY, TENANT_KEY, ROLES_KEY}) == Identity._PRINTABLE


def test_an_empty_identity_prints_nothing_to_redact() -> None:
    assert repr(Identity()) == "Identity({})"
