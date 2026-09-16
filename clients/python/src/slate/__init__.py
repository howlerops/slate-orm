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
    Client,
    AggregateExplanation,
    Explanation,
    GroupStream,
    Identity,
    JoinExplanation,
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
    GroupedJoinQuery,
    JoinAlgorithm,
    JoinInput,
    JoinQuery,
    JoinType,
    NullsOrder,
    DeleteWhere,
    Query,
    UpdateWhere,
    ScanOrder,
    SortKey,
    asc,
    desc,
)
from .rows import Group, JoinedRow, Row
from .scalar import (
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
    distance,
    extract,
    in_zone,
    day_of_month,
    day_of_week,
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
from .values import NULL, Null, PyValue, Vector, i64, u64

__version__ = "0.0.1"

__all__ = [
    "NULL",
    "Agg",
    "AggregateQuery",
    "AlreadyExists",
    "Cancelled",
    "Client",
    "CmpOp",
    "Column",
    "ColumnRef",
    "Columns",
    "Conflict",
    "DataLoss",
    "DeadlineExceeded",
    "AggregateExplanation",
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
    "PermissionDenied",
    "PyValue",
    "Query",
    "DeleteWhere",
    "UpdateWhere",
    "ReadToken",
    "ResourceLimit",
    "Retryable",
    "Row",
    "Page",
    "RowStream",
    "Scalar",
    "ScanOrder",
    "ServedBy",
    "Session",
    "SlateError",
    "SortKey",
    "Table",
    "TimeUnit",
    "Transaction",
    "Unauthenticated",
    "Unavailable",
    "UnknownOutcome",
    "ValueType",
    "Vector",
    "WriteResult",
    "all_of",
    "any_of",
    "asc",
    "case",
    "coalesce",
    "concat",
    "date_trunc",
    "desc",
    "as_scalar",
    "calendar_part",
    "calendar_trunc",
    "day_of_month",
    "day_of_week",
    "distance",
    "extract",
    "in_zone",
    "i64",
    "length",
    "lit",
    "lower",
    "month",
    "month_start",
    "not_",
    "regexp_replace",
    "round_",
    "u64",
    "year",
    "year_start",
    "upper",
]
