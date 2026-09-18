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
import contextlib
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from slate import (
    Agg,
    Atomicity,
    Batch,
    Client,
    DeleteWhere,
    GroupedJoinQuery,
    Identity,
    JoinQuery,
    JoinType,
    Metric,
    Query,
    SlateError,
    Step,
    TimeUnit,
    Units,
    UpdateWhere,
    Vector,
    as_scalar,
    asc,
    case,
    concat,
    desc,
    distance,
    extract,
    i64,
    in_zone,
    lit,
    lower,
    month_start,
    regexp_replace,
    u64,
    upper,
    year,
)

from .schema import AUTHORS, BOOKS, BY_NAME, EDITIONS, SALES
from .values import decode, encode, encode_row, format_float

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

    def related(self, session, body):
        """One relationship, loaded for many parents in one read.

        `sales.book_id -> books`, the demo's only foreign key: `books.author_id`
        cannot be one, because `Author Unknown` names author 99 on purpose so
        the outer joins have an unmatched side to show.

        Read the `parents` way it goes through `books`, which carries the row
        policy -- so a `reader` asking for the books behind a page of sales
        must see the same gap in all three SDKs, and a client that resolved the
        relationship itself rather than asking the server would not have one.
        """
        parents = body.get("way") == "parents"
        keys = [decode(raw) for raw in body.get("keys", [])]
        # `through` is overridable only so the conformance corpus can name a
        # key that does not exist and compare the three refusals, which is the
        # one thing about this call the three could spell differently.
        groups = session.related(
            BOOKS if parents else SALES,
            keys,
            through=body.get("through") or "sale_book",
            on=SALES,
            children=not parents,
        )
        # A group per key the caller sent, in the caller's order, including the
        # empty ones -- the shape all three clients promise.
        return {
            "groups": [[encode_row(list(row)) for row in group] for group in groups]
        }

    def path(self, session, body):
        """A relationship *path*, resolved level by level in one request.

        `sales -> books -> editions`: up to the book a sale sold, then down to
        that book's editions. Two steps in opposite directions, which is the
        case worth comparing across three SDKs — a path that only ever went one
        way would agree even with the two directions confused.

        The keys are `book_id` values read off sale rows, because that is where
        a path starts: at the column of the caller's own rows that relates them
        to the first step.

        Both shapes in one answer. `trees` keeps the middle level, which is
        `load_nested`; `through` drops it, which is `load_related_through`. They
        are one request and one regrouping, and the difference between them is
        exactly one line in each SDK — the line that reads "the rows at the
        bottom" rather than "the rows with nothing below them", and that is
        worth pinning in all three.
        """
        keys = [decode(raw) for raw in body.get("keys", [])]
        steps = [
            Step(on=SALES, through="sale_book", table=BOOKS, children=False),
            Step(on=EDITIONS, through="edition_book", table=EDITIONS),
        ]
        trees = session.related_path(keys, steps)
        through = session.related_through(keys, steps)
        return {
            "trees": [
                [
                    {
                        "row": encode_row(list(node.row)),
                        "related": [encode_row(list(leaf.row)) for leaf in node.related],
                    }
                    for node in tree
                ]
                for tree in trees
            ],
            "through": [[encode_row(list(row)) for row in rows] for rows in through],
        }

    #: The embedding every `/api/nearest` request measures against.
    #:
    #: Fixed rather than taken from the request body, because the point is that
    #: three SDKs build the same `Distance` scalar and agree on the order it
    #: produces. A vector from the body would let a caller ask a question the
    #: other two adapters were not asked.
    QUERY_VECTOR = (0.1, 0.2, 0.3, 0.4)

    def nearest(self, session, body):
        """Books ranked by cosine distance from `QUERY_VECTOR`.

        The last scalar family the three SDKs were never compared on. It is
        also the only one whose *result* cannot be compared: a distance is an
        f64 and the three clients format floats differently, which is why
        `/api/explain` excludes `estimatedCost` for the same reason. So this
        returns the titles in order and not the distances -- the order is the
        claim, and it is total because the sort breaks ties on the id.
        """
        query = Query(BOOKS)
        query.compute(
            distance(query.c.embedding, lit(Vector(self.QUERY_VECTOR)), Metric.COSINE)
        )
        query.select(query.c.id, query.c.title)
        query.sort(asc(query.computed(0)), asc(query.c.id))
        if body.get("limit") is not None:
            query.limit(int(body["limit"]))
        titles = [encode(row.get("title")) for row in session.query(query)]
        return {"titles": titles}

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

    def chain(self, session, body):
        """Three tables in one request: authors, their books, those books' sales.

        Separate from `/api/join` because it is the thing worth comparing and
        not a variation on a two-table join. A chain is not a different RPC --
        a `JoinQuery` carries as many inputs as it is given and the kernel
        picks its chain path past two -- so what could differ between the three
        SDKs is how each spells the *third* input's attachment: it joins back
        to the second, and a client that attached it to the first would produce
        a cross join with the right number of columns.
        """
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
        books = join.add(BOOKS, on=[(authors.c.id, "author_id")], join_type=kind)
        # books.id to sales.book_id -- named against the *second* input, which
        # is what makes this a chain rather than two joins onto the first.
        join.add(SALES, on=[(books.c.id, "book_id")], join_type=kind)
        if body.get("limit") is not None:
            join.limit(int(body["limit"]))

        def side(joined, at):
            # `None` where an outer join found no match, kept distinct from a
            # row of nulls.
            return encode_row(list(joined[at])) if joined[at] is not None else None

        rows = [
            {
                "authors": side(joined, 0),
                "books": side(joined, 1),
                "sales": side(joined, 2),
            }
            for joined in session.join(join)
        ]
        rows.sort(key=lambda row: json.dumps(row, sort_keys=True))
        return {"rows": rows}

    def page(self, session, body):
        """One page of `books` by keyset, and where to resume.

        The cursor comes back as a row so the three adapters encode it the way
        they encode everything else, and so the corpus compares its *type* as
        well as its value -- a cursor arriving as a bare number would agree
        across three clients that had all lost the same distinction.
        """
        query = Query(BOOKS)
        if body.get("limit"):
            query.limit(int(body["limit"]))
        if body.get("after"):
            query.after([decode(v) for v in body["after"]])
        if body.get("columns"):
            query.select(*(query.c[BOOKS.column_names[i]] for i in body["columns"]))
        keys = []
        for key in body.get("sort") or []:
            column = query.c[BOOKS.column_names[key["column"]]]
            keys.append(desc(column) if key.get("direction") == "desc" else asc(column))
        if keys:
            query.sort(*keys)

        page = session.page(query)
        # `None` rather than an empty list for the last page, so "there is
        # nothing after this" is one value in all three adapters, not two.
        cursor = None if page.is_last else [encode(v) for v in page.cursor]
        return {
            "rows": [encode_row(list(row)) for row in page.rows],
            "cursor": cursor,
            "isLast": page.is_last,
        }

    def _build_aggregate(self, body):
        """The join and grouping the contract's aggregate body names.

        Shared by `aggregate` and `explain_aggregate` for the same reason the
        kernel shares its narrowing between running a grouped read and
        explaining one: an explanation of a *different* request is worse than
        none.
        """
        join = JoinQuery()
        authors = join.add(AUTHORS)
        books = join.add(BOOKS, on=[(authors.c.id, "author_id")])

        by = body.get("groupBy")
        if by == "author":
            key = authors.c.name
        elif by == "country":
            key = authors.c.country
        elif by == "decade":
            # Not a column at all. This used to be refused here, in all three
            # adapters, with "needs a computed column, which this demo does not
            # declare" -- true of the clients rather than of the database, since
            # the kernel has had scalar expressions throughout. Hand-bucketing
            # it here would have been the adapter doing the database's job, and
            # would have made three adapters bucket identically for no reason.
            #
            # `books.year / 10 * 10`, computed over the *joined* row. Integer
            # division truncates toward zero, which is what a decade means for
            # these years.
            join.compute((as_scalar(books.c.year) / i64(10)) * i64(10))
            key = join.computed(0)
        # Everything below is a different *kind* of scalar rather than a
        # different column. Arithmetic was the only expression the three SDKs
        # were ever compared on, and division is the one operation every
        # language spells identically -- so agreement on it proved much less
        # than it looked.
        elif by == "shout":
            # A string function, on the *left* input. The author's *name*
            # rather than the country, because every country here is already
            # upper case -- so an adapter that dropped the `upper` would have
            # passed.
            join.compute(upper(authors.c.name))
            key = join.computed(0)
        elif by == "era":
            # A conditional whose branches are strings and whose test is on an
            # integer column, so the types differ across the expression.
            join.compute(
                # 1970 rather than 2000, because every book here predates
                # 2000 -- so the `otherwise` branch was never taken and the
                # conditional was a constant. Five fall either side of 1970.
                case(
                    [(books.c.year.lt(i64(1970)), lit("before 1970"))],
                    otherwise=lit("from 1970"),
                )
            )
            key = join.computed(0)
        elif by == "tidy":
            # A regular expression over a lower-cased title: the pattern
            # dialect and the case folding both have to agree.
            join.compute(regexp_replace(lower(books.c.title), "[^a-z]+", "-"))
            key = join.computed(0)
        elif by == "releasedYear":
            # A calendar field over seconds since the epoch. Seven of the
            # eleven books are before 1970, so this runs on negative instants.
            join.compute(year(books.c.released))
            key = join.computed(0)
        elif by == "releasedMonth":
            # A calendar *truncation*, which is not a division: a month has no
            # fixed number of seconds.
            join.compute(month_start(books.c.released))
            key = join.computed(0)
        elif by == "releasedHourNY":
            # A named timezone, resolved through the server's transition
            # table. Some of these dates are in daylight saving and some are
            # not, so this is not a constant shift.
            join.compute(
                extract(TimeUnit.HOUR, in_zone(books.c.released, "America/New_York"))
            )
            key = join.computed(0)
        elif by == "label":
            # Concatenation across *both* inputs, which no input's own compute
            # could express.
            # The year at the end is an i64 spliced into a string, which is
            # where `Concat` was rendering Rust's `Debug` form: `1968` came
            # out as `I64(1968)`.
            join.compute(
                concat(
                    authors.c.country, lit("/"), books.c.title, lit("/"),
                    books.c.year,
                )
            )
            key = join.computed(0)
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
        return grouped

    def aggregate(self, session, body):
        grouped = self._build_aggregate(body)
        groups = []
        for group in session.aggregate(grouped):
            entry: dict[str, Any] = {"key": encode_row(list(group.key))}
            values = list(group.aggregates)
            if values:
                entry["count"] = encode(values[0])
            groups.append(entry)
        return {"groups": groups}

    def explain_aggregate(self, session, body):
        """The plan of the *grouped* read, which is not the plan of the join
        underneath: grouping narrows each input's projection to the group keys
        and the aggregates' columns. `decodes` is where that shows."""
        plan = session.explain_aggregate(self._build_aggregate(body))
        if plan.join is None:
            raise ValueError("a grouped join explained as something other than a join")
        return {
            "inputs": [
                {
                    "table": input.plan.table,
                    "access": input.plan.access,
                    "indexOnly": input.plan.index_only,
                    "decodes": list(input.plan.decodes),
                    "algorithm": input.algorithm or "",
                }
                for input in plan.join.inputs
            ],
            "display": plan.display,
        }

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

    #: The id range the predicate-write handler owns, clear of the fixture and
    #: of the transaction probe at 9001.
    #:
    #: Predicate writes mutate, and the runner drives all three adapters
    #: against one database, so each run seeds its own rows first and the three
    #: see the same four. The fixture is never touched: a case that deleted
    #: from it would make every later case depend on which SDK ran first.
    PREDICATE_FIRST = 9100

    def predicate_write(self, session, body):
        """Seed four rows, write over them by predicate, report what came back.

        Self-contained and idempotent, like the transaction probe below and for
        the same reason: the demo, and the corpus, must give the same answer
        run twice.
        """
        first = self.PREDICATE_FIRST
        mine = DeleteWhere(BOOKS)
        # Clean slate. A predicate delete is the tidiest way to say "whatever
        # is left from last time", and it exercises the feature on the way in.
        session.delete_where(mine.where(mine.c.id.ge(u64(first))))
        session.insert(
            BOOKS,
            [
                [
                    u64(first + n), u64(1), f"Predicate {n}", i64(2000 + n),
                    3.0, i64(1767225600), Vector((0.1, 0.2, 0.3, 0.4)),
                    Units(1000),
                ]
                for n in range(4)
            ],
        )

        returning = bool(body.get("returning"))
        kind = body.get("kind")
        if kind == "delete":
            w = DeleteWhere(BOOKS)
            # Rows 9102 and 9103: year >= 2002.
            result = session.delete_where(
                w.where(w.c.id.ge(u64(first)) & w.c.year.ge(i64(2002)))
                .returning(returning)
            )
        elif kind == "update":
            w = UpdateWhere(BOOKS)
            w.where(w.c.id.ge(u64(first)) & w.c.year.ge(i64(2002)))
            if not body.get("noSet"):
                # rating = rating + 1, read off the row as it was.
                w.set(w.c.rating, w.c.rating + 1.0)
            result = session.update_where(w.returning(returning))
        else:
            raise ValueError(f"unknown predicate write {kind!r}")

        # How many of the four are left, which is what makes a delete's effect
        # visible rather than only its report.
        q = Query(BOOKS)
        left = len(list(session.query(q.where(q.c.id.ge(u64(first))))))
        return {
            "affected": result.affected,
            "rows": [encode_row(row.values) for row in result.rows],
            "left": left,
        }

    #: The id range the batch handler owns, clear of the fixture, of the
    #: transaction probe at 9001 and of the predicate-write range at 9100.
    BATCH_FIRST = 9200

    def batch(self, session, body):
        """Three writes, one of which collides, under the asked-for atomicity.

        The duplicate is the point: it is the operation that makes the two
        guarantees visibly different, and `left` afterwards is how the corpus
        sees which one happened.
        """
        first = self.BATCH_FIRST
        mine = DeleteWhere(BOOKS)
        session.delete_where(mine.where(mine.c.id.ge(u64(first))))
        session.insert(BOOKS, [self._book(first + 1, "Already There")])

        atomicity = (
            Atomicity.ALL_OR_NOTHING
            if body.get("atomicity") == "all-or-nothing"
            else Atomicity.INDEPENDENT
        )
        b = Batch(atomicity)
        b.insert(BOOKS, [self._book(first, "First")])
        b.insert(BOOKS, [self._book(first + 1, "Collides")])
        b.insert(BOOKS, [self._book(first + 2, "Third")])

        failed = ""
        outcomes = []
        try:
            # `session.batch(b)`, not `b and session.batch(b)`. The `and` was
            # dead weight that a type checker found: `Batch` has no `__bool__`
            # a reader can predict, and on the branch where it is falsy
            # `result` would be the *batch* and `result.outcomes` an
            # `AttributeError` from inside a request handler. Three operations
            # are always queued above, so the branch has never been taken.
            result = session.batch(b)
            for one in result.outcomes:
                if one.ok:
                    outcomes.append({"ok": one.written.affected})
                else:
                    outcomes.append(
                        {"kind": kind_name(one.error), "reason": one.error.reason}
                    )
        except SlateError as error:
            # An atomic batch fails the call. Reported as a field rather than
            # raised, so the corpus compares the *outcome* of the two
            # atomicities rather than one being a refusal case and one not.
            failed = kind_name(error)

        q = Query(BOOKS)
        left = len(list(session.query(q.where(q.c.id.ge(u64(first))))))
        return {"failed": failed, "outcomes": outcomes, "left": left}

    #: The id the conditional-update handler owns, clear of every other range.
    CONDITIONAL_ID = 9300

    def conditional_update(self, session, body):
        """Optimistic concurrency, and the decimal column it exists to protect.

        Seeds one book at 9300 priced 10.00, reads it back, optionally lets
        somebody else move the price, then tries a conditional update to 12.50
        guarded by the row as it was read. With `stale: false` it lands; with
        `stale: true` the server refuses it and the price is whatever the other
        writer left.

        One endpoint rather than two because the *pair* is the point: an
        unconditional update and a conditional one over an unchanged row do
        exactly the same thing, so only the stale case tells them apart.
        """
        id_ = self.CONDITIONAL_ID
        session.insert(BOOKS, [self._book(id_, "Priced")], upsert=True)

        # Read the row back rather than reusing what was written: a conditional
        # update guards against what is *stored*, and a caller that guards with
        # its own draft is testing its memory rather than the database.
        row = session.get(BOOKS, (u64(id_),))
        if row is None:
            raise RuntimeError("the seeded row is not there")
        was = list(row)

        if bool(body.get("stale")):
            moved = list(was)
            moved[7] = Units(1100)
            session.update(BOOKS, [moved])

        nxt = list(was)
        nxt[7] = Units(1250)
        refused = ""
        try:
            session.update(BOOKS, [nxt], expected=[was])
        except SlateError as error:
            # A field rather than an adapter error, so the corpus compares the
            # two cases as ordinary answers instead of one being a refusal.
            refused = kind_name(error)

        after = session.get(BOOKS, (u64(id_),))
        if after is None:
            raise RuntimeError("the row vanished")
        price = after.get("price")
        if not isinstance(price, Units):
            raise RuntimeError(f"a decimal came back as {type(price).__name__}")
        return {
            "refused": refused,
            "price": encode(price),
            # The rendering, against the scale this adapter declares. The
            # tagged value above is the units and says nothing about a scale,
            # so this is the only place the three clients' renderers are
            # compared.
            "rendered": price.to_string_with_scale(2),
        }

    #: The id the conditional-delete handler owns.
    CONDITIONAL_DELETE_ID = 9301

    def conditional_delete(self, session, body):
        """The delete half of optimistic concurrency.

        `stale` lets somebody else edit the row first; `gone` removes it first.
        Applied, refused-because-moved and refused-because-absent are three
        different answers, and the third is the interesting one: a *plain*
        delete reports an absent key as `affected: 0`, and a conditional one
        refuses it.
        """
        id_ = self.CONDITIONAL_DELETE_ID
        session.insert(BOOKS, [self._book(id_, "Doomed")], upsert=True)
        row = session.get(BOOKS, (u64(id_),))
        if row is None:
            raise RuntimeError("the seeded row is not there")
        was = list(row)

        if bool(body.get("stale")):
            moved = list(was)
            moved[7] = Units(1100)
            session.update(BOOKS, [moved])
        if bool(body.get("gone")):
            session.delete(BOOKS, [(u64(id_),)])

        refused = ""
        affected = 0
        try:
            result = session.delete(BOOKS, [(u64(id_),)], expected=[was])
            affected = result.affected
        except SlateError as error:
            refused = kind_name(error)

        # `left` is what the table says, beside `affected` and `refused`, which
        # are what the server said it did.
        left = session.get(BOOKS, (u64(id_),)) is not None
        return {"refused": refused, "affected": affected, "left": left}

    @staticmethod
    def _book(id_: int, title: str) -> list:
        """A whole `books` row, every column in ordinal order."""
        return [
            u64(id_), u64(1), title, i64(2020), 4.0,
            i64(1767225600), Vector((0.5, 0.5, 0.5, 0.5)),
            # 10.00, at the column's declared scale of 2.
            Units(1000),
        ]

    def transaction(self, session, body):
        """The one thing a single request cannot show: a write visible only to
        its own transaction until it commits."""
        from slate import NotFound

        probe = 9001
        # `suppress` rather than a bare `pass`: the probe row is absent on the
        # first run and present on a re-run, and both are fine.
        with contextlib.suppress(NotFound):
            session.delete(BOOKS, [[u64(probe)]])

        with session.transaction() as tx:
            # Every column, in ordinal order, including the two `books` grew
            # for the conformance corpus. A row on the wire is one value per
            # column, so a five-value row here is a refusal -- which is how
            # adding them was caught.
            tx.insert(
                BOOKS,
                [[
                    u64(probe), u64(1), "A Book In Flight", i64(2026), 5.0,
                    i64(1767225600), Vector((0.4, 0.3, 0.2, 0.1)),
                    Units(1000),
                ]],
            )
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
    "/api/explain-aggregate": "explain_aggregate",
    "/api/nearest": "nearest",
    "/api/chain": "chain",
    "/api/page": "page",
    "/api/related": "related",
    "/api/batch": "batch",
    "/api/path": "path",
    "/api/predicate-write": "predicate_write",
    "/api/conditional-update": "conditional_update",
    "/api/conditional-delete": "conditional_delete",
    "/api/transaction": "transaction",
}


def handler_for(adapter: Adapter):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, format: str, *args: Any) -> None:
            # Silence, on purpose: `run.sh` captures each adapter's output and
            # a line per request would bury the ones that matter. The signature
            # matches `BaseHTTPRequestHandler`'s exactly — `*_args` alone
            # dropped the positional `format` and made this an override the
            # base class could not call.
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

        def do_OPTIONS(self) -> None:
            self._send(204, {})

        def do_GET(self) -> None:
            self._dispatch()

        def do_POST(self) -> None:
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
                #
                # `reason` rides along for the same purpose and is the stronger
                # of the two: a kind is this adapter's word for a status code,
                # while the token is the server's own and is finer than the
                # code. A client that decodes the details blob differently from
                # the other two disagrees here rather than in production.
                self._send(200, {"error": {
                    "kind": kind_name(error),
                    "message": str(error.message),
                    "reason": error.reason,
                }})
                return
            except Exception as error:  # noqa: BLE001
                # Blind on purpose, and the directive is live: this is the
                # outermost handler of an HTTP request, so anything it does not
                # catch takes the adapter's connection down and the conformance
                # runner sees a transport error rather than an answer it can
                # compare.
                #
                # No `reason`: this one never reached the server, so there is no
                # token to report and an empty one would be a claim about a
                # failure the server never saw.
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
