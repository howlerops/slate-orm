"""The client: a connection, a session, a transaction, and the streams they read.

# The three objects, and why there are three

`Client` is a connection and an identity. It is *also* a session, because the
common case is one process acting as one caller and making it write
`client.session()` before it can read anything would be ceremony with no
decision in it.

`Session` is a freshness scope: the watermark that makes reads
read-your-writes and monotonic lives here. A process serving many end users
should give each one its own — `client.session()` — so that one user's write
does not pin another user's reads to the writer.

`Transaction` is a scope on the writer. It is a context manager, it commits on
a clean exit and rolls back on any exception, and it refuses a `freshness`
argument rather than ignoring one: a read inside a transaction is served by the
transaction, so freshness is not a thing it can honour and accepting the
argument would be a lie.

# Streams are iterators, and the first message is read eagerly

`Query`, `Join` and `Aggregate` are server-streaming. gRPC in Python does not
report a failure until the first message is read, so a stream that is
constructed and never iterated hides an access denial. The server, for its own
reasons, guarantees that the first message is always sent even when the result
is empty, because it carries `served_by` — so this client reads it in the
constructor. That turns "the read was refused" back into an exception at the
call site, and populates `served_by` before the first row.
"""

from __future__ import annotations

import contextlib
import dataclasses
import random
import time
import types
import uuid
from collections.abc import Callable, Iterable, Iterator, Sequence
from typing import Any, Final, TypeVar, cast

import grpc

from ._proto.slate.v1 import records_pb2 as pb
from ._proto.slate.v1 import records_pb2_grpc as pb_grpc
from .errors import Conflict, InvalidRequest, SlateError, from_batch_error, from_rpc_error
from .freshness import Freshness, ReadToken, ServedBy, Watermark
from .query import (
    AggregateQuery,
    Batch,
    DeleteWhere,
    GroupedJoinQuery,
    JoinQuery,
    Query,
    UpdateWhere,
)
from .rows import Group, JoinedRow, RelatedNode, Row, Step, _leaves, _nest
from .schema import Table, fingerprint_of
from .values import PyValue, from_value, to_value

__all__ = [
    "BatchOutcome",
    "BatchResult",
    "Client",
    "Explanation",
    "GroupStream",
    "Identity",
    "JoinExplanation",
    "JoinStream",
    "LeadershipStatus",
    "RowStream",
    "Session",
    "Transaction",
    "WriteResult",
]

T = TypeVar("T")

#: The metadata keys the shipped `MetadataIdentity` authenticator reads. They
#: are constants of `crates/slate-server/src/auth.rs`, not of this client — a
#: deployment with its own `Authenticator` will want different ones, which is
#: what `Identity.extra` is for.
PRINCIPAL_KEY: Final = "slate-principal"
TENANT_KEY: Final = "slate-tenant"
ROLES_KEY: Final = "slate-roles"
#: A caller's own label for one call, which the daemon's request log prints.
#:
#: Not an identity key and deliberately listed apart from the three above: the
#: server never decides anything from it, and a caller who forged one would be
#: forging a note to themselves. See `crates/slate-serverd/src/observe.rs`,
#: which filters it before it reaches a log line.
REQUEST_ID_KEY: Final = "slate-request-id"


class Identity:
    """Who the caller is, as transport metadata.

    The `.proto` has no principal field anywhere: "an identity in a request
    body is not an identity, it is a request to be trusted". So an identity is
    metadata, and what a deployment's `Authenticator` reads is that
    deployment's business. This class produces what the shipped
    `MetadataIdentity` reads; `extra` carries anything else — a bearer token, a
    mesh header — for a deployment that authenticates differently.

    A principal id is a typed value spelled the way `slate_tuple` parses it:
    `u64:1`, `str:alice`. That spelling is the server's, not this client's.
    """

    __slots__ = ("_metadata",)

    def __init__(
        self,
        principal: str | None = None,
        *,
        tenant: str | None = None,
        roles: Iterable[str] = (),
        extra: Iterable[tuple[str, str]] = (),
    ) -> None:
        metadata: list[tuple[str, str]] = []
        if principal is not None:
            metadata.append((PRINCIPAL_KEY, principal))
        if tenant is not None:
            metadata.append((TENANT_KEY, tenant))
        role_list = list(roles)
        if role_list:
            metadata.append((ROLES_KEY, ",".join(role_list)))
        metadata.extend(extra)
        self._metadata = tuple(metadata)

    @property
    def metadata(self) -> tuple[tuple[str, str], ...]:
        return self._metadata

    def __repr__(self) -> str:
        return f"Identity({dict(self._metadata)!r})"


class BatchOutcome:
    """One operation's outcome inside an independent batch.

    Exactly one of `written` and `error` is set, which is why they are two
    fields and not a `WriteResult` with an optional error: a value that can
    carry both is one somebody reads the wrong half of.
    """

    __slots__ = ("error", "written")

    def __init__(self, written: WriteResult | None, error: SlateError | None) -> None:
        #: What the operation did, or `None` if it failed.
        self.written = written
        #: Why it failed, or `None` if it did not.
        #:
        #: The same exception type the operation would have raised sent alone,
        #: built rather than caught — see `errors.from_batch_error`. It is
        #: returned rather than raised because the *request* succeeded, and the
        #: operations beside it landed.
        self.error = error

    @property
    def ok(self) -> bool:
        """Whether it succeeded."""
        return self.error is None

    @classmethod
    def from_proto(cls, result: pb.BatchResult, table: Table) -> BatchOutcome:
        which = result.WhichOneof("of")
        if which == "error":
            return cls(
                None,
                from_batch_error(
                    result.error.code, result.error.message, result.error.reason
                ),
            )
        sequence = result.ok.sequence if result.ok.HasField("sequence") else None
        return cls(
            WriteResult(
                None if sequence is None else ReadToken(sequence),
                result.ok.affected,
                [Row.from_proto(r, table) for r in result.ok.rows],
            ),
            None,
        )

    def __repr__(self) -> str:
        return f"BatchOutcome(ok={self.ok})"


class BatchResult:
    """What a batch returned."""

    __slots__ = ("outcomes", "sequence")

    def __init__(self, sequence: ReadToken | None, outcomes: list[BatchOutcome]) -> None:
        #: Where the writer got to, or `None` inside a transaction.
        self.sequence = sequence
        #: One per operation, in order — but **only** for an independent batch.
        #:
        #: Empty for `ALL_OR_NOTHING`, and that is not a missing feature: they
        #: all happened, or the request raised and none did. A list of
        #: successes would invite a caller to check it and could only ever say
        #: "yes".
        self.outcomes = outcomes

    def __len__(self) -> int:
        return len(self.outcomes)

    @property
    def failures(self) -> list[BatchOutcome]:
        """The outcomes that failed, which is usually the only interesting half."""
        return [one for one in self.outcomes if not one.ok]


class WriteResult:
    """What a write returned.

    `sequence` is set only for a write that committed — a single-statement
    write, which the server opens, commits and retries on conflict by itself.
    A write inside a transaction has no sequence until that transaction
    commits, and this is `None` there.

    `affected` is what the server counted. Read its docstring before believing
    it for an insert or an update.
    """

    __slots__ = ("affected", "rows", "sequence")

    def __init__(
        self,
        sequence: ReadToken | None,
        affected: int,
        rows: list[Row] | None = None,
    ) -> None:
        self.sequence = sequence
        #: The rows the write touched, when `returning()` asked for them.
        #:
        #: Empty unless asked, and empty on a write that matched nothing. Only
        #: the predicate writes can fill it: `insert` and `update` are given
        #: whole rows and this server applies no `DEFAULT` to them, so the row
        #: written is the row sent and returning it would hand back the
        #: request. A predicate write is the other case — the caller named a
        #: condition, and which rows matched is a fact it does not have.
        self.rows: list[Row] = rows or []
        #: How many rows the write acted on.
        #:
        #: For a **delete** this is how many existed — a row the caller's
        #: policy hides counts as absent, so it cannot be used to probe for
        #: one. For an **insert** and an **update** the server returns the
        #: number of rows it was *given*, not a number it discovered, because
        #: both refuse the whole batch rather than applying a prefix. So
        #: `affected == len(rows)` always holds there and the field carries no
        #: information. Reported as a protocol finding rather than hidden.
        self.affected = affected

    def __repr__(self) -> str:
        return f"WriteResult(sequence={self.sequence!r}, affected={self.affected})"


class Explanation:
    """A plan, as `Explain` describes it."""

    __slots__ = (
        "access",
        "decodes",
        "descending",
        "display",
        "estimated_cost",
        "estimated_rows",
        "index_only",
        "limit",
        "offset",
        "residual",
        "served_by",
        "sorts",
        "table",
        "warnings",
    )

    def __init__(self, wire: pb.ExplainResponse) -> None:
        self.table = wire.table
        self.access = wire.access
        #: The predicate re-checked on every candidate row, *including the
        #: mandatory security filter* — which is why it is worth returning: it
        #: is the proof that a policy reached the executor.
        self.residual = wire.residual
        self.descending = wire.descending
        #: The columns this plan decodes: the projection plus whatever the
        #: residual reads. Often the only visible difference between a grouped
        #: read's plan and the plan of the read it groups, since narrowing the
        #: projection need not change the access path.
        self.decodes = tuple(wire.decodes)
        self.estimated_rows = wire.estimated_rows
        self.estimated_cost = wire.estimated_cost
        self.limit = wire.limit if wire.HasField("limit") else None
        self.offset = wire.offset
        self.sorts = wire.sorts
        self.index_only = wire.index_only
        self.display = wire.display
        #: Things the server did that the request did not ask for — an unusable
        #: index hint ignored, a `build_limit` clamped. Only `Explain` and
        #: `ExplainJoin` carry these; a plain `Query` discards them.
        self.warnings = tuple(wire.warnings)
        self.served_by = ServedBy.from_proto(wire.served_by)

    def __repr__(self) -> str:
        return f"Explanation({self.display!r})"


def _algorithm_of(wire: pb.JoinInputPlan) -> str | None:
    """The join algorithm as a name, or `None` where the wire named none."""
    if not wire.HasField("algorithm"):
        return None
    which = wire.algorithm.WhichOneof("algorithm")
    if which == "hash_build":
        return "hash"
    if which == "nested_loop":
        return "nested loop"
    return None


class JoinInputPlan:
    """How one input of a join will be read."""

    __slots__ = ("algorithm", "estimated_cost", "estimated_rows", "join_type", "plan")

    def __init__(self, wire: pb.JoinInputPlan) -> None:
        self.plan = Explanation(wire.plan)
        self.join_type = wire.join_type
        #: `"hash"`, `"nested loop"`, or `None` on the first input, which
        #: nothing is joined to.
        #:
        #: A string, and *these* strings, because the three clients have to
        #: spell it the same. This used to hand back the raw protobuf message
        #: while Go returned `"hash"` and TypeScript returned whichever key
        #: `proto-loader` gave the `oneof` — three answers to one question,
        #: which is the failure the conformance runner exists to catch and
        #: which nothing had asked either client about.
        self.algorithm = _algorithm_of(wire)
        self.estimated_rows = wire.estimated_rows
        self.estimated_cost = wire.estimated_cost


class JoinExplanation:
    """How each input of a join will be read, and how each is combined."""

    __slots__ = ("display", "estimated_cost", "estimated_rows", "inputs", "served_by", "warnings")

    def __init__(self, wire: pb.JoinExplainResponse) -> None:
        self.inputs = tuple(JoinInputPlan(i) for i in wire.inputs)
        self.estimated_rows = wire.estimated_rows
        self.estimated_cost = wire.estimated_cost
        self.display = wire.display
        self.warnings = tuple(wire.warnings)
        self.served_by = ServedBy.from_proto(wire.served_by)

    def __repr__(self) -> str:
        return f"JoinExplanation({self.display!r})"


class AggregateExplanation:
    """The plan a grouped read would run under.

    Exactly one of `input` and `join` is set, matching the request: `input` for
    an aggregate over one table, `join` for one over a join or a chain.

    Separate from `Explanation` and `JoinExplanation` because grouping changes
    the plan — each input's projection becomes the group keys plus what the
    aggregates read — so explaining the underlying read describes something the
    aggregate will not run.
    """

    __slots__ = ("display", "input", "join", "served_by", "warnings")

    def __init__(self, wire: pb.AggregateExplainResponse) -> None:
        self.input = Explanation(wire.input) if wire.HasField("input") else None
        self.join = JoinExplanation(wire.join) if wire.HasField("join") else None
        self.display = wire.display
        self.warnings = tuple(wire.warnings)
        self.served_by = ServedBy.from_proto(wire.served_by)

    def __repr__(self) -> str:
        return f"AggregateExplanation({self.display!r})"


class LeadershipStatus:
    """What a node believes about who may write.

    Believes, not knows: a node that has lost the lease without noticing still
    reports itself the leader, and the storage layer's fence — not this — is
    what stops it writing.
    """

    __slots__ = ("generation", "holder", "standing", "stepped_down_because")

    #: `pb.LeadershipStatus.Standing`.
    FOLLOWER: Final = pb.LeadershipStatus.STANDING_FOLLOWER
    LEADER: Final = pb.LeadershipStatus.STANDING_LEADER
    STEPPED_DOWN: Final = pb.LeadershipStatus.STANDING_STEPPED_DOWN

    def __init__(self, wire: pb.LeadershipStatus) -> None:
        self.standing = wire.standing
        self.generation = wire.generation if wire.HasField("generation") else None
        self.holder = wire.holder
        self.stepped_down_because = wire.stepped_down_because

    @property
    def is_leader(self) -> bool:
        return self.standing == self.LEADER

    def __repr__(self) -> str:
        name = pb.LeadershipStatus.Standing.Name(self.standing)
        return f"LeadershipStatus({name}, holder={self.holder!r})"


# --- streams ----------------------------------------------------------------


class _Stream(Iterator[T]):
    """A server stream, with its first message already read.

    See the module docstring for why the first message is eager. The batch
    boundary is not exposed: the server chooses it (256 rows by argument, not
    measurement), it is not part of the answer, and a caller iterating rows
    should not have to know it exists.
    """

    __slots__ = ("_buffer", "_call", "_done", "_request_id", "_served_by")

    def __init__(self, call: Iterator[object], request_id: str = "") -> None:
        self._call = call
        # Kept for the whole stream, because a scan can fail on its tenth
        # message as easily as its first and the id names the same call either
        # way. The server logged one line for this stream, at its head.
        self._request_id = request_id
        self._buffer: list[T] = []
        self._done = False
        self._served_by: ServedBy | None = None
        self._pump()

    def _decode(self, message: object) -> tuple[list[T], pb.ServedBy]:  # pragma: no cover
        raise NotImplementedError

    def _pump(self) -> None:
        """Read one message into the buffer."""
        while not self._buffer and not self._done:
            try:
                message = next(self._call)
            except StopIteration:
                self._done = True
                return
            except grpc.RpcError as error:
                self._done = True
                raise from_rpc_error(error, self._request_id) from error
            items, served_by = self._decode(message)
            if self._served_by is None:
                self._served_by = ServedBy.from_proto(served_by)
            self._buffer.extend(items)

    @property
    def served_by(self) -> ServedBy | None:
        """Which view served this read.

        Available before the first row: the server always sends a first
        message, even for an empty result, because it carries this.
        """
        return self._served_by

    def __iter__(self) -> Iterator[T]:
        return self

    def __next__(self) -> T:
        self._pump()
        if not self._buffer:
            raise StopIteration
        return self._buffer.pop(0)

    def cancel(self) -> None:
        """Stop reading and free the server's task.

        A stream abandoned without this keeps a cursor and a spawned task alive
        on the head node until the channel notices.
        """
        cancel = getattr(self._call, "cancel", None)
        if cancel is not None:
            with contextlib.suppress(Exception):
                cancel()
        self._done = True

    def __enter__(self) -> _Stream[T]:
        return self

    def __exit__(self, *exc: object) -> None:
        self.cancel()


@dataclasses.dataclass(frozen=True)
class Page:
    """One page of a keyset-paged read, and where to resume.

    `cursor` is `None` when this page was short and there is provably nothing
    after it. A page that comes back *full* might be the last one, and the only
    way to know is to ask again: this returns a cursor anyway rather than
    reading one row further to find out, because that extra read would be paid
    on every page to save one empty request at the end of a sequence most
    callers never finish. So a caller looping until `is_last` makes one final
    request that returns nothing, which is the ordinary shape of every cursor
    API and is documented rather than optimised away.
    """

    rows: list[Row]
    cursor: list[PyValue] | None
    served_by: ServedBy | None

    @property
    def is_last(self) -> bool:
        """Whether there is provably nothing after this page."""
        return self.cursor is None


@dataclasses.dataclass(frozen=True)
class JoinPage:
    """One page of a keyset-paged join or chain, and where to resume.

    `cursor` is a primary key of the **first input's** table, because a page of
    a join is a page of its driving table. `rows` holds every joined row those
    input-0 rows produced, so it is usually longer than the page size — a page
    of 20 with a fan-out of 3 is about 60 rows, and 20 is how far the cursor
    advanced.

    `cursor` is `None` when the page was short and there is provably nothing
    after it, where "short" is measured in input-0 rows for the same reason.
    """

    rows: list[JoinedRow]
    cursor: list[PyValue] | None
    served_by: ServedBy | None

    @property
    def is_last(self) -> bool:
        """Whether there is provably nothing after this page."""
        return self.cursor is None


class RowStream(_Stream[Row]):
    """The rows of a query."""

    __slots__ = ("_table",)

    def __init__(
        self, call: tuple[Iterator[object], str], table: Table | None
    ) -> None:
        self._table = table
        super().__init__(*call)

    def _decode(self, message: object) -> tuple[list[Row], pb.ServedBy]:
        response = cast(pb.QueryResponse, message)
        return [Row.from_proto(r, self._table) for r in response.rows], response.served_by


class JoinStream(_Stream[JoinedRow]):
    """The rows of a join or chain."""

    __slots__ = ("_tables",)

    def __init__(
        self, call: tuple[Iterator[object], str], tables: Sequence[Table | None]
    ) -> None:
        self._tables = tuple(tables)
        super().__init__(*call)

    def _decode(self, message: object) -> tuple[list[JoinedRow], pb.ServedBy]:
        response = cast(pb.JoinResponse, message)
        return (
            [JoinedRow.from_proto(r, self._tables) for r in response.rows],
            response.served_by,
        )


class GroupStream(_Stream[Group]):
    """The groups of an aggregate query.

    In the kernel's own order: ascending by the encoded group key. There is no
    `ORDER BY` over groups and none is approximated here.
    """

    def _decode(self, message: object) -> tuple[list[Group], pb.ServedBy]:
        response = cast(pb.AggregateResponse, message)
        return [Group.from_proto(g) for g in response.groups], response.served_by


# --- the operations ---------------------------------------------------------


class _Connection:
    """A channel, a stub and an identity. Shared by every session on it."""

    __slots__ = ("channel", "identity", "owns_channel", "stub")

    def __init__(
        self, channel: grpc.Channel, identity: Identity, owns_channel: bool
    ) -> None:
        self.channel = channel
        self.stub = pb_grpc.RecordsStub(channel)
        self.identity = identity
        self.owns_channel = owns_channel


class _Ops:
    """Every operation, written once.

    `Session` and `Transaction` differ in exactly two ways — which transaction
    id goes on the request, and what happens to a `freshness` argument — so
    those are the two abstract methods and nothing else is duplicated. Writing
    the operations twice is how the two come to disagree about, say, whether a
    delete's key is validated.
    """

    _conn: _Connection

    #: Seconds allowed for each call, or `None` for no deadline.
    #:
    #: A class attribute so that every `_Ops` has one whether or not its
    #: `__init__` set it, which is what keeps `Transaction` — constructed from a
    #: `Session` rather than from arguments — from silently losing it.
    _timeout: float | None = None

    def _transaction_id(self) -> str:  # pragma: no cover - overridden
        raise NotImplementedError

    def _freshness(self, asked: Freshness | None) -> pb.Freshness | None:  # pragma: no cover
        raise NotImplementedError

    def _observe(self, sequence: int | None) -> None:  # pragma: no cover
        raise NotImplementedError

    def _observe_read(self, served_by: ServedBy | None) -> None:  # pragma: no cover
        raise NotImplementedError

    # --- plumbing ---------------------------------------------------------

    def _sending(self) -> tuple[str, tuple[tuple[str, str], ...]]:
        """An id for one call, and the metadata carrying it.

        A fresh id per *call*, not per session or per connection: the point of
        it is to name one line in the server's log, and a session-wide id names
        every line the session produced, which is the thing a caller already
        has. A `uuid4` rather than a counter because two processes sharing a
        counter's namespace would collide in exactly the log somebody is
        reading to tell them apart, and `hex` rather than the dashed form to
        keep the log column narrow.

        The identity metadata is built once and cached; this appends to it,
        which costs one tuple per call. That is the price of the id being
        per-call and it is the right trade — an id that cost nothing and named
        nothing would not be worth sending.
        """
        request_id = uuid.uuid4().hex
        return request_id, (
            *self._conn.identity.metadata,
            (REQUEST_ID_KEY, request_id),
        )

    def _unary(self, method: Callable[..., T], request: object) -> T:
        request_id, metadata = self._sending()
        try:
            return method(request, metadata=metadata, timeout=self._timeout)
        except grpc.RpcError as error:
            raise from_rpc_error(error, request_id) from error

    def _stream(
        self, method: Callable[..., Iterator[object]], request: object
    ) -> tuple[Iterator[object], str]:
        # The call object is returned without a round trip; the failure arrives
        # when the first message is read, which `_Stream` does eagerly.
        #
        # The deadline covers the *whole stream*, not each message — a scan that
        # returns rows steadily for longer than the timeout is cancelled
        # part-way. That is what gRPC deadlines mean, and it is the reason
        # `with_timeout` exists rather than one value for the connection: a
        # point get and a hundred-thousand-row scan do not want the same number.
        request_id, metadata = self._sending()
        return method(request, metadata=metadata, timeout=self._timeout), request_id

    def _rows_proto(self, table: Table, rows: Iterable[Sequence[PyValue] | Row]) -> list[pb.Row]:
        types = table.column_types()
        out: list[pb.Row] = []
        for row in rows:
            values = list(row.values) if isinstance(row, Row) else list(row)
            if len(values) != len(types):
                raise ValueError(
                    f"table `{table.name}` declares {len(types)} columns and this row "
                    f"has {len(values)}. A row on the wire is one value per column, in "
                    "ordinal order."
                )
            out.append(
                pb.Row(values=[to_value(v, t) for v, t in zip(values, types, strict=True)])
            )
        return out

    def _key_proto(self, table: Table, key: Sequence[PyValue]) -> pb.Row:
        types = table.key_types()
        if len(key) != len(types):
            raise ValueError(
                f"the primary key of `{table.name}` is "
                f"{', '.join(table.primary_key)} — {len(types)} values, not {len(key)}"
            )
        return pb.Row(values=[to_value(v, t) for v, t in zip(key, types, strict=True)])

    # --- writes -----------------------------------------------------------

    @staticmethod
    def _schema_check(table: Table) -> pb.SchemaCheck:
        """This client's claim about `table`, for the server to disagree with.

        Sent on every request that names a table rather than once at connect:
        a connection outlives a rolling deployment, and a check made only at
        connect would pass against the node it happened to reach and then be
        wrong for the rest of the session. Per-request costs 12 bytes and
        cannot go stale. See `fingerprint_of`.
        """
        return pb.SchemaCheck(
            columns=table.width, fingerprint=fingerprint_of(table)
        )

    def insert(
        self,
        table: Table,
        rows: Iterable[Sequence[PyValue] | Row],
        *,
        upsert: bool = False,
    ) -> WriteResult:
        """Insert rows. `upsert` replaces a row already at the key.

        Outside a transaction this is a single-statement transaction the server
        opens, commits and — on a conflict — retries by itself. Inside one it
        is part of that transaction and has no sequence until it commits.
        """
        response = self._unary(
            self._conn.stub.Insert,
            pb.InsertRequest(
                transaction=self._transaction_id(),
                table=table.name,
                rows=self._rows_proto(table, rows),
                upsert=upsert,
                schema=self._schema_check(table),
            ),
        )
        return self._write_result(response)

    def update(
        self, table: Table, rows: Iterable[Sequence[PyValue] | Row]
    ) -> WriteResult:
        """Replace rows by primary key.

        The whole batch is refused if any row is missing, rather than a prefix
        being applied.
        """
        response = self._unary(
            self._conn.stub.Update,
            pb.UpdateRequest(
                transaction=self._transaction_id(),
                table=table.name,
                rows=self._rows_proto(table, rows),
                schema=self._schema_check(table),
            ),
        )
        return self._write_result(response)

    def delete(
        self, table: Table, primary_keys: Iterable[Sequence[PyValue]]
    ) -> WriteResult:
        """Delete by primary key. `affected` is how many existed."""
        response = self._unary(
            self._conn.stub.Delete,
            pb.DeleteRequest(
                transaction=self._transaction_id(),
                table=table.name,
                primary_keys=[self._key_proto(table, k) for k in primary_keys],
                schema=self._schema_check(table),
            ),
        )
        return self._write_result(response)

    def _write_result(
        self, response: pb.WriteResponse, table: Table | None = None
    ) -> WriteResult:
        sequence = response.sequence if response.HasField("sequence") else None
        self._observe(sequence)
        # Decoded against the table the write named, which is the only table a
        # write touches. A response carrying rows for a write that cannot
        # return any would decode into nothing here; the server does not send
        # them, and `table=None` says this call cannot receive them.
        rows = (
            [Row.from_proto(r, table) for r in response.rows]
            if table is not None
            else []
        )
        return WriteResult(
            None if sequence is None else ReadToken(sequence),
            response.affected,
            rows,
        )

    def batch(self, batch: Batch) -> BatchResult:
        """Several writes in one round trip.

        With `Atomicity.INDEPENDENT` the result carries one outcome per
        operation, and a failed operation is one of those outcomes rather than
        an exception — the *request* succeeded. With `ALL_OR_NOTHING` the
        result carries none, and a failure raises, because the operations
        before it did not happen either.
        """
        if not batch._operations:
            raise InvalidRequest(
                "a batch needs at least one operation",
                code=grpc.StatusCode.INVALID_ARGUMENT,
            )
        operations = [self._operation_proto(kind, payload) for kind, payload in batch._operations]
        response = self._unary(
            self._conn.stub.Batch,
            pb.BatchRequest(
                operations=operations,
                atomicity=batch._atomicity.value,
                transaction=self._transaction_id(),
            ),
        )
        sequence = response.sequence if response.HasField("sequence") else None
        self._observe(sequence)
        # The table each operation named, in order, so a returned row decodes
        # against the right one. A batch may span tables, so a single table on
        # the result would be wrong for all but the first.
        tables = [self._operation_table(kind, payload) for kind, payload in batch._operations]
        outcomes = [
            BatchOutcome.from_proto(result, table)
            for result, table in zip(response.results, tables, strict=False)
        ]
        return BatchResult(
            None if sequence is None else ReadToken(sequence), outcomes
        )

    @staticmethod
    def _operation_table(kind: str, payload: Any) -> Table:
        """Which table an operation touches, for decoding its returned rows.

        `payload: Any` rather than `object`, matching `_operation_proto` below
        and matching `Batch._operations`, which is a `list[tuple[str, Any]]` at
        the source: the payload's shape is decided by `kind`, and no annotation
        short of a tagged union says that.

        It used to be `object` with two `type: ignore` comments, and neither
        suppressed the error it was written for — mypy reported an uncovered
        `no-any-return` and an uncovered `attr-defined` *through* them. An
        ignore that does not match is worse than no ignore: it reads as a
        considered exception and silences nothing.
        """
        if kind in ("delete_where", "update_where"):
            return cast(Table, payload.table)
        return cast(Table, payload[0])

    def _operation_proto(self, kind: str, payload: Any) -> pb.BatchOperation:
        """One operation, in the same request message the lone RPC sends.

        The same messages rather than batch-specific ones, so a batched insert
        and a lone insert are the same bytes. `transaction` is left unset on
        every one of them: it has no meaning inside a batch and the server
        refuses it, which is the loud version of ignoring it.
        """
        if kind == "insert":
            table, rows, upsert = payload
            return pb.BatchOperation(
                insert=pb.InsertRequest(
                    table=table.name,
                    rows=self._rows_proto(table, rows),
                    upsert=upsert,
                    schema=self._schema_check(table),
                )
            )
        if kind == "update":
            table, rows = payload
            return pb.BatchOperation(
                update=pb.UpdateRequest(
                    table=table.name,
                    rows=self._rows_proto(table, rows),
                    schema=self._schema_check(table),
                )
            )
        if kind == "delete":
            table, keys = payload
            return pb.BatchOperation(
                delete=pb.DeleteRequest(
                    table=table.name,
                    primary_keys=[self._key_proto(table, key) for key in keys],
                    schema=self._schema_check(table),
                )
            )
        if kind == "delete_where":
            return pb.BatchOperation(
                delete_where=pb.DeleteWhereRequest(
                    table=payload.table.name,
                    filter=payload._filter.to_proto() if payload._filter else None,
                    returning=payload._returning,
                    schema=self._schema_check(payload.table),
                )
            )
        if kind == "update_where":
            return pb.BatchOperation(
                update_where=pb.UpdateWhereRequest(
                    table=payload.table.name,
                    filter=payload._filter.to_proto() if payload._filter else None,
                    assignments=[
                        pb.Assignment(column=column.to_proto(), value=value.to_proto())
                        for column, value in payload._assignments
                    ],
                    returning=payload._returning,
                    schema=self._schema_check(payload.table),
                )
            )
        raise AssertionError(f"unknown batch operation {kind!r}")

    def delete_where(self, write: DeleteWhere) -> WriteResult:
        """Delete every row the predicate selects, in one statement.

        With `returning()`, `result.rows` are the rows as they were before
        removal — the only moment they exist to be read.
        """
        response = self._unary(
            self._conn.stub.DeleteWhere,
            pb.DeleteWhereRequest(
                transaction=self._transaction_id(),
                table=write.table.name,
                filter=write._filter.to_proto() if write._filter else None,
                returning=write._returning,
                schema=self._schema_check(write.table),
            ),
        )
        return self._write_result(response, write.table)

    def update_where(self, write: UpdateWhere) -> WriteResult:
        """Assign to columns of every row the predicate selects.

        With `returning()`, `result.rows` are the rows **as written**.
        """
        response = self._unary(
            self._conn.stub.UpdateWhere,
            pb.UpdateWhereRequest(
                transaction=self._transaction_id(),
                table=write.table.name,
                filter=write._filter.to_proto() if write._filter else None,
                assignments=[
                    pb.Assignment(column=column.to_proto(), value=value.to_proto())
                    for column, value in write._assignments
                ],
                returning=write._returning,
                schema=self._schema_check(write.table),
            ),
        )
        return self._write_result(response, write.table)

    # --- reads ------------------------------------------------------------

    def get(
        self,
        table: Table,
        primary_key: Sequence[PyValue],
        *,
        freshness: Freshness | None = None,
    ) -> Row | None:
        """One row by primary key, or `None`.

        A row the caller's policy hides also reads as absent, deliberately: the
        result must not distinguish "not there" from "not yours".
        """
        request = pb.GetRequest(
            transaction=self._transaction_id(),
            table=table.name,
            primary_key=self._key_proto(table, primary_key),
            schema=self._schema_check(table),
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        response = self._unary(self._conn.stub.Get, request)
        self._observe_read(ServedBy.from_proto(response.served_by))
        if not response.found:
            return None
        return Row.from_proto(response.row, table)

    def related(
        self,
        table: Table,
        keys: Sequence[PyValue],
        *,
        through: str,
        on: Table,
        children: bool = True,
        freshness: Freshness | None = None,
    ) -> list[list[Row]]:
        """Load one relationship for many parent rows, in a single read.

        Returns a list the same length as `keys`: entry `i` is the rows related
        to `keys[i]`. A key with nothing related to it gets an empty list, not a
        missing entry, so the result is indexable by the caller's own loop
        counter.

        The relationship is named by the foreign key that already declares it,
        rather than by anything the client holds:

        - `on` is the table that holds the foreign key — always the child,
          whichever way the relationship is being read.
        - `through` is that key's name.
        - `children=True` reads the rows holding the key (an author's books);
          `children=False` reads the rows it points at (a book's author).
        - `table` is the table the rows come back as, which is `on` for
          children and the key's parent for parents. It is passed separately
          because the client decodes against it and does not hold the catalog.

        Duplicate keys are expected and are the point. Two books by one author
        send the same value twice, the server reads it once, and both books map
        onto the one group that comes back — so this is one read whatever the
        number of parents, which is the whole reason it exists.
        """
        request = pb.RelatedRequest(
            transaction=self._transaction_id(),
            relation=pb.Relation(
                table=on.name,
                foreign_key=through,
                direction=(
                    pb.Relation.Direction.CHILDREN
                    if children
                    else pb.Relation.Direction.PARENTS
                ),
            ),
            keys=[to_value(key) for key in keys],
            schema=self._schema_check(table),
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        response = self._unary(self._conn.stub.Related, request)
        self._observe_read(ServedBy.from_proto(response.served_by))

        # Keyed by the serialised value, not by the Python object: the server
        # groups on the kernel's own equality and the client has to agree with
        # it, and `1` and `1.0` are one key in Python and two on the wire.
        grouped: dict[bytes, list[Row]] = {}
        for group in response.groups:
            grouped[group.key.SerializeToString(deterministic=True)] = [
                Row.from_proto(row, table) for row in group.rows
            ]
        return [
            grouped.get(to_value(key).SerializeToString(deterministic=True), [])
            for key in keys
        ]

    def related_path(
        self,
        keys: Sequence[PyValue],
        path: Sequence[Step],
        *,
        freshness: Freshness | None = None,
    ) -> list[list[RelatedNode]]:
        """Walk a path of relationships for many parent rows, in one request.

        Returns a list the same length as `keys`: entry `i` is the first
        level's rows for `keys[i]`, each carrying its own next level, and so on
        down the path. A key that related to nothing gets an empty list.

        ```python
        # Every article's tags, through the join table, in one call.
        trees = session.related_path(
            [a[0] for a in articles],
            [
                Step(on=ARTICLE_TAGS, through="at_article", table=ARTICLE_TAGS),
                Step(on=ARTICLE_TAGS, through="at_tag", table=TAGS, children=False),
            ],
        )
        for article, tree in zip(articles, trees):
            tags = [leaf.row for join in tree for leaf in join.related]
        ```

        **One read per level, whatever the number of parents.** Each step's key
        set is the previous step's rows, deduplicated by the server, so a
        hundred articles and a thousand join rows are still two reads. Doing
        this from the client — one `related` call per level, regrouped by hand
        — is the same shape as the N+1 this whole layer exists to prevent, one
        level up, and it is what callers of every SDK had to write until this
        existed.

        The path is bounded by the server's `max_relation_depth`, because one
        step is one read and the depth arrives in the request. A path past it
        is refused with `InvalidRequest` naming the limit.
        """
        if not path:
            raise InvalidRequest(
                "a path needs at least one step; use `related` for one relationship",
                code=grpc.StatusCode.INVALID_ARGUMENT,
            )
        request = pb.RelatedRequest(
            transaction=self._transaction_id(),
            keys=[to_value(key) for key in keys],
            path=[
                pb.RelatedStep(
                    relation=pb.Relation(
                        table=step.on.name,
                        foreign_key=step.through,
                        direction=(
                            pb.Relation.Direction.CHILDREN
                            if step.children
                            else pb.Relation.Direction.PARENTS
                        ),
                    ),
                    schema=self._schema_check(step.table),
                )
                for step in path
            ],
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        response = self._unary(self._conn.stub.Related, request)
        self._observe_read(ServedBy.from_proto(response.served_by))
        return _nest(response, path, keys)

    def related_through(
        self,
        keys: Sequence[PyValue],
        path: Sequence[Step],
        *,
        freshness: Freshness | None = None,
    ) -> list[list[Row]]:
        """A path's *far* rows per parent, with the levels between dropped.

        The many-to-many: an article's tags, where the join rows exist only to
        connect the two and the caller has no use for them. Exactly
        `related_path` with the intermediate levels flattened away, and one
        request either way — which is why it is written on top of it rather
        than beside it, as `slate-orm`'s own `load_related_through` is written
        on top of `load_nested`. Two regroupings of one shape is two places for
        an off-by-one to live.

        Duplicates are kept and are not an error: two join rows pointing at one
        tag give that tag twice, because the caller is the one who knows
        whether that means anything. Deduplicating here would quietly discard a
        row a caller may be counting.
        """
        return [
            list(_leaves(tree, len(path) - 1))
            for tree in self.related_path(keys, path, freshness=freshness)
        ]

    def query(self, query: Query, *, freshness: Freshness | None = None) -> RowStream:
        """Run a query. The stream's first message is read before this returns."""
        request = pb.QueryRequest(
            transaction=self._transaction_id(), query=query.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        stream = RowStream(self._stream(self._conn.stub.Query, request), query.table)
        self._observe_read(stream.served_by)
        return stream

    def page(self, query: Query, *, freshness: Freshness | None = None) -> Page:
        """One page of a keyset-paged read, with the cursor for the next.

        `query.limit` must be set: a page with no size is the whole table, and
        the cursor it would return names its last row. Pass `page.cursor` to
        `Query.after` for the page after this one, and stop when it is `None`.

        ```python
        cursor = None
        while True:
            page = session.page(Query(BOOKS).limit(100).after(cursor))
            for row in page.rows:
                ...
            if page.is_last:
                break
            cursor = page.cursor
        ```

        Not `offset`, which counts rows and is only correct while nothing
        changes: delete a row ahead of the cursor between two pages and the
        reader silently skips one, insert one and they see a row twice, and
        nothing reports either.

        The whole page is read before this returns, unlike `query`, which
        streams. A page is bounded by its own limit, so there is nothing to
        stream *to* -- and the cursor arrives at the end, so a caller would
        have to drain the stream before it could ask for the next page anyway.
        """
        # Set on the request rather than on `query`, which belongs to the
        # caller: `page(q)` followed by `query(q)` must not have quietly turned
        # `q` into a paged read.
        wire = query.to_proto()
        wire.paged = True
        request = pb.QueryRequest(transaction=self._transaction_id(), query=wire)
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)

        rows: list[Row] = []
        cursor: list[PyValue] | None = None
        served_by: ServedBy | None = None
        # Wrapped as `_Stream._pump` wraps it: a refusal arrives as the
        # stream's first error, and an unwrapped one reaches the caller as a
        # `grpc.RpcError` that `except SlateError` cannot catch. The server's
        # refusals are half of what paging is -- a page it cannot build a
        # cursor for -- so a caller that could not catch them would have
        # nothing to handle.
        # Set before the `try`, because `_stream` is inside it: if opening the
        # call is what fails, there is no id to report and `""` says so.
        request_id = ""
        try:
            call, request_id = self._stream(self._conn.stub.Query, request)
            for message in call:
                response = cast(pb.QueryResponse, message)
                if served_by is None:
                    served_by = ServedBy.from_proto(response.served_by)
                rows.extend(Row.from_proto(r, query.table) for r in response.rows)
                # On whichever message carries it, not on the last one: a page
                # whose rows divide evenly into batches gets a trailing message
                # with the cursor and no rows, and one that does not gets it on
                # the final batch.
                if response.next_cursor:
                    cursor = [from_value(v) for v in response.next_cursor]
        except grpc.RpcError as error:
            raise from_rpc_error(error, request_id) from error
        self._observe_read(served_by)
        return Page(rows=rows, cursor=cursor, served_by=served_by)

    def join(self, join: JoinQuery, *, freshness: Freshness | None = None) -> JoinStream:
        """Run a join or chain.

        One routing decision and one snapshot however many tables it reads, so
        the freshness asked for applies to the whole result and there is one
        `served_by`.
        """
        request = pb.JoinRequest(
            transaction=self._transaction_id(), join=join.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        tables = [i.table for i in join.inputs]
        stream = JoinStream(self._stream(self._conn.stub.Join, request), tables)
        self._observe_read(stream.served_by)
        return stream

    def page_join(
        self, join: JoinQuery, *, freshness: Freshness | None = None
    ) -> JoinPage:
        """One page of a keyset-paged join or chain, with the cursor for the next.

        `join.limit` must be set, and it counts **input-0 rows**: a page of a
        join is a page of its driving table, and every joined row those rows
        produce comes back with them.

        ```python
        cursor = None
        while True:
            page = session.page_join(q.limit(100).after(cursor))
            for row in page.rows:
                ...
            if page.is_last:
                break
            cursor = page.cursor
        ```

        Every joined row derives from exactly one input-0 row, so this visits
        every joined row exactly once even while rows are being inserted and
        deleted — which `offset` does not, because it counts.

        The page is read whole before this returns, as `page` does and for the
        same reason: the cursor arrives at the end, so a caller would have to
        drain a stream before it could ask for the next page anyway. Note that
        the page is bounded in input-0 rows, not in returned rows.
        """
        wire = join.to_proto()
        wire.paged = True
        request = pb.JoinRequest(transaction=self._transaction_id(), join=wire)
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)

        tables = [i.table for i in join.inputs]
        rows: list[JoinedRow] = []
        cursor: list[PyValue] | None = None
        served_by: ServedBy | None = None
        # Wrapped as `page` wraps it: the server's refusals are half of what
        # paging is — a right outer join, an offset alongside a cursor — and an
        # unwrapped one reaches the caller as a `grpc.RpcError` that
        # `except SlateError` cannot catch.
        # Set before the `try`, because `_stream` is inside it: if opening the
        # call is what fails, there is no id to report and `""` says so.
        request_id = ""
        try:
            call, request_id = self._stream(self._conn.stub.Join, request)
            for message in call:
                response = cast(pb.JoinResponse, message)
                if served_by is None:
                    served_by = ServedBy.from_proto(response.served_by)
                rows.extend(JoinedRow.from_proto(r, tables) for r in response.rows)
                if response.next_cursor:
                    cursor = [from_value(v) for v in response.next_cursor]
        except grpc.RpcError as error:
            raise from_rpc_error(error, request_id) from error
        self._observe_read(served_by)
        return JoinPage(rows=rows, cursor=cursor, served_by=served_by)

    def aggregate(
        self,
        aggregate: AggregateQuery | GroupedJoinQuery,
        *,
        freshness: Freshness | None = None,
    ) -> GroupStream:
        """Run a grouped aggregate, over one table or over a join.

        One method for both because the response is identical — a stream of
        `Group` — and a second one would duplicate the streaming, the
        batching, the `served_by` header and the warnings to vary one field of
        the request.
        """
        request = pb.AggregateRequest(
            transaction=self._transaction_id(), aggregate=aggregate.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        stream = GroupStream(*self._stream(self._conn.stub.Aggregate, request))
        self._observe_read(stream.served_by)
        return stream

    def explain(self, query: Query, *, freshness: Freshness | None = None) -> Explanation:
        """The plan a query would run under, without running it."""
        request = pb.ExplainRequest(
            transaction=self._transaction_id(), query=query.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        return Explanation(self._unary(self._conn.stub.Explain, request))

    def explain_join(
        self, join: JoinQuery, *, freshness: Freshness | None = None
    ) -> JoinExplanation:
        """The plan a join would run under."""
        request = pb.ExplainJoinRequest(
            transaction=self._transaction_id(), join=join.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        return JoinExplanation(self._unary(self._conn.stub.ExplainJoin, request))

    def explain_aggregate(
        self, aggregate: AggregateQuery | GroupedJoinQuery, *, freshness: Freshness | None = None
    ) -> AggregateExplanation:
        """The plan a grouped read would run under.

        Not `explain` or `explain_join` on the underlying read: grouping
        narrows each input's projection to the group keys and the aggregates'
        columns, which is what lets an index answer `COUNT(*)` without touching
        a row. Comparing `decodes` between the two is how you see it where the
        access path does not change.
        """
        request = pb.ExplainAggregateRequest(
            transaction=self._transaction_id(), aggregate=aggregate.to_proto()
        )
        wire_freshness = self._freshness(freshness)
        if wire_freshness is not None:
            request.freshness.CopyFrom(wire_freshness)
        return AggregateExplanation(
            self._unary(self._conn.stub.ExplainAggregate, request)
        )


class Session(_Ops):
    """A freshness scope.

    Holds the watermark: the highest sequence this caller has observed, from
    its own commits and — when `monotonic_reads` is on — from the views that
    served its reads. Every read carries it as `Freshness.at_least`, which is
    what makes a write followed by a read see the write without the caller
    naming a token.
    """

    def __init__(
        self,
        conn: _Connection,
        *,
        monotonic_reads: bool = True,
        timeout: float | None = None,
        _watermark: Watermark | None = None,
    ) -> None:
        self._conn = conn
        # Shared by reference when one is passed, which is what lets
        # `with_timeout` return a *different* session over the *same* freshness
        # scope. Copying it would mean a write through one view and a read
        # through the other could not see each other, which is the bug this
        # class exists to prevent.
        self._watermark = Watermark() if _watermark is None else _watermark
        self._monotonic_reads = monotonic_reads
        self._timeout = timeout

    def with_timeout(self, seconds: float | None) -> Session:
        """This session's operations, under a different per-call deadline.

        A new object rather than a mutable setting, so it is safe to hand one
        view to a background thread while another is in use — and so a caller
        cannot leave a short deadline switched on by forgetting to restore it.
        The freshness scope is shared, so a write through one view is visible to
        a read through the other.

        `None` means no deadline, which is gRPC's default and this client's,
        and is only the right answer when something else bounds the call.
        """
        return Session(
            self._conn,
            monotonic_reads=self._monotonic_reads,
            timeout=seconds,
            _watermark=self._watermark,
        )

    # --- freshness --------------------------------------------------------

    @property
    def watermark(self) -> ReadToken | None:
        """The highest sequence this session has observed, or `None`.

        Readable so that a caller who wants to reason about staleness can, and
        so that a watermark can be handed to another session — a job queue
        passing "read at least what I wrote" to a worker, which is the case
        this model exists for and which no amount of implicit threading covers.
        """
        return self._watermark.token

    def observe(self, token: ReadToken | None) -> None:
        """Fold another session's token into this one's watermark.

        The other half of `watermark`: a worker that was handed a token from
        the process that enqueued its job calls this, and every subsequent read
        in the session is at least that fresh.
        """
        self._watermark.observe(None if token is None else token.sequence)

    def _transaction_id(self) -> str:
        return ""

    def _freshness(self, asked: Freshness | None) -> pb.Freshness | None:
        if asked is not None:
            return asked.to_proto()
        implied = self._watermark.freshness()
        return None if implied is None else implied.to_proto()

    def _observe(self, sequence: int | None) -> None:
        self._watermark.observe(sequence)

    def _observe_read(self, served_by: ServedBy | None) -> None:
        if self._monotonic_reads and served_by is not None:
            self._watermark.observe(served_by.sequence)

    # --- transactions -----------------------------------------------------

    @contextlib.contextmanager
    def transaction(self) -> Iterator[Transaction]:
        """Open a transaction, commit it on a clean exit, roll it back on any error.

        A conflict is *not* retried here: retrying would mean running the body
        again, and a context manager cannot re-run its own block. Use
        `transact` for that, which takes the body as a function precisely so it
        can be run twice.
        """
        response = self._unary(self._conn.stub.Begin, pb.BeginRequest())
        transaction = Transaction(self, response.transaction)
        try:
            yield transaction
        except BaseException:
            # Rollback is best effort: the transaction may already be gone —
            # the server rolls one back on an idle timeout — and raising a
            # second error out of the cleanup would hide the first, which is
            # the one that says what went wrong.
            transaction._rollback_quietly()
            raise
        # Skipped when the body already ended the transaction itself. An
        # explicit `rollback()` inside the block is a legitimate way to say "not
        # this one", and committing over it would both undo the caller's
        # decision and fail with `no such transaction`.
        if not transaction._finished:
            transaction._commit()

    def transact(
        self,
        body: Callable[[Transaction], T],
        *,
        attempts: int = 5,
        base_delay: float = 0.005,
        max_delay: float = 0.5,
    ) -> T:
        """Run `body` in a transaction, retrying a conflict.

        Retries `Conflict` and nothing else, which is `RecordStore::transact`'s
        own rule: "a unique violation, an access denial or a fenced writer
        fails identically forever, and retrying them turns a clear error into a
        hang". In particular `Unavailable` is not retried here even though it
        is `Retryable` — a fenced writer is permanent for that node, and the
        retry that helps is against a different node.

        Backoff is jittered, because writers that collided once otherwise wake
        together and collide again.
        """
        if attempts < 1:
            raise ValueError("attempts must be at least 1")
        for attempt in range(attempts):
            try:
                with self.transaction() as transaction:
                    return body(transaction)
            except Conflict:
                # Re-raised from the last attempt rather than saved and thrown
                # after the loop: an `assert` that the saved one is not None
                # disappears under `python -O`, and `raise None` would replace
                # the conflict with a TypeError.
                if attempt == attempts - 1:
                    raise
                # Full jitter: uniform in [0, backoff] rather than
                # backoff ± something, which still leaves a mode for the
                # colliding writers to land on.
                backoff = min(max_delay, base_delay * (2**attempt))
                time.sleep(random.uniform(0, backoff))
        # Unreachable: the loop either returns or raises on its last attempt.
        raise AssertionError("transact fell out of its retry loop")

    # --- other -------------------------------------------------------------

    def leadership(self) -> LeadershipStatus:
        """What this node believes about who may write."""
        return LeadershipStatus(
            self._unary(self._conn.stub.Leadership, pb.LeadershipRequest())
        )


class Transaction(_Ops):
    """A transaction on the writer.

    Reads inside it are served by the transaction, on the writer, and they fail
    on a takeover — that is inherent, and a client that must keep reading
    through a handover reads outside a transaction.
    """

    def __init__(self, session: Session, id: str) -> None:
        self._session = session
        self._conn = session._conn
        # Inherited, not defaulted: a caller who set a deadline on the session
        # meant it for the work, and the work is mostly inside transactions.
        self._timeout = session._timeout
        self._id = id
        self._finished = False
        #: The sequence the commit landed at, once it has. `None` if the
        #: transaction wrote nothing.
        self.sequence: ReadToken | None = None

    @property
    def id(self) -> str:
        """The server's handle.

        A capability: anyone holding it can use the transaction, which is why
        it is a v4 UUID and is additionally pinned to the principal that opened
        it. Do not log it.
        """
        return self._id

    def _transaction_id(self) -> str:
        return self._id

    def _freshness(self, asked: Freshness | None) -> pb.Freshness | None:
        if asked is not None:
            raise ValueError(
                "a read inside a transaction is served by that transaction, on the "
                "writer, so freshness has no meaning for it. Read outside the "
                "transaction to choose a view."
            )
        return None

    def _observe(self, sequence: int | None) -> None:
        # A write inside a transaction has no sequence until the commit, and
        # the commit is what feeds the session's watermark.
        return

    def _observe_read(self, served_by: ServedBy | None) -> None:
        # A transactional read reports `writer (in transaction)` at sequence 0,
        # which is not a sequence any other view can be compared against.
        return

    def _commit(self) -> None:
        response = self._unary(self._conn.stub.Commit, pb.CommitRequest(transaction=self._id))
        self._finished = True
        if response.HasField("sequence"):
            self.sequence = ReadToken(response.sequence)
            # The whole point: the session that opened this transaction can now
            # read its own writes from a replica without anybody naming a token.
            self._session._observe(response.sequence)

    def rollback(self) -> None:
        """Abandon the transaction now, rather than at the end of the block."""
        if self._finished:
            return
        self._unary(self._conn.stub.Rollback, pb.RollbackRequest(transaction=self._id))
        self._finished = True

    def _rollback_quietly(self) -> None:
        if self._finished:
            return
        with contextlib.suppress(SlateError):
            self.rollback()
        self._finished = True


class Client(Session):
    """A connection to a head node, which is also a session.

    `Client` is a `Session` because the common case is one process acting as
    one caller. A process serving many end users should call `session()` per
    user, so that one user's write does not pin another's reads to the writer.
    """

    def __init__(
        self,
        target: str,
        identity: Identity | None = None,
        *,
        channel: grpc.Channel | None = None,
        monotonic_reads: bool = True,
        timeout: float | None = None,
        options: Sequence[tuple[str, object]] | None = None,
    ) -> None:
        owns = channel is None
        if channel is None:
            # Insecure by default and by design: the `.proto` says TLS belongs
            # to whatever fronts the server, and inventing a second place to
            # configure it makes the deployment worse. Pass `channel=` for a
            # secure one.
            channel = grpc.insecure_channel(target, options=list(options or ()))
        super().__init__(
            _Connection(channel, identity or Identity(), owns),
            monotonic_reads=monotonic_reads,
            timeout=timeout,
        )
        self._monotonic_default = monotonic_reads

    def session(
        self,
        *,
        monotonic_reads: bool | None = None,
        timeout: float | None = None,
    ) -> Session:
        """An independent freshness scope over the same connection.

        `timeout` defaults to this client's, rather than to `None`: a caller who
        set one meant it for the connection, and a `session()` that quietly
        dropped it would leave exactly the calls a busy process makes most
        without a deadline. Pass `timeout=None` explicitly to opt out.
        """
        return Session(
            self._conn,
            monotonic_reads=self._monotonic_default
            if monotonic_reads is None
            else monotonic_reads,
            timeout=self._timeout if timeout is None else timeout,
        )

    def close(self) -> None:
        if self._conn.owns_channel:
            self._conn.channel.close()

    def __enter__(self) -> Client:
        return self

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc: BaseException | None,
        tb: types.TracebackType | None,
    ) -> None:
        self.close()

    def wait_for_ready(self, timeout: float = 10.0) -> None:
        """Block until the channel is connected, or raise.

        `grpc.insecure_channel` is lazy, so the first RPC is what discovers
        that nothing is listening — and it discovers it as whatever error that
        RPC would have raised. This makes the connection failure its own event.
        """
        try:
            grpc.channel_ready_future(self._conn.channel).result(timeout=timeout)
        except grpc.FutureTimeoutError as error:
            raise TimeoutError(
                f"the head node did not accept a connection within {timeout}s"
            ) from error
