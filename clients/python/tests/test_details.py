"""Decoding the reason token out of a status's details.

The blob below was captured from a running head node rather than built here,
which is the point: a fixture this module encoded itself would agree with this
module's own idea of the wire format, and agreeing with yourself proves
nothing. This one is what `slate-serverd` actually sent, so a decoder that
drifts from the server fails here.

The same bytes are decoded by the Go and TypeScript suites, so the three
clients are checked against one artefact rather than three transcriptions.
"""

from __future__ import annotations

import grpc
import pytest

from slate._details import ERROR_INFO_URL, check_failures_of, reason_of
from slate.errors import (
    _DETAILS_KEY,
    NotFound,
    ResourceLimit,
    RpcCall,
    from_batch_error,
    from_rpc_error,
)

#: A real `grpc-status-details-bin`, captured from a `delete_where` with
#: `returning` that matched more rows than `max_returned_rows` allowed.
#: `google.rpc.Status{code: 8, message: "...", details: [ErrorInfo{...}]}`.
BLOB = bytes.fromhex(
    "08081290016120707265646963617465207772697465206d617463686564206d6f72"
    "65207468616e203320726f77732c20616e642069747320726f777320776572652061"
    "736b656420666f723b206e6172726f7720746865207072656469636174652c206472"
    "6f7020746865207265717565737420666f722074686520726f77732c206f72207261"
    "69736520746865206c696d69741a520a28747970652e676f6f676c65617069732e63"
    "6f6d2f676f6f676c652e7270632e4572726f72496e666f12260a1950524544494341"
    "54455f57524954455f544f4f5f4c415247451209736c6174652d6f726d"
)


def test_the_token_comes_out_of_a_real_blob() -> None:
    assert reason_of(BLOB) == "PREDICATE_WRITE_TOO_LARGE"


def test_the_url_is_the_one_the_server_packs_under() -> None:
    # Not a restatement of the constant: the captured blob carries the URL as
    # bytes, so this asserts the client's idea of it matches the server's.
    assert ERROR_INFO_URL.encode() in BLOB


@pytest.mark.parametrize(
    "blob",
    [
        pytest.param(b"", id="empty"),
        pytest.param(b"\xff\xff\xff\xff", id="not a protobuf at all"),
        # A well-formed Status with no details at all: code 8, no field 3.
        pytest.param(b"\x08\x08", id="a status carrying no details"),
    ],
)
def test_anything_else_is_the_empty_token(blob: bytes) -> None:
    # Never an exception: a client that raised while building an error object
    # would replace the server's failure with its own, and the caller would
    # stop learning why the call failed at all.
    assert reason_of(blob) == ""


def test_a_truncated_blob_does_not_raise() -> None:
    # Every prefix of a real message, including the ones that parse into
    # something plausible and the ones that do not.
    for cut in range(len(BLOB)):
        assert reason_of(BLOB[:cut]) in ("", "PREDICATE_WRITE_TOO_LARGE")


def _tag(number: int, wire: int) -> bytes:
    return _varint((number << 3) | wire)


def _varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | (0x80 if value else 0))
        if not value:
            return bytes(out)


def _bytes_field(number: int, payload: bytes) -> bytes:
    return _tag(number, 2) + _varint(len(payload)) + payload


def decoy(url: str, reason: str = "DECOY") -> bytes:
    """A well-formed `Status` whose one detail is packed under `url`.

    Encoded by hand, in the opposite direction from the decoder under test, so
    that agreeing with it says something. The payload is shaped exactly like an
    `ErrorInfo` — field 1, a string — so a decoder that skipped the type check
    would hand back `reason` rather than `""`. That is the whole point: the
    check is what stops one message's field 1 being read as another's.
    """
    packed = _bytes_field(1, reason.encode())
    any_message = _bytes_field(1, url.encode()) + _bytes_field(2, packed)
    return _tag(1, 0) + _varint(8) + _bytes_field(3, any_message)


def test_a_detail_of_another_type_is_skipped_not_misread() -> None:
    blob = decoy("type.googleapis.com/google.rpc.RetryInfo")
    assert reason_of(blob) == ""


def test_the_decoy_would_be_read_without_the_type_check() -> None:
    """The negative control for the test above.

    Without it, `test_a_detail_of_another_type_is_skipped_not_misread` passes
    for two different reasons — the URL check working, or the decoy being
    malformed and decoding to nothing either way — and only one of them is the
    property. This pins the second: packed under the URL the server really
    uses, the very same bytes come back as a token.
    """
    assert reason_of(decoy(ERROR_INFO_URL)) == "DECOY"


class _FakeCall(Exception):
    """A `grpc.RpcError` as a call delivers one, with the blob in its trailers.

    Hand-rolled rather than mocked: `from_rpc_error` reads four things off the
    exception and this supplies exactly those, so a change to what it reads
    shows up as an AttributeError here rather than as a silently empty field.
    """

    def __init__(self, blob: bytes) -> None:
        super().__init__("resource exhausted")
        self._blob = blob

    def code(self) -> grpc.StatusCode:
        return grpc.StatusCode.RESOURCE_EXHAUSTED

    def details(self) -> str:
        return "a predicate write matched more rows than it may return"

    def trailing_metadata(self) -> tuple[tuple[str, object], ...]:
        # A text trailer beside the binary one, because `_trailers` and
        # `_reason` walk the same sequence and must not trip over each other.
        return (("content-type", "application/grpc"), (_DETAILS_KEY, self._blob))


def test_from_rpc_error_carries_the_token_onto_the_exception() -> None:
    """The wiring test, and a different test from the ones above on purpose.

    Those call `reason_of` directly, so all of them keep passing if
    `from_rpc_error` simply stops asking for a token — which is exactly the
    state this change fixed. The same mutation in the Go client survived every
    decoder test and was caught only by the conformance runner, which is a slow
    and indirect way to learn that one client stopped filling one field.
    """
    error = from_rpc_error(_FakeCall(BLOB))
    assert error.reason == "PREDICATE_WRITE_TOO_LARGE"
    # Unchanged by carrying a token: the class, and the text trailers.
    assert isinstance(error, ResourceLimit)
    assert error.trailers == {"content-type": "application/grpc"}


def test_a_failure_with_no_details_has_the_empty_token() -> None:
    error = from_rpc_error(_FakeCall(b""))
    assert error.reason == ""


class _CodeOnly(Exception):
    """A failure carrying `code` and not `details`.

    The one behaviour difference between the `RpcCall` Protocol and the two
    independent `hasattr` calls it replaced. gRPC does not produce this — the
    two methods come together off `grpc.Call` — so the test pins the choice
    rather than a requirement: both or neither.
    """

    def code(self) -> grpc.StatusCode:
        return grpc.StatusCode.RESOURCE_EXHAUSTED


def test_a_half_shaped_failure_is_taken_as_neither() -> None:
    # Suppressed because the excluded case *is* the test: `_CodeOnly` is
    # neither an `RpcError` nor an `RpcCall` — it has `code` and not `details`
    # — and what is being pinned is what `from_rpc_error` does when handed one.
    error = from_rpc_error(_CodeOnly("half a call"))  # ty: ignore[invalid-argument-type]
    assert error.code is grpc.StatusCode.UNKNOWN, (
        "code and details are taken together; one without the other is not a call"
    )
    assert error.message == "half a call"


def test_the_protocol_matches_what_a_real_failure_carries() -> None:
    # `runtime_checkable` checks attribute presence only, which is what makes
    # it a drop-in for `hasattr` — and what makes the hand-rolled fake below,
    # which is not a `grpc.Call`, still count as one.
    assert isinstance(_FakeCall(b""), RpcCall)
    assert not isinstance(_CodeOnly("x"), RpcCall)


#: A real `grpc-status-details-bin` from a write that violated **three**
#: checks, captured the same way `BLOB` was — printed by
#: `cargo test -p slate-server --test status -- --ignored --nocapture
#: emit_a_check_violation_blob`, which exists to produce exactly this.
#:
#: Three failures on purpose, and the third with neither a column nor a
#: message: `discount_under_price` spans two columns, so naming one would be a
#: lie a form renders beside the wrong field. A fixture with one failure, or
#: three identical ones, would not separate "reads the list" from "reads the
#: first" or "assumes every failure has a column".
CHECKS_BLOB = bytes.fromhex(
    "0803129b01726f772076696f6c61746573203320636865636b73206f6e207461626c"
    "652060646f6373603a20607469746c655f6c656e677468603a205469746c65206d75"
    "7374206265203120746f20383020636861726163746572732e3b206073697a655f70"
    "6f736974697665603a2053697a652063616e6e6f74206265206e656761746976652e"
    "3b2060646973636f756e745f756e6465725f7072696365601ae1020a28747970652e"
    "676f6f676c65617069732e636f6d2f676f6f676c652e7270632e4572726f72496e66"
    "6f12b4020a0f434845434b5f56494f4c4154494f4e1209736c6174652d6f726d1a0f"
    "0a06636f6c756d6e12057469746c651a180a07636865636b2e31120d73697a655f70"
    "6f7369746976651a1f0a07636865636b2e321214646973636f756e745f756e646572"
    "5f70726963651a110a08636f6c756d6e2e3012057469746c651a2e0a096d65737361"
    "67652e3012215469746c65206d757374206265203120746f20383020636861726163"
    "746572732e1a100a08636f6c756d6e2e31120473697a651a250a096d657373616765"
    "2e31121853697a652063616e6e6f74206265206e656761746976652e1a0d0a057461"
    "626c651204646f63731a0f0a0a76696f6c6174696f6e731201331a170a0763686563"
    "6b2e30120c7469746c655f6c656e6774681a150a05636865636b120c7469746c655f"
    "6c656e677468"
)


def test_every_failing_check_comes_back_typed() -> None:
    """The payoff of publishing `column` and `message`: no prose to parse.

    A form reads `column` to pick the field and `message` to fill it. The
    alternative a caller has without this is a regular expression over the
    status text, which lasts until somebody rewords a sentence.
    """
    failures = check_failures_of(CHECKS_BLOB)
    assert [f.check for f in failures] == [
        "title_length",
        "size_positive",
        "discount_under_price",
    ], "declaration order, not the map's"
    assert failures[0].column == "title"
    assert failures[0].message == "Title must be 1 to 80 characters."
    assert failures[1].column == "size"
    assert failures[1].message == "Size cannot be negative."
    # The cross-column one: a name and nothing to hang it on.
    assert failures[2].column is None
    assert failures[2].message is None


def test_the_order_is_the_schemas_and_not_the_maps() -> None:
    """`check.10` must not sort between `check.1` and `check.2`.

    The metadata is a string-keyed map and the server sends the index in the
    key, so anything that walked the map in key order would be right for nine
    failures and wrong for eleven. Reading `violations` and counting up is
    what makes that unreachable, and this is the assertion that says so —
    against the real three-failure blob, whose map order is *not* the
    declaration order.
    """
    # The captured bytes really do carry the keys out of order, which is what
    # makes this worth asserting rather than assuming.
    assert CHECKS_BLOB.index(b"check.1") < CHECKS_BLOB.index(b"check.0")
    assert check_failures_of(CHECKS_BLOB)[0].check == "title_length"


def violation_decoy(reason: str, metadata: dict[str, str]) -> bytes:
    """An `ErrorInfo` with a chosen reason and a chosen metadata map.

    Hand-encoded in the opposite direction from the decoder, like `decoy`
    above and for the same reason. This one exists because two properties
    cannot be reached with a blob the server would actually send: a
    *non*-check failure carrying check-shaped keys, and a metadata map whose
    count and keys disagree. Both are what the decoder's guards are for, and
    both were mutations that survived until this existed.
    """
    fields = _bytes_field(1, reason.encode()) + _bytes_field(2, b"slate-orm")
    for key, value in metadata.items():
        entry = _bytes_field(1, key.encode()) + _bytes_field(2, value.encode())
        fields += _bytes_field(3, entry)
    any_message = _bytes_field(1, ERROR_INFO_URL.encode()) + _bytes_field(2, fields)
    return _tag(1, 0) + _varint(9) + _bytes_field(3, any_message)


def test_the_decoy_is_read_when_it_says_it_is_a_check_violation() -> None:
    """The negative control, in the shape this file already uses.

    Without it the two tests below pass for two different reasons — the guard
    working, or the hand-encoder producing something no decoder could read —
    and only one of those is the property.
    """
    blob = violation_decoy(
        "CHECK_VIOLATION",
        {"violations": "1", "check.0": "only", "column.0": "a"},
    )
    assert [f.check for f in check_failures_of(blob)] == ["only"]


def test_check_shaped_metadata_under_another_reason_is_ignored() -> None:
    """A failure that is not a check violation yields nothing, whatever it carries.

    The real `BLOB` cannot show this: it carries no `violations` key, so a
    decoder missing the reason check falls through to the same empty answer by
    accident. This one carries the keys and the wrong reason, so only the
    check itself can produce the empty list.
    """
    blob = violation_decoy(
        "UNIQUE_VIOLATION",
        {"violations": "1", "check.0": "not_a_check", "column.0": "email"},
    )
    assert check_failures_of(blob) == []


def test_a_count_the_keys_do_not_match_yields_nothing() -> None:
    """Three promised, two present: the prefix would be a quiet lie.

    A caller shown two failures for a row that broke three fixes two fields,
    resubmits, and is refused again — which is the round-trip-per-field
    behaviour this whole feature exists to remove. Returning nothing makes the
    disagreement visible instead.
    """
    blob = violation_decoy(
        "CHECK_VIOLATION",
        {"violations": "3", "check.0": "one", "check.1": "two"},
    )
    assert check_failures_of(blob) == []


def test_a_failure_that_is_not_a_check_violation_has_none() -> None:
    """`BLOB` is a predicate write that matched too many rows."""
    assert check_failures_of(BLOB) == []


def test_rubbish_yields_no_failures_rather_than_raising() -> None:
    """The reasoning `reason_of` gives: never replace the server's failure."""
    assert check_failures_of(b"not a status") == []
    assert check_failures_of(b"") == []


def test_the_error_a_caller_catches_carries_them() -> None:
    """End to end, and a different test from the ones above on purpose.

    Those call `check_failures_of` directly, so every one of them keeps
    passing if `from_rpc_error` simply stops asking — which is exactly the
    wiring a caller depends on and never touches itself. The reasoning is the
    token test's, two functions up.
    """
    error = from_rpc_error(_FakeCall(CHECKS_BLOB))
    assert error.reason == "CHECK_VIOLATION"
    assert [f.column for f in error.violations] == ["title", "size", None]
    assert error.violations[0].message == "Title must be 1 to 80 characters."


def test_an_ordinary_failure_carries_an_empty_list() -> None:
    """Not `None`: a caller iterating does not have to check first."""
    error = from_rpc_error(_FakeCall(BLOB))
    assert error.violations == []


def test_a_batched_refusal_carries_the_same_violations() -> None:
    """The field a batched failure could not have.

    An independent batch reports each failure *as data* inside a successful
    response, so there are no trailers and no `grpc-status-details-bin` — a
    form submitted as a batch got the token and the prose and nothing to put
    beside a field. The server puts the same blob in the message body now.

    Against the same fixture the lone path uses, which is the point: one blob,
    one decoder, and a batched refusal that cannot come to disagree with an
    unbatched one.
    """
    error = from_batch_error(
        grpc.StatusCode.INVALID_ARGUMENT.value[0],
        "row violates 3 checks on table `docs`",
        "CHECK_VIOLATION",
        CHECKS_BLOB,
    )
    assert error.reason == "CHECK_VIOLATION"
    assert [f.check for f in error.violations] == [
        "title_length",
        "size_positive",
        "discount_under_price",
    ]
    assert error.violations[2].column is None


def test_a_batched_failure_with_no_details_has_no_violations() -> None:
    """Which is most of them, and is why `details` defaults to empty."""
    error = from_batch_error(grpc.StatusCode.NOT_FOUND.value[0], "no such row", "")
    assert error.violations == []
    assert isinstance(error, NotFound)


def test_a_batched_failure_with_rubbish_details_does_not_raise() -> None:
    """The reasoning `reason_of` gives, one path over."""
    error = from_batch_error(
        grpc.StatusCode.INVALID_ARGUMENT.value[0],
        "refused",
        "CHECK_VIOLATION",
        b"not a status",
    )
    assert error.violations == []
