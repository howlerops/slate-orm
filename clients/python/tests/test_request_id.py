"""The id this client sends, and where a caller finds it afterwards.

`slate-serverd` writes one line per request when `[observability] request_log`
is on, and until now a caller holding a failure had no way to say which line
was theirs. Every call this client makes now carries a `slate-request-id`, and
every failure it raises carries the same value.

What is *not* here is the other half of the correlation — that the server
prints what was sent — because this suite runs against `slate-testserver`,
which has no request log. That half is
`crates/slate-serverd/tests/observing.rs::the_log_carries_the_id_the_caller_sent`,
against the real daemon through a real socket.
"""

from __future__ import annotations

import pathlib
import re

import pytest

from slate import Client, Query, SlateError, Table, u64
from slate.client import REQUEST_ID_KEY

from .fixture import DOCS


def _sent(client: Client) -> list[tuple[str, str]]:
    """The metadata one call would carry, without making it."""
    _, metadata = client._sending()
    return list(metadata)


def test_every_call_carries_an_id(client: Client) -> None:
    metadata = dict(_sent(client))
    assert REQUEST_ID_KEY in metadata
    # 32 hex characters: a uuid4 without its dashes. Asserted rather than
    # eyeballed because the server filters this value to `[A-Za-z0-9._:-]` and
    # truncates at 64, so a client that started sending something longer or
    # punctuated would be silently altered on the way into the log.
    sent = metadata[REQUEST_ID_KEY]
    assert len(sent) == 32, sent
    assert all(c in "0123456789abcdef" for c in sent), sent


def test_the_identity_still_travels_beside_it(client: Client) -> None:
    # The id is appended to the identity metadata rather than replacing it,
    # which is the kind of thing that works until the tuple is built wrong.
    metadata = dict(_sent(client))
    assert metadata["slate-principal"] == "u64:1"
    assert metadata["slate-tenant"] == "u64:1"


def test_two_calls_get_two_ids(client: Client) -> None:
    # Per call, not per session or per connection. A session-wide id would name
    # every line the session wrote, which is what a caller already has.
    first = dict(_sent(client))[REQUEST_ID_KEY]
    second = dict(_sent(client))[REQUEST_ID_KEY]
    assert first != second


def test_a_failure_names_the_id_of_the_call_that_failed(client: Client) -> None:
    # The whole point. A caller with this id can go and find the server's line.
    missing = Table("not_a_table", [("id", DOCS.columns[0].type)], primary_key=["id"])
    with pytest.raises(SlateError) as raised:
        list(client.query(Query(missing)))
    assert len(raised.value.request_id) == 32, raised.value.request_id


def test_a_failure_part_way_through_a_stream_names_it_too(client: Client) -> None:
    # A scan can fail on its tenth message as easily as its first, and the id
    # names the same call either way: the server logged one line for the whole
    # stream, at its head.
    q = Query(DOCS)
    stream = client.query(q.where(q.c.id.eq(u64(1))))
    # Reaching inside because there is no way to make a healthy server fail
    # mid-stream on demand; what is being checked is that the stream kept the
    # id rather than dropping it after the first message.
    assert len(stream._request_id) == 32
    list(stream)


def test_an_error_raised_before_any_call_has_no_id() -> None:
    # `""` rather than a made-up one: an id that never went anywhere would send
    # somebody looking for a log line that cannot exist.
    error = SlateError("nothing was sent", code=__import__("grpc").StatusCode.UNKNOWN)
    assert error.request_id == ""


def test_the_key_matches_the_one_the_server_reads() -> None:
    """The client's spelling, against the server's source.

    A mismatch here is *silent*: the server ignores metadata it does not
    recognise, so a misspelled key means every call carries an id nobody ever
    sees and every log line says `id=-`. No test on either side alone can
    catch that, because each is internally consistent.

    Reading the Rust constant rather than restating it, which is the same
    argument the demo's `TABLES` guard makes: a second copy of a shared
    spelling is a second place for it to drift.
    """
    auth = (
        pathlib.Path(__file__).resolve().parents[3]
        / "crates"
        / "slate-server"
        / "src"
        / "auth.rs"
    )
    # Asserted, not skipped. A skip here would be green on a wrong path, and
    # the wrong path is the likelier bug: this suite already needs the repo
    # (`conftest._build` runs cargo in it), so the file not being there means
    # this test is looking in the wrong place rather than that the repo is
    # absent — and a guard that quietly stops guarding is worse than none.
    assert auth.exists(), f"the server source should be at {auth}"
    declared = re.search(
        r'pub const REQUEST_ID_KEY: &str = "([^"]+)";', auth.read_text()
    )
    assert declared is not None, f"no REQUEST_ID_KEY in {auth}"
    assert declared.group(1) == REQUEST_ID_KEY
