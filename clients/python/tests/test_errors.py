"""The error mapping, against a server that really produces each code.

Every case below reaches the head node and comes back with a status. Nothing is
constructed by hand, because a hand-built `grpc.RpcError` tests this module's
opinion of the protocol rather than the protocol.

Four codes the server can emit are *not* covered, and are listed here rather
than quietly missing:

- `UNKNOWN` (`CommitTimedOut`) — needs a storage layer that times out a commit.
  `MemoryStore` cannot.
- `DATA_LOSS` (`KeyDecode`, `CorruptIndexEntry`) — needs a corrupted store.
- `DEADLINE_EXCEEDED` — a client-side deadline, which this client does not yet
  expose.
- `CANCELLED` — reachable by cancelling a stream, which `test_streaming.py`
  does, but the server does not report it back to the canceller.
"""

from __future__ import annotations

import grpc
import pytest

from slate import (
    AlreadyExists,
    Client,
    Conflict,
    Identity,
    InvalidRequest,
    JoinQuery,
    NotFound,
    NotLeader,
    PermissionDenied,
    Query,
    ResourceLimit,
    Retryable,
    SlateError,
    Unauthenticated,
    Unavailable,
    UnknownOutcome,
    i64,
    u64,
)
from slate.errors import _BY_CODE, from_rpc_error
from slate.errors import UnknownOutcome as _UnknownOutcome

from .conftest import Serving, connect
from .fixture import BOOKS, DOCS, SECRETS

# --- the classification, as a property of the hierarchy ---------------------


def test_every_grpc_code_has_a_mapping() -> None:
    """A code that fell through to the wildcard would be reported as INTERNAL.

    "The server is broken" is the wrong thing to say about a request that was
    merely refused, so the mapping is required to be total over what `grpc`
    defines rather than over what the server happens to send today.
    """
    missing = [code.name for code in grpc.StatusCode if code not in _BY_CODE]
    assert not missing, f"these codes fall through to InternalError: {missing}"


def test_unknown_is_not_retryable() -> None:
    """The single most load-bearing line in the hierarchy.

    A commit timeout becomes `UNKNOWN` precisely so that a client does not
    retry an insert that may already have succeeded. A hierarchy that put it
    under `Retryable` would undo that on every machine that imported this
    package, and no test against a live server would notice.
    """
    assert not issubclass(_UnknownOutcome, Retryable)
    assert issubclass(Conflict, Retryable)
    assert issubclass(Unavailable, Retryable)
    assert issubclass(ResourceLimit, Retryable)


def test_a_redirect_becomes_not_leader_and_carries_the_leader() -> None:
    """The one structural refinement available: metadata, not prose."""

    # Subclassing the exception the library raises, which is the only way to
    # test the one refinement this client makes on top of the status code.
    #
    # This carried `# type: ignore[misc]` and a comment saying `grpc` ships no
    # stubs so `RpcError` was `Any`. That was true of mypy, which had `grpc.*`
    # under `ignore_missing_imports`; `ty` resolves the package and the
    # subclass is unremarkable to it. The suppression is gone with the claim.
    class Fake(grpc.RpcError):
        def code(self) -> grpc.StatusCode:
            return grpc.StatusCode.UNAVAILABLE

        def details(self) -> str:
            return "this node is not the writer"

        def trailing_metadata(self) -> tuple[tuple[str, str], ...]:
            return (("slate-leader", "10.0.0.7:50051"),)

    error = from_rpc_error(Fake())
    assert isinstance(error, NotLeader)
    assert error.leader == "10.0.0.7:50051"


# --- against the real server ------------------------------------------------


def test_not_found_for_an_unknown_table(client: Client) -> None:
    from slate import Column, Table, ValueType

    ghost = Table("nowhere", [Column("id", ValueType.U64)], primary_key=["id"])
    with pytest.raises(NotFound):
        list(client.query(Query(ghost)))


def test_not_found_for_a_missing_row_on_update(client: Client) -> None:
    with pytest.raises(NotFound):
        client.update(DOCS, [(u64(8_001), "x", i64(0), None)])


def test_already_exists_for_a_duplicate_primary_key(client: Client) -> None:
    client.insert(DOCS, [(u64(400), "e", i64(1), None)])
    with pytest.raises(AlreadyExists):
        client.insert(DOCS, [(u64(400), "e", i64(1), None)])


def test_permission_denied_for_a_table_with_no_grant(client: Client) -> None:
    # Distinct from a row policy returning nothing, which is invisible by
    # design. A test that could not tell the two apart would not be testing
    # the denial.
    with pytest.raises(PermissionDenied):
        list(client.query(Query(SECRETS)))


def test_unauthenticated_when_no_identity_reaches_the_transport(
    server: Serving,
) -> None:
    with connect(server, Identity()) as anonymous, pytest.raises(Unauthenticated):
        list(anonymous.query(Query(DOCS)))


def test_invalid_argument_for_a_having_over_an_ungrouped_column(
    client: Client,
) -> None:
    """The refusal `ColumnRef` makes decidable.

    Under flat ordinals this is a legitimate reference that happens to be in
    range. Here it is a *kind* mismatch, and the server says which.
    """
    from slate import Agg, AggregateQuery

    a = AggregateQuery(BOOKS)
    a.group_by(a.c.author_id)
    a.aggregate(Agg.count())
    a.having(a.c.year.ge(i64(2000)))
    with pytest.raises(InvalidRequest, match="over groups"):
        list(client.aggregate(a))


def test_invalid_argument_for_a_computed_value_named_across_a_join(
    client: Client,
) -> None:
    """The other refusal the kind makes decidable.

    The kernel's joined space is packed by declared table width, so a computed
    value has no slot in it. A flat ordinal naming one would land on the next
    table's first column.
    """
    from .fixture import AUTHORS

    j = JoinQuery()
    a = j.add(AUTHORS)
    a.compute(a.c.born + i64(1))
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    b.having(b.c.year.gt(a.computed(0)))
    with pytest.raises(InvalidRequest, match="not addressable across inputs"):
        list(client.join(j))


def test_invalid_argument_for_a_nested_loop_on_a_right_outer_join(
    client: Client,
) -> None:
    """Not advice, unlike an index hint.

    A nested loop cannot preserve unmatched rows of the second input, and a
    quiet fallback to a hash join would return the inner rows and a short
    answer — which no assertion about rows would ever catch.
    """
    from slate import JoinAlgorithm, JoinType

    from .fixture import AUTHORS

    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")], join_type=JoinType.RIGHT)
    b.force(JoinAlgorithm.nested_loop())
    with pytest.raises(InvalidRequest):
        list(client.join(j))


def test_invalid_argument_for_a_join_of_one_table(client: Client) -> None:
    j = JoinQuery()
    j.add(DOCS)
    with pytest.raises(InvalidRequest, match="at least two inputs"):
        list(client.join(j))


def test_resource_exhausted_when_a_build_side_is_too_large(client: Client) -> None:
    """The limit is on a *hash* build side, so the algorithm has to be forced.

    Left to the planner this fixture is small enough to get a nested loop,
    which has no build side and no limit to exceed — the test would pass by
    not reaching the code it is about. Worth stating: `build_limit` on the
    wire reads like a property of the request, and it is a property of one
    algorithm.
    """
    from slate import JoinAlgorithm

    from .fixture import AUTHORS

    j = JoinQuery()
    a = j.add(AUTHORS)
    b = j.add(BOOKS, on=[(a.c.id, "author_id")])
    b.force(JoinAlgorithm.hash_build_left())
    j.build_limit(1)
    with pytest.raises(ResourceLimit):
        list(client.join(j))


def test_a_write_to_a_follower_is_not_leader_with_the_leader_named(
    follower_server: Serving,
) -> None:
    with connect(follower_server) as follower:
        with pytest.raises(NotLeader) as caught:
            follower.insert(DOCS, [(u64(410), "x", i64(1), None)])
        assert caught.value.leader == "the-other-node"
        # And it is retryable, because the request should be retried — just
        # not here.
        assert isinstance(caught.value, Retryable)


def test_a_follower_still_serves_reads(follower_server: Serving) -> None:
    """The read/write split is what makes a handover survivable.

    A node that refused everything would drop those connections for no gain.
    """
    with connect(follower_server) as follower:
        # The reads still work, and they come back from the pool rather than
        # from the writer this node may not use.
        assert len(list(follower.query(Query(DOCS)))) == 7
        assert not follower.leadership().is_leader


def test_a_conflict_is_aborted(server: Serving) -> None:
    with connect(server) as one, connect(server) as two:
        with pytest.raises(Conflict) as caught, one.transaction() as first:
            first.insert(DOCS, [(u64(420), "one", i64(1), None)])
            with two.transaction() as second:
                second.insert(DOCS, [(u64(420), "two", i64(2), None)])
        assert caught.value.code is grpc.StatusCode.ABORTED


def test_every_error_is_a_slate_error(client: Client) -> None:
    """So that `except SlateError` is a complete catch."""
    with pytest.raises(SlateError):
        client.update(DOCS, [(u64(8_002), "x", i64(0), None)])


def test_the_status_code_survives_on_the_exception(client: Client) -> None:
    """Kept because it is the server's own classification.

    Discarding it would make this hierarchy the only account of what happened,
    and this hierarchy is coarser than the codes are.
    """
    with pytest.raises(NotFound) as caught:
        client.update(DOCS, [(u64(8_003), "x", i64(0), None)])
    assert caught.value.code is grpc.StatusCode.NOT_FOUND
    assert caught.value.message


def test_unknown_outcome_is_never_produced_by_this_fixture() -> None:
    """Named so the gap is visible rather than absent. See the module docstring."""
    assert issubclass(UnknownOutcome, SlateError)


def test_every_error_class_accepts_every_keyword_the_base_takes() -> None:
    """The guard for the mistake this file caught three times.

    `NotLeader` used to override `__init__` to forward the base's keywords by
    hand so it could set `leader`. Three times a keyword was added to the base,
    the last being `violations`, and three times the override went stale — each
    time surfacing as a `TypeError` on a redirect: loud, but only on a path a
    suite has to happen to exercise.

    The override is gone, so nothing can go stale today. This is here for the
    next subclass that grows one, because the reasoning that produced it the
    first time is perfectly good reasoning and will recur. It constructs every
    error class with every keyword `from_rpc_error` passes, which is the real
    contract: that function calls `kind(...)` with a fixed keyword set and has
    no idea which class it picked.
    """
    # `SlateError` itself is included, because it is what the others must stay
    # compatible with: constructing it proves the keyword list here has not
    # drifted from the signature it is meant to mirror.
    every = _descendants(SlateError)
    assert len(every) > 10, "the walk found almost nothing; it is not walking"
    for subclass in every:
        error = subclass(
            "a message",
            code=grpc.StatusCode.UNAVAILABLE,
            trailers={"slate-leader": "elsewhere:1"},
            reason="A_TOKEN",
            request_id="req-1",
            violations=[],
        )
        assert error.reason == "A_TOKEN", subclass.__name__
        assert error.request_id == "req-1", subclass.__name__
        assert error.violations == [], subclass.__name__


def _descendants(root: type) -> list[type]:
    """`root` and every subclass of it, however deep."""
    found = [root]
    for subclass in root.__subclasses__():
        found.extend(_descendants(subclass))
    return found
