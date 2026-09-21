"""A typed Python client for the slate-orm head node.

    from slate import Client, Identity, Query, Table, Column, ValueType, u64

    docs = Table(
        "docs",
        [("id", ValueType.U64), ("kind", ValueType.STR), ("size", ValueType.I64)],
        primary_key=["id"],
    )

    with Client("127.0.0.1:50051", Identity("u64:1", roles=["app"])) as client:
        client.insert(docs, [(u64(1), "kind-a", 5)])

        # Reads its own write back, from a replica, without naming a token:
        # the session threads the sequence the write returned.
        q = Query(docs)
        for row in client.query(q.where(q.c.size >= 5)):
            print(row.get("kind"))

Three things worth reading before using it:

- `slate.expr` — why a column reference always comes from a builder and never
  from a `Table`, and why `==` is `.eq()`.
- `slate.freshness` — where the line between "threads a token for you" and
  "lets you reason about staleness" is drawn, and why.
- `slate.errors` — what the gRPC status codes mean and which of them may be
  retried.

And `PROTOCOL-FINDINGS.md`, which is what this client was written to produce.
"""

from __future__ import annotations

from .client import (
    AggregateExplanation,
    BatchOutcome,
    BatchResult,
    Client,
    Explanation,
    GroupStream,
    Identity,
    JoinExplanation,
    JoinPage,
    JoinStream,
    LeadershipStatus,
    Page,
    RowStream,
    Session,
    Transaction,
    WriteResult,
)
from .errors import (
    AlreadyExists,
    Cancelled,
    CheckFailure,
    Conflict,
    DataLoss,
    DeadlineExceeded,
    InternalError,
    InvalidRequest,
    NotFound,
    NotLeader,
    PermissionDenied,
    ResourceLimit,
    Retryable,
    SlateError,
    Unauthenticated,
    Unavailable,
    UnknownOutcome,
)
from .expr import CmpOp, ColumnRef, Columns, Expr, all_of, any_of, not_
from .freshness import Freshness, ReadToken, ServedBy
from .query import (
    Agg,
    AggregateQuery,
    Atomicity,
    Batch,
    DeleteWhere,
    GroupedJoinQuery,
    JoinAlgorithm,
    JoinInput,
    JoinQuery,
    JoinType,
    NullsOrder,
    Query,
    ScanOrder,
    SortKey,
    UpdateWhere,
    Window,
    asc,
    desc,
)
from .rows import Group, JoinedRow, RelatedNode, Row, Step
from .scalar import (
    CalendarPart,
    CalendarUnit,
    Metric,
    Scalar,
    TimeUnit,
    as_scalar,
    calendar_part,
    calendar_trunc,
    case,
    coalesce,
    concat,
    date_trunc,
    day_of_month,
    day_of_week,
    distance,
    extract,
    in_zone,
    length,
    lit,
    lower,
    month,
    month_start,
    regexp_replace,
    round_,
    upper,
    year,
    year_start,
)
from .schema import Column, Table
from .types import ValueType
from .values import NULL, Array, Null, PyValue, Units, Vector, i64, u64

__version__ = "0.0.1"

__all__ = [
    "NULL",
    "Agg",
    "AggregateExplanation",
    "AggregateQuery",
    "AlreadyExists",
    "Array",
    "Atomicity",
    "Batch",
    "BatchOutcome",
    "BatchResult",
    "CalendarPart",
    "CalendarUnit",
    "Cancelled",
    "CheckFailure",
    "Client",
    "CmpOp",
    "Column",
    "ColumnRef",
    "Columns",
    "Conflict",
    "DataLoss",
    "DeadlineExceeded",
    "DeleteWhere",
    "Explanation",
    "Expr",
    "Freshness",
    "Group",
    "GroupStream",
    "GroupedJoinQuery",
    "Identity",
    "InternalError",
    "InvalidRequest",
    "JoinAlgorithm",
    "JoinExplanation",
    "JoinInput",
    "JoinPage",
    "JoinQuery",
    "JoinStream",
    "JoinType",
    "JoinedRow",
    "LeadershipStatus",
    "Metric",
    "NotFound",
    "NotLeader",
    "Null",
    "NullsOrder",
    "Page",
    "PermissionDenied",
    "PyValue",
    "Query",
    "ReadToken",
    "RelatedNode",
    "ResourceLimit",
    "Retryable",
    "Row",
    "RowStream",
    "Scalar",
    "ScanOrder",
    "ServedBy",
    "Session",
    "SlateError",
    "SortKey",
    "Step",
    "Table",
    "TimeUnit",
    "Transaction",
    "Unauthenticated",
    "Unavailable",
    "Units",
    "UnknownOutcome",
    "UpdateWhere",
    "ValueType",
    "Vector",
    "Window",
    "WriteResult",
    "all_of",
    "any_of",
    "as_scalar",
    "asc",
    "calendar_part",
    "calendar_trunc",
    "case",
    "coalesce",
    "concat",
    "date_trunc",
    "day_of_month",
    "day_of_week",
    "desc",
    "distance",
    "extract",
    "i64",
    "in_zone",
    "length",
    "lit",
    "lower",
    "month",
    "month_start",
    "not_",
    "regexp_replace",
    "round_",
    "u64",
    "upper",
    "year",
    "year_start",
]
