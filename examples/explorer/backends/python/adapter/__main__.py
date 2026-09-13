"""The Python adapter of the explorer demo.

Speaks the HTTP contract in ../../CONTRACT.md against a slate head node, using
the Python client. Two more adapters do the same through the Go and TypeScript
clients, and `conformance/` requires all three to answer identically.

`http.server` rather than a framework: the adapter is six endpoints and no
middleware, and a dependency here would be a dependency the demo asks a reader
to install before they can see anything work.
"""

from __future__ import annotations

import argparse
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from slate import (
    Agg,
    AggregateQuery,
    Client,
    GroupedJoinQuery,
    Identity,
    JoinQuery,
    JoinType,
    Query,
    SlateError,
    asc,
    desc,
    u64,
)

from .schema import AUTHORS, BOOKS, BY_NAME
from .values import encode, encode_row, format_float

# The demo's three personas, mapped onto head-node identities.
#
# `reader` differs from `app` only in what the *database* grants it, and
# `stranger` holds a role with no grant on the tables the UI shows. No adapter
# enforces any of it — that is the point of the switcher.
IDENTITIES = {
    "app": Identity("u64:1", tenant="u64:1", roles=["app"]),
    "reader": Identity("u64:2", tenant="u64:1", roles=["reader"]),
    "stranger": Identity("u64:3", tenant="u64:1", roles=["stranger"]),
}


def build_filter(query, table, spec: dict[str, Any] | None):
    """The contract's filter tree, as this client's expressions.

    A closed grammar rather than a string each adapter parses: three parsers
    for one little language is three things to keep in agreement, and the first
    divergence would look like a database bug.
    """
    from .values import decode

    if spec is None:
        return None
    op = spec.get("op")

    def column_at(index: int):
        return query.c[table.column_names[index]]

    if op in {"eq", "ne", "lt", "le", "gt", "ge"}:
        return getattr(column_at(spec["column"]), op)(decode(spec["value"]))
    if op in {"like", "ilike"}:
        return column_at(spec["column"]).like(spec["pattern"], insensitive=(op == "ilike"))
    if op == "isNull":
        return column_at(spec["column"]).is_null()
    if op == "isNotNull":
        return column_at(spec["column"]).is_not_null()
    if op == "in":
        return column_at(spec["column"]).in_([decode(v) for v in spec["values"]])
    if op in {"and", "or"}:
        from slate import all_of, any_of

        parts = [build_filter(query, table, part) for part in spec["parts"]]
        return (all_of if op == "and" else any_of)(parts)
    if op == "not":
        from slate import not_

        return not_(build_filter(query, table, spec["part"]))
    raise ValueError(f"no such filter operator: {op}")


def build_query(spec: dict[str, Any]) -> Query:
    name = spec.get("table", "")
    table = BY_NAME.get(name)
    if table is None:
        # Caught here rather than by the server, because two of the three
        # clients hold a schema and cannot build a request without one. The
        # contract says so; all three word it identically.
        raise ValueError(f"no such table: {name}")
    query = Query(table)
    where = build_filter(query, table, spec.get("filter"))
    if where is not None:
        query.where(where)
    keys = []
    for key in spec.get("sort") or []:
        column = query.c[table.column_names[key["column"]]]
        keys.append(desc(column) if key.get("direction") == "desc" else asc(column))
    if keys:
        # One call: `sort` replaces the ordering rather than appending to it,
        # so a loop of calls keeps only the last key.
        query.sort(*keys)
    if spec.get("limit") is not None:
        query.limit(int(spec["limit"]))
    if spec.get("offset"):
        query.offset(int(spec["offset"]))
    if spec.get("columns"):
        query.select(*(query.c[table.column_names[i]] for i in spec["columns"]))
    return query


class Adapter:
    def __init__(self, head: str) -> None:
        self.head = head
        self.clients = {
            name: Client(head, identity=identity) for name, identity in IDENTITIES.items()
        }

    def meta(self, session, _body):
        return {
            "sdk": "python",
            "leader": self.clients["app"].session().leadership().is_leader,
            "tables": ["authors", "books", "sales"],
        }

    def query(self, session, body):
        rows = [encode_row(list(row)) for row in session.query(build_query(body))]
        return {"rows": rows}

    def join(self, session, body):
        kinds = {
            "inner": JoinType.INNER,
            "left": JoinType.LEFT,
            "right": JoinType.RIGHT,
            "full": JoinType.FULL,
        }
        kind = kinds.get(body.get("type", ""))
        if kind is None:
            raise ValueError(f"no such join type: {body.get('type')}")

        join = JoinQuery()
        authors = join.add(AUTHORS)
        join.add(BOOKS, on=[(authors.c.id, "author_id")], join_type=kind)
        if body.get("limit") is not None:
            join.limit(int(body["limit"]))

        rows = []
        for joined in session.join(join):
            # A JoinedRow is a sequence with one entry per input, `None` where
            # an outer join found no match — kept distinct from a row of nulls.
            left, right = joined[0], joined[1]
            rows.append(
                {
                    "authors": encode_row(list(left)) if left is not None else None,
                    "books": encode_row(list(right)) if right is not None else None,
                }
            )
        # A join's row order is the plan's business. Sorted so the three
        # adapters are comparable and the table does not reshuffle when a hint
        # changes the algorithm.
        rows.sort(key=lambda row: json.dumps(row, sort_keys=True))
        return {"rows": rows}

    def aggregate(self, session, body):
        join = JoinQuery()
        authors = join.add(AUTHORS)
        join.add(BOOKS, on=[(authors.c.id, "author_id")])

        by = body.get("groupBy")
        if by == "author":
            key = authors.c.name
        elif by == "country":
            key = authors.c.country
        elif by == "decade":
            # Not a column. The kernel can compute one with a scalar
            # expression; hand-bucketing it here would be the adapter doing the
            # database's job, and the three adapters would then have to bucket
            # identically for no reason.
            raise ValueError(
                "grouping by decade needs a computed column, which this demo does not declare"
            )
        else:
            raise ValueError(f"no such grouping: {by}")

        grouped = GroupedJoinQuery(join)
        grouped.group_by(key)
        grouped.aggregate(Agg.count())
        if body.get("having"):
            grouped.having(grouped.agg(0).ge(u64(int(body["having"]["minCount"]))))

        column = grouped.key(0) if body.get("sort") == "key" else grouped.agg(0)
        direction = desc if body.get("direction") == "desc" else asc
        # A tie-break on the key, so equal counts do not come back in whatever
        # order the hash produced — which would differ between adapters.
        grouped.sort(direction(column), asc(grouped.key(0)))
        if body.get("limit") is not None:
            grouped.limit(int(body["limit"]))

        groups = []
        for group in session.aggregate(grouped):
            entry: dict[str, Any] = {"key": encode_row(list(group.key))}
            values = list(group.aggregates)
            if values:
                entry["count"] = encode(values[0])
            groups.append(entry)
        return {"groups": groups}

    def explain(self, session, body):
        plan = session.explain(build_query(body))
        # `estimatedCost` is deliberately absent: a float the three clients may
        # render differently, and the contract compares text.
        return {
            "table": plan.table,
            "access": plan.access,
            "residual": plan.residual,
            "indexOnly": plan.index_only,
            "sorts": plan.sorts,
            "descending": plan.descending,
            "estimatedRows": format_float(plan.estimated_rows),
            "display": plan.display,
        }

    def transaction(self, session, body):
        """The one thing a single request cannot show: a write visible only to
        its own transaction until it commits."""
        from slate import NotFound

        probe = 9001
        try:
            session.delete(BOOKS, [[u64(probe)]])
        except NotFound:
            pass

        with session.transaction() as tx:
            tx.insert(BOOKS, [[u64(probe), u64(1), "A Book In Flight", 2026, 5.0]])
            inside = tx.get(BOOKS, [u64(probe)]) is not None
            if not body.get("commit"):
                tx.rollback()

        after = session.get(BOOKS, [u64(probe)]) is not None
        # Leave nothing behind, so a committed run and a rolled-back run start
        # the same way and the demo is idempotent.
        if after:
            session.delete(BOOKS, [[u64(probe)]])
        return {"visibleInside": inside, "visibleAfter": after}


ROUTES = {
    "/api/meta": "meta",
    "/api/query": "query",
    "/api/join": "join",
    "/api/aggregate": "aggregate",
    "/api/explain": "explain",
    "/api/transaction": "transaction",
}


def handler_for(adapter: Adapter):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args):  # noqa: D102 - quiet by default
            pass

        def _send(self, status: int, body: dict[str, Any]) -> None:
            payload = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.send_header("Access-Control-Allow-Origin", "*")
            self.send_header("Access-Control-Allow-Headers", "content-type, x-demo-identity")
            self.end_headers()
            self.wfile.write(payload)

        def do_OPTIONS(self) -> None:  # noqa: N802 - the http.server spelling
            self._send(204, {})

        def do_GET(self) -> None:  # noqa: N802
            self._dispatch()

        def do_POST(self) -> None:  # noqa: N802
            self._dispatch()

        def _dispatch(self) -> None:
            name = ROUTES.get(self.path.split("?")[0])
            if name is None:
                self._send(404, {"error": {"kind": "not-found", "message": "no such endpoint"}})
                return

            persona = self.headers.get("X-Demo-Identity") or "app"
            client = adapter.clients.get(persona)
            if client is None:
                self._send(
                    400,
                    {"error": {"kind": "invalid-request", "message": f"no such identity: {persona!r}"}},
                )
                return

            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            try:
                body = json.loads(raw) if raw else {}
            except json.JSONDecodeError as error:
                self._send(400, {"error": {"kind": "adapter", "message": str(error)}})
                return

            try:
                result = getattr(adapter, name)(client.session(), body)
            except SlateError as error:
                # A slate error keeps its kind; the kinds are spelled the same
                # in all three adapters so the conformance runner compares them.
                self._send(200, {"error": {"kind": kind_name(error), "message": str(error.message)}})
                return
            except Exception as error:  # noqa: BLE001 - the adapter's own fault
                self._send(400, {"error": {"kind": "adapter", "message": str(error)}})
                return
            self._send(200, result)

    return Handler


def kind_name(error: SlateError) -> str:
    """The contract's spelling of an error kind."""
    from slate import (
        AlreadyExists,
        Cancelled,
        Conflict,
        DataLoss,
        DeadlineExceeded,
        InvalidRequest,
        NotFound,
        NotLeader,
        PermissionDenied,
        ResourceLimit,
        Unauthenticated,
        Unavailable,
        UnknownOutcome,
    )

    # Most specific first: `NotLeader` is an `Unavailable`, and `Conflict` and
    # `ResourceLimit` are both `Retryable`.
    for kind, name in (
        (NotLeader, "not-leader"),
        (Conflict, "conflict"),
        (ResourceLimit, "resource-limit"),
        (Unavailable, "unavailable"),
        (InvalidRequest, "invalid-request"),
        (NotFound, "not-found"),
        (AlreadyExists, "already-exists"),
        (PermissionDenied, "permission-denied"),
        (Unauthenticated, "unauthenticated"),
        (UnknownOutcome, "unknown-outcome"),
        (DataLoss, "data-loss"),
        (DeadlineExceeded, "deadline-exceeded"),
        (Cancelled, "cancelled"),
    ):
        if isinstance(error, kind):
            return name
    return "internal"


def main() -> None:
    parser = argparse.ArgumentParser(description="the explorer's Python adapter")
    parser.add_argument("--head", default="127.0.0.1:7421")
    parser.add_argument("--listen", default="127.0.0.1:7433")
    args = parser.parse_args()

    adapter = Adapter(args.head)
    host, _, port = args.listen.rpartition(":")
    server = ThreadingHTTPServer((host, int(port)), handler_for(adapter))

    # The same handshake the head node uses, so `run.sh` can wait for a line
    # rather than poll a port.
    print(f"LISTENING {args.listen}", flush=True)
    threading.current_thread().name = "adapter"
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    sys.exit(main())
