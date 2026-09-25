"""A client sends `OR`, in a `WHERE` and in a `HAVING`.

# Why this file exists

`ledger/2026-09-25-the-disjunction-the-kernel-always-had.md` closed a
four-entry gap by teaching the *SQL front end* to write `OR`, and its caveats
said no client could send one and that the gRPC `Query` had no equivalent.

**Both were wrong.** `Expr.disjunction` has been on the wire in both
directions since the protocol carried expressions, `convert.rs` maps it to
`Expr::Or` and back, and all three clients have had a builder for it —
Python's `any_of` and `|`, Go's `Disjunction`, TypeScript's `or`. What was
missing was only a way to *write* one as SQL text.

Nothing executed that, which is how a wrong caveat got written by somebody
looking at the SQL front end and generalising. This file executes it: a real
server, a real client, and the union of two arms.
"""

from __future__ import annotations

from slate import Agg, AggregateQuery, Client, Query, any_of, i64

from .conftest import as_int
from .fixture import DOCS


def test_a_client_sends_a_disjunction_in_a_where(oracle_client: Client) -> None:
    # Thresholds derived from the rows rather than written down: a hard-coded
    # `size > 100` matched nothing and the test failed on its fixture instead
    # of on its subject. The median splits the table, so both arms have rows
    # whatever the seed holds.
    rows = list(oracle_client.query(Query(DOCS)))
    assert len(rows) > 3, "too few rows for a median to split anything"
    sizes = sorted(as_int(row[2]) for row in rows)
    cut = sizes[len(sizes) // 2]
    kind = str(rows[0][1])

    def ids(expr) -> set[int]:
        q = Query(DOCS)
        return {as_int(row[0]) for row in oracle_client.query(q.where(expr(q)))}

    big = ids(lambda q: q.c.size.gt(i64(cut)))
    named = ids(lambda q: q.c.kind.eq(kind))
    both = ids(lambda q: any_of([q.c.size.gt(i64(cut)), q.c.kind.eq(kind)]))

    assert big, f"no row above the median size {cut}"
    assert named, f"no row of kind {kind!r}"
    # The arms must differ, or a server that ANDed them would agree anyway.
    assert big - named or named - big, "the arms do not discriminate on this fixture"
    assert both == big | named
    # And a conjunction really is smaller here, which is what makes the
    # assertion above a test of the connective rather than of the data.
    conjoined = ids(lambda q: q.c.size.gt(i64(cut)) & q.c.kind.eq(kind))
    assert conjoined == big & named
    assert conjoined != both


def test_a_client_sends_a_disjunction_in_a_having(oracle_client: Client) -> None:
    def groups(having) -> set[str]:
        a = AggregateQuery(DOCS)
        a.group_by(a.c.kind)
        a.aggregate(Agg.count())
        if having is not None:
            a.having(having(a))
        return {str(g.key[0]) for g in oracle_client.aggregate(a)}

    # `a.agg(0)` is the first aggregate — the count — in the group space.

    everything = groups(None)
    assert len(everything) > 2, "need several kinds for the arms to differ"

    big = groups(lambda a: a.agg(0).gt(i64(1)))
    small = groups(lambda a: a.agg(0).lt(i64(1)))
    both = groups(lambda a: any_of([a.agg(0).gt(i64(1)), a.agg(0).lt(i64(1))]))

    assert both == big | small
    # And the disjunction is not just "everything": if it were, a server that
    # ignored HAVING entirely would pass.
    assert both != everything or not (everything - big - small), (
        "the arms should leave something out for this to discriminate"
    )
