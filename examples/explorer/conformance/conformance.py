#!/usr/bin/env python3
"""Do the three SDKs answer identically?

Until this file, nothing in the repository compared the clients to each other.
Each client's suite runs against the same server, which catches *a* client being
wrong and cannot catch three being wrong the same way — and cannot catch two
clients quietly disagreeing about something the server accepts from both, which
is the more likely failure.

The three adapters implement one HTTP contract (../CONTRACT.md) over one head
node. This sends every case below to all three and requires the JSON to match
exactly, `sdk` excluded.

Start the stack and run this against it, in one command:

    ./run.sh --conformance

which picks free ports, waits for every adapter to say it is listening, runs
the cases, and tears the stack down again. Against a stack you already have
up, run it directly:

    ./run.sh --headless      # in another terminal
    python3 conformance/conformance.py

Exit status is 0 when they agree.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.error
import urllib.request
from typing import Any, NamedTuple

# The demo's default ports. Overridable, because `./run.sh --conformance`
# starts the whole stack on ports the kernel picked: a suite that can only run
# on three fixed ports is a suite that cannot run twice at once, and one that
# fails confusingly when something else already holds 7431.
DEFAULTS = {
    "go": "http://127.0.0.1:7431",
    "node": "http://127.0.0.1:7432",
    "python": "http://127.0.0.1:7433",
}


def call(base: str, path: str, body: Any, identity: str = "app") -> Any:
    """One request, with the transport's own failures kept distinguishable
    from the adapter's answers."""
    data = json.dumps(body).encode() if body is not None else b"{}"
    request = urllib.request.Request(
        f"{base}{path}",
        data=data,
        headers={"Content-Type": "application/json", "X-Demo-Identity": identity},
        method="POST" if body is not None else "GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as error:
        # A refusal is an answer and has to be compared like one.
        return json.loads(error.read())
    except Exception as error:  # noqa: BLE001 - the adapter is down, not wrong
        return {"__transport__": str(error)}


# Each case is (name, path, body, identity).
#
# Chosen to cover what the three clients could plausibly disagree about: value
# encoding at every type, filter lowering, join semantics including the outer
# cases, group ordering and its tie-break, the plan's text, and the shape of a
# refusal. A case whose answer is the same no matter what the client does —
# `SELECT * LIMIT 1` — proves nothing and is not here.
CASES: list[tuple[str, str, Any, str]] = [
    ("meta", "/api/meta", None, "app"),

    # Every value type, in one row, so an encoding difference shows up as a
    # diff rather than as a rounding nobody notices. No projection on purpose:
    # a column appended to `books` joins this case without anybody remembering
    # to add it, which is how the missing vector arm was caught and how the
    # decimal one would have been.
    ("all types", "/api/query",
     {"table": "books", "filter": {"op": "eq", "column": 0, "value": {"u64": "10"}}}, "app"),

    # A decimal on the *filter* side, which is a different path from reading
    # one back: the value has to be lowered into a predicate, and a client that
    # sent it as an `i64` would match nothing rather than fail — this server
    # orders values type first, so the comparison is a type mismatch and not a
    # wrong answer, but "no rows" is what a caller sees either way.
    ("a filter on a decimal column", "/api/query",
     {"table": "books", "filter": {"op": "ge", "column": 7, "value": {"decimal": "1400"}},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("ordered by a float", "/api/query",
     {"table": "books", "sort": [{"column": 4, "direction": "desc"},
                                 {"column": 0, "direction": "asc"}], "limit": 5}, "app"),

    ("a range filter", "/api/query",
     {"table": "books", "filter": {"op": "ge", "column": 3, "value": {"i64": "1970"}},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("a conjunction", "/api/query",
     {"table": "books",
      "filter": {"op": "and", "parts": [
          {"op": "ge", "column": 3, "value": {"i64": "1960"}},
          {"op": "lt", "column": 3, "value": {"i64": "1990"}}]},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("a negated disjunction", "/api/query",
     {"table": "books",
      "filter": {"op": "not", "part": {"op": "or", "parts": [
          {"op": "eq", "column": 1, "value": {"u64": "1"}},
          {"op": "eq", "column": 1, "value": {"u64": "2"}}]}},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("a pattern", "/api/query",
     {"table": "books", "filter": {"op": "like", "column": 2, "pattern": "The %"},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("a case-insensitive pattern", "/api/query",
     {"table": "books", "filter": {"op": "ilike", "column": 2, "pattern": "the %"},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("an IN list", "/api/query",
     {"table": "authors", "filter": {"op": "in", "column": 0,
                                     "values": [{"u64": "1"}, {"u64": "3"}, {"u64": "99"}]},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    ("a projection", "/api/query",
     {"table": "books", "columns": [0, 2], "sort": [{"column": 0, "direction": "asc"}],
      "limit": 4}, "app"),

    ("an offset past the start", "/api/query",
     {"table": "books", "sort": [{"column": 0, "direction": "asc"}],
      "limit": 3, "offset": 5}, "app"),

    # Reading through a view, which is the only thing a view can demonstrate:
    # `docs/views.md` refuses a projection, a sort and a limit, so a view is a
    # name for a `WHERE` and the question is which rows it admits.
    #
    # `classics` is `year < 1980`, which is eight of eleven books. `app` holds
    # a policy that admits everything, so this is the view's own answer.
    ("a read through a view", "/api/query",
     {"table": "classics", "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    # The same view, the same request, a different caller — and a different
    # answer, which is the case worth having.
    #
    # `reader`'s row policy on `books` admits `year >= 1960`, so the two books
    # from 1955 and 1951 are inside the view and outside the policy and this
    # returns six rows rather than eight. That is `docs/views.md` §1: the view
    # is substituted away before planning, the *base* table's `TableId` reaches
    # `row_filter_with`, and the policy that runs is `books`'s. A view carrying
    # its own id would have had no policy at all and handed a reader all eight,
    # so a single-identity case would have passed against a privilege
    # escalation.
    ("a reader's read through the same view", "/api/query",
     {"table": "classics", "sort": [{"column": 0, "direction": "asc"}]}, "reader"),

    # And the caller's own filter, which composes with the view's rather than
    # replacing it: `year >= 1970` over a view of `year < 1980` is the decade
    # between, not the whole of either. An `OR` here would return everything
    # from 1970 on, which is how a client that got the composition backwards
    # would look.
    ("a filter composed with a view's", "/api/query",
     {"table": "classics", "filter": {"op": "ge", "column": 3, "value": {"i64": "1970"}},
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),

    # A view is not a table, and every path but `query` says so.
    #
    # All three adapters share their query builder with `/api/explain`, so all
    # three accept the name locally and the *server* refuses — which makes this
    # the cross-SDK check of that refusal's wording as well as of its kind.
    # It reads "`classics` is a view over `books`, and only a plain query can
    # read through one", which is the message `Head::no_such_table` produces
    # and the reason it exists: "no table named `classics`" would send an
    # operator to check a spelling that is correct.
    ("explain cannot name a view", "/api/explain", {"table": "classics"}, "app"),

    # The four join types, where the outer cases are the ones a client can get
    # subtly wrong: an unmatched side must be null, not a row of nulls.
    *[(f"a {kind} join", "/api/join", {"type": kind}, "app")
      for kind in ("inner", "left", "right", "full")],

    # Three tables in one read. A chain is not a separate RPC — `JoinQuery`
    # carries as many inputs as it is given — so what is being compared is how
    # each SDK spells the third input's attachment: it joins back to the
    # *second*, and a client that attached it to the first would produce a
    # cross join with exactly the right number of columns and far too many
    # rows. All four types, because the outer cases are where a chain's
    # unmatched sides are hardest to get right: `Author Unknown` has a book and
    # no author, `Ann Leckie` has no books at all, and every book has a sale.
    *[(f"a {kind} chain", "/api/chain", {"type": kind}, "app")
      for kind in ("inner", "left", "right", "full")],

    # And as a reader, whose row policy hides a book — so the chain has to lose
    # that book *and* the sale hanging off it, which a client resolving the
    # third step itself would not.
    ("a reader's chain", "/api/chain", {"type": "inner"}, "reader"),

    # Windows, which are the one operator whose answer comes back in a list of
    # its own. Every other case here compares columns; these compare the
    # *placement* as well, because an adapter that folded a window value into
    # the row would return something the caller reads as a different thing —
    # and each SDK's own suite checks that against its own server, which
    # cannot catch all three doing it the same way.
    #
    # Six functions, partitioned and not: a partition is the difference
    # between "this row's rank among its author's books" and "among all of
    # them", and a client that dropped the clause answers the second question
    # with no error anywhere.
    *[(f"a {fn} window{' per author' if partition else ''}", "/api/window",
       {"function": fn, "partition": partition, "limit": 20}, "app")
      for fn in ("rowNumber", "rank", "denseRank", "lag", "lead", "sum", "count")
      for partition in (True, False)],

    # And the frame rule, which is where SQL surprises people: an aggregate
    # with no order is the whole partition repeated on every row, and the same
    # aggregate *with* one is a running value. Not a different spelling — the
    # standard's default frame changing — so a client that always sent an
    # order, or never did, disagrees here and nowhere else.
    *[(f"a running {fn} window", "/api/window",
       {"function": fn, "partition": True, "running": True, "limit": 20}, "app")
      for fn in ("sum", "count")],

    # As a reader, whose row policy hides `The Astronauts`: the window is
    # computed over what the policy admits, so author 4's numbering has to be
    # two rows rather than three in all three SDKs. A client that windowed
    # before the filter — which is not a thing this protocol can express, and
    # is exactly the mistake the refusal on a keyset page exists to prevent —
    # would report a rank nobody can see the row for.
    ("a reader's window", "/api/window",
     {"function": "rowNumber", "partition": True, "limit": 20}, "reader"),

    # Full-text, by index and by scan. Both must return the same rows — that
    # is what an access path *is* — so the rows alone would agree across three
    # SDKs even if one ignored the hint entirely. `access` is in the answer for
    # that reason, and the MUST_DIFFER pairs below compare the two.
    #
    # Three searches, each testing a different property of the tokenizer, and
    # each run down both paths:
    #
    # - "the" is five titles, which is the ordinary case.
    # - "the games" is one — *The Player of Games* — because the terms are
    #   conjunctive. A client that sent only the first term, or that turned
    #   the search into a disjunction, answers five here.
    # - "GAMES" is the same one title, written in the other case. A client
    #   that folded case itself would agree; a client that did not fold at all
    #   answers nothing. Neither is distinguishable from the correct answer
    #   without this case, because the *server* is what folds.
    *[(f"a search for {text!r} by {path}", "/api/search",
       {"text": text, "path": path, "limit": 20}, "app")
      for text in ("the", "the games", "GAMES")
      for path in ("index", "scan")],

    # A term that is a prefix of a real one and not a term itself. `LIKE
    # '%game%'` finds *The Player of Games*; this finds nothing, and the
    # difference is the whole reason both predicates exist. An adapter that
    # quietly lowered `contains` to a `like` would pass every case above and
    # fail this one.
    #
    # What none of these cases catches, established by trying it: removing
    # `text = true` from the demo's catalog leaves every one of them green and
    # the MUST_DIFFER pairs satisfied. An ordinary index on `title` accepts the
    # same hint and the plan summary names the index without its key range, so
    # the two are indistinguishable over HTTP. That is not a hole to plug here
    # — three SDKs compared to each other cannot see it, because all three
    # would be equally wrong — and it is asserted in `slate-serverd`'s
    # `schema.rs` tests and the kernel's `fulltext.rs` instead.
    ("a search for a word that is only a prefix", "/api/search",
     {"text": "game", "path": "index", "limit": 20}, "app"),

    # As a reader, whose row policy hides `The Astronauts` (1951). It holds
    # "the", so the policy has to cut the five down to four — through the
    # *index*, which is the path where a policy is easiest to lose: the
    # entries are read before any row is, and a filter applied only to a table
    # scan would show up here and nowhere else in this corpus.
    ("a reader's search", "/api/search",
     {"text": "the", "path": "index", "limit": 20}, "reader"),

    # `decade` is the only case here whose group key is not a column: it is a
    # value the *join* computes, `books.year / 10 * 10`. Every SDK builds that
    # expression itself, so this is the one case that compares three
    # independent `Scalar` surfaces rather than three ways of naming a column —
    # which is what all three refused to do until they had one.
    ("a grouped join by a computed decade", "/api/aggregate",
     {"groupBy": "decade", "sort": "key", "direction": "asc"}, "app"),

    ("a computed decade, ordered by count", "/api/aggregate",
     {"groupBy": "decade", "sort": "count", "direction": "desc"}, "app"),

    ("a computed decade with a HAVING", "/api/aggregate",
     {"groupBy": "decade", "having": {"minCount": 2}, "sort": "key",
      "direction": "asc"}, "app"),

    ("explaining a grouped join over a computed decade", "/api/explain-aggregate",
     {"groupBy": "decade", "sort": "key", "direction": "asc"}, "app"),

    # A computed key of every *kind*, not just every column. `decade` above is
    # integer division, which is the one operation every language spells
    # identically — so three clients agreeing about it said much less than it
    # looked. Each of these exercises a different `Scalar` family, and each is
    # built independently by all three SDKs.
    *[(f"a grouped join by a computed {name}", "/api/aggregate",
       {"groupBy": key, "sort": "key", "direction": "asc"}, "app")
      for name, key in (
          ("upper-cased author", "shout"),
          ("CASE over the year", "era"),
          ("regular expression over the title", "tidy"),
          ("calendar year", "releasedYear"),
          ("calendar month boundary", "releasedMonth"),
          ("local hour in New York", "releasedHourNY"),
          ("concatenation across both tables", "label"),
          # Money. `discounted` is the case worth the most here: a decimal
          # literal has no scale of its own and takes the column's, so all
          # three clients have to send `Decimal(50)` rather than the integer
          # 50 that each of their languages would reach for first — and an
          # adapter that sent the integer is refused by the server rather than
          # answering differently, which is a failure the three-way comparison
          # would otherwise never see.
          ("discounted price", "discounted"),
          ("doubled price", "doubled"),
      )],

    # The refusal half, which is the claim the positive cases cannot make:
    # `books.price + books.year` is money plus a count of nothing, refused by
    # the *server* at plan time. All three clients must surface the same
    # refusal, so an adapter that validated locally — or one that let the
    # expression through and rendered a column of nulls — fails here.
    ("a grouped join by a decimal added to a plain number", "/api/aggregate",
     {"groupBy": "badPrice", "sort": "key", "direction": "asc"}, "app"),

    # Ordered by count rather than by key for two of them, because the
    # tie-break is what a client can silently drop — and a computed string key
    # ties far more often than a decade does.
    ("a computed era, ordered by count", "/api/aggregate",
     {"groupBy": "era", "sort": "count", "direction": "desc"}, "app"),
    ("a computed local hour, ordered by count", "/api/aggregate",
     {"groupBy": "releasedHourNY", "sort": "count", "direction": "desc"}, "app"),

    # And the plan of one, which narrows each input's projection to the
    # columns the expression reads — a different set from a bare column's.
    ("explaining a grouped join over a calendar month", "/api/explain-aggregate",
     {"groupBy": "releasedMonth", "sort": "key", "direction": "asc"}, "app"),

    # Nearest-neighbour search: the last `Scalar` family the three SDKs were
    # never compared on. The *order* is the answer — the distances are f64 and
    # the three clients format floats differently.
    ("nearest by cosine distance", "/api/nearest", {"limit": 5}, "app"),
    # Every book, so the comparison is the whole ranking rather than its head.
    ("the whole ranking by cosine distance", "/api/nearest", {}, "app"),
    # And what a reader sees, since the row policy hides a book: the ranking
    # has to be of the rows this identity may see, not of all of them.
    ("a reader's nearest", "/api/nearest", {"limit": 5}, "reader"),

    ("a grouped join by author", "/api/aggregate",
     {"groupBy": "author", "sort": "count", "direction": "desc"}, "app"),

    ("a grouped join by country", "/api/aggregate",
     {"groupBy": "country", "sort": "key", "direction": "asc"}, "app"),

    # Equal counts, so the tie-break decides the order and a client that
    # dropped it disagrees with the other two.
    ("a grouping with ties", "/api/aggregate",
     {"groupBy": "author", "sort": "count", "direction": "asc"}, "app"),

    ("a grouping with HAVING", "/api/aggregate",
     {"groupBy": "author", "having": {"minCount": 3}, "sort": "key", "direction": "asc"}, "app"),

    ("a limited grouping", "/api/aggregate",
     {"groupBy": "author", "sort": "count", "direction": "desc", "limit": 2}, "app"),

    ("a plan", "/api/explain", {"table": "books"}, "app"),

    # The plan of a *grouped* read. Separate from the one above because
    # grouping narrows each input's projection, so the two describe different
    # reads — and `decodes` is the field that says so, since the access path is
    # unchanged wherever no index applies.
    ("a grouped plan", "/api/explain-aggregate",
     {"groupBy": "author", "sort": "count", "direction": "desc"}, "app"),
    ("a grouped plan by country", "/api/explain-aggregate",
     {"groupBy": "country", "sort": "key", "direction": "asc"}, "app"),
    ("a reader may not explain a grouping", "/api/explain-aggregate",
     {"groupBy": "author"}, "reader"),

    ("a plan under a filter", "/api/explain",
     {"table": "books", "filter": {"op": "eq", "column": 0, "value": {"u64": "10"}}}, "app"),

    # Row-level security. The three must agree on what a reader may see, since
    # none of them enforces it.
    ("what a reader sees", "/api/query",
     {"table": "books", "sort": [{"column": 0, "direction": "asc"}]}, "reader"),

    ("a reader's grouped join", "/api/aggregate",
     {"groupBy": "author", "sort": "key", "direction": "asc"}, "reader"),

    # Refusals are answers. The kind must be spelled identically or a caller
    # switching SDKs has to rewrite their error handling.
    ("a reader may not explain", "/api/explain", {"table": "books"}, "reader"),
    ("a stranger may not read", "/api/query", {"table": "books"}, "stranger"),
    ("no such table", "/api/query", {"table": "nope"}, "app"),
    ("no such filter operator", "/api/query",
     {"table": "books", "filter": {"op": "approximately", "column": 0}}, "app"),
    ("no such grouping", "/api/aggregate", {"groupBy": "century"}, "app"),

    # Relationships. One read resolves a page of parents, and the three SDKs
    # each group the response back onto the caller's own key order — which is
    # where they can disagree without the server noticing, since the server
    # sends one group per *distinct* key and says nothing about the order the
    # caller asked in.
    #
    # `sales.book_id -> books` is the demo's only foreign key; see head.toml
    # for why it is not `books.author_id`.
    ("the sales of several books", "/api/related",
     {"way": "children", "keys": [{"u64": "10"}, {"u64": "11"}, {"u64": "12"}]}, "app"),

    # A book nothing has sold, so a client that dropped the empty group rather
    # than returning it disagrees. 21 does not exist at all, which is the same
    # answer by a different route and must also be an empty group rather than
    # a refusal.
    ("a book with no sales, and a book that does not exist", "/api/related",
     {"way": "children", "keys": [{"u64": "21"}, {"u64": "10"}]}, "app"),

    # The same key twice. The server sends one group; each client has to hand
    # it back twice, and one that returned two groups or one would differ.
    ("a repeated key", "/api/related",
     {"way": "children", "keys": [{"u64": "10"}, {"u64": "10"}]}, "app"),

    ("no keys at all", "/api/related", {"way": "children", "keys": []}, "app"),

    # Backwards: the books behind a page of sales. Sales 100 and 110 point at
    # books 10 and 20, and two sales of one book collapse to one group.
    ("the books behind several sales", "/api/related",
     {"way": "parents", "keys": [{"u64": "10"}, {"u64": "20"}, {"u64": "10"}]}, "app"),

    # And the same question as a `reader`, whose row policy hides book 20
    # (published 1951). The relationship load has to lose that book exactly as
    # a direct read would — a client resolving the relationship itself would
    # not, which is the security argument for the RPC existing at all.
    ("a reader's books behind several sales", "/api/related",
     {"way": "parents", "keys": [{"u64": "10"}, {"u64": "20"}]}, "reader"),

    # `parents`, not `children`: a stranger *is* granted read on `sales`, and
    # only `books` and `authors` are denied. Written the other way round first,
    # where it quietly returned rows and asserted nothing.
    ("a stranger may not load a relationship", "/api/related",
     {"way": "parents", "keys": [{"u64": "100"}]}, "stranger"),

    # The refusal has to name the keys that do exist, identically in all three.
    ("no such foreign key", "/api/related",
     {"way": "children", "through": "nosuch", "keys": [{"u64": "10"}]}, "app"),

    # A relationship *path*: `sales -> books -> editions`, two steps in
    # opposite directions, one request.
    #
    # This is where a three-way disagreement about the regrouping shows up and
    # nowhere else. The server sends flat levels plus, per level, the ordinal
    # in the level above where its key is found; each SDK rebuilds the tree
    # from that, and three separate implementations of one walk is exactly the
    # shape that drifts. Book 10 has two editions, 11 has one, and 12 has none
    # — so the answer is not uniform and a client that grouped by the wrong
    # ordinal would still produce *an* answer.
    ("a two-step path, with the middle level kept", "/api/path",
     {"keys": [{"u64": "10"}, {"u64": "11"}, {"u64": "12"}]}, "app"),
    # A key relating to nothing at either level: 12 has a book and no editions,
    # 99 has no book at all. The first ends with an empty `related`, the second
    # with an empty tree, and the difference between those two is the thing a
    # hand-written regrouping gets wrong.
    ("a path whose keys run out at different levels", "/api/path",
     {"keys": [{"u64": "12"}, {"u64": "99"}]}, "app"),
    # The row policy applies at every level, not only the first. `reader` sees
    # books published from 1960, so a path through `books` loses the same rows
    # for all three SDKs — and a client that resolved the path itself would
    # not lose them at all.
    ("a reader's path loses what the policy hides", "/api/path",
     {"keys": [{"u64": "10"}, {"u64": "11"}, {"u64": "12"}]}, "reader"),

    # Keyset pagination. The cursor is the server's, built from the last row's
    # primary key, so what is compared is whether the three clients read it off
    # the same message and hand it back in the same shape — a cursor that
    # arrived as a bare number rather than a `u64` would look identical in one
    # client and fail against the others.
    ("the first page of books", "/api/page", {"limit": 4}, "app"),

    # The second page, by the cursor the first one returns. Written out rather
    # than threaded, so the corpus stays a list of independent requests: book
    # 13 is the fourth by id, so this is what the first page's cursor is.
    ("the second page, resumed from a cursor", "/api/page",
     {"limit": 4, "after": [{"u64": "13"}]}, "app"),

    # Short, so there is provably nothing after it and `cursor` is absent in
    # all three rather than empty in one of them.
    ("a page larger than the table", "/api/page", {"limit": 100}, "app"),

    # The last *full* page still carries a cursor, and the page after it is
    # empty — the documented cost of not reading one row ahead every time.
    ("the page after the last row", "/api/page",
     {"limit": 4, "after": [{"u64": "20"}]}, "app"),

    # As a reader, whose row policy hides a book: the page is one row short of
    # its limit and must be so in all three, which a client that counted rows
    # before the policy applied would get wrong.
    ("a reader's first page", "/api/page", {"limit": 4}, "reader"),

    ("a page with no limit", "/api/page", {}, "app"),
    ("a page whose projection drops the key", "/api/page",
     {"limit": 4, "columns": [1, 2]}, "app"),
    # The kernel's own refusal, reaching the wire: a sort into an order the
    # primary key does not give means the page boundary is not where the cursor
    # says. Refused rather than paged wrongly, and all three must say so.
    ("a page sorted into an order the key does not give", "/api/page",
     {"limit": 4, "sort": [{"column": 4, "direction": "desc"}]}, "app"),

    # Predicate writes, and `returning`. Each case seeds its own four rows in
    # an id range clear of the fixture, so the three adapters see the same
    # starting state whichever runs first and a re-run gives the same answer.
    #
    # `left` is in the answer because `affected` is the server's report and
    # `left` is what the table says: a delete that reported three and removed
    # none would agree across three clients on the number it made up.
    ("a predicate delete, returning what it destroyed", "/api/predicate-write",
     {"kind": "delete", "returning": True}, "app"),
    # The same write without `returning`: the rows must be absent rather than
    # returned anyway, which is the direction a client is likeliest to get
    # wrong by ignoring the flag.
    ("a predicate delete, not returning", "/api/predicate-write",
     {"kind": "delete"}, "app"),
    ("a predicate update, returning the rows as written", "/api/predicate-write",
     {"kind": "update", "returning": True}, "app"),
    # Refused rather than reported as zero rows written, because zero is what a
    # predicate that matched nothing reports and the two are different
    # mistakes. All three must refuse it, and with the same message.
    ("a predicate update with no assignments", "/api/predicate-write",
     {"kind": "update", "noSet": True}, "app"),
    # A reader may not write, and must be refused before anything is removed.
    ("a reader may not write by predicate", "/api/predicate-write",
     {"kind": "delete"}, "reader"),

    # A conditional update, landing and refused. The pair is the case: an
    # unconditional update and a conditional one over an *unchanged* row do
    # exactly the same thing, so an adapter that built the expected rows and
    # never sent them would pass the first of these and fail the second. That
    # is not hypothetical — it is the mutation that survived in two of the
    # three clients before their own suites grew a stale-row test.
    #
    # `rendered` is the only place in this corpus where the three clients'
    # *decimal renderers* are compared: a `{"decimal": ...}` tag is the units
    # and carries no scale, so each adapter renders against the 2 it declares
    # locally and three implementations of "put the point two from the right"
    # have to agree.
    ("a conditional update over the row the caller read", "/api/conditional-update",
     {"stale": False}, "app"),
    ("a conditional update over a row that moved", "/api/conditional-update",
     {"stale": True}, "app"),

    # The delete half, in its three states. The third is the case that is not
    # the update's twin: a plain delete of an absent key reports zero and no
    # error, and a conditional one refuses. Three clients agreeing that it is
    # *not-found* rather than *conflict* is the claim — each maps the server's
    # code through its own error taxonomy, so this is the one place those three
    # taxonomies are compared on a code neither of the other endpoints
    # produces.
    ("a conditional delete over the row the caller read", "/api/conditional-delete",
     {}, "app"),
    ("a conditional delete over a row that moved", "/api/conditional-delete",
     {"stale": True}, "app"),
    ("a conditional delete of a row somebody else removed", "/api/conditional-delete",
     {"gone": True}, "app"),

    # A batch, under each atomicity, with the same three operations — one of
    # which collides. That collision is the whole comparison: independent
    # reports it and keeps going, atomic fails the call and undoes the rest,
    # and `left` afterwards is how the corpus sees which happened. Three
    # clients agreeing on *both* answers is what says the distinction survived
    # three separate implementations of it.
    ("an independent batch, reporting a failure it carried on past", "/api/batch",
     {"atomicity": "independent"}, "app"),
    ("an atomic batch, undoing everything before the failure", "/api/batch",
     {"atomicity": "all-or-nothing"}, "app"),
    # A reader may not write, batched or not, and must be refused before
    # anything lands.
    ("a reader may not batch", "/api/batch", {"atomicity": "independent"}, "reader"),

    ("a committed transaction", "/api/transaction", {"commit": True}, "app"),
    ("a rolled-back transaction", "/api/transaction", {"commit": False}, "app"),

    # Soft delete, across the wire, in all three clients.
    #
    # `shipments` seeds four rows and retires one. These three cases are the
    # whole of the convention: what an ordinary read sees, what lifting the
    # filter adds, and who is allowed to lift it.
    #
    # The first two are a pair on purpose. "Four rows came back" proves
    # nothing on its own — a server that ignored the flag entirely would also
    # return four — so the case that matters is the *difference* between them,
    # and the corpus only sees a difference if it asks both ways.
    ("a read that cannot see a retired row", "/api/query",
     {"table": "shipments", "sort": [{"column": 0, "direction": "asc"}]}, "app"),
    ("a read that asks for retired rows too", "/api/query",
     {"table": "shipments", "includeDeleted": True,
      "sort": [{"column": 0, "direction": "asc"}]}, "app"),
    # And the refusal. `reader` holds `read` on `shipments` and not
    # `read_deleted`, which is the entire reason the action is separate: a
    # server that folded it into `read` would answer this with rows.
    # And the other half of soft delete: erasing what was retired.
    #
    # Each adapter seeds its own three rows, retires two and purges, so the
    # three runs of this case do not depend on each other — the first purge
    # erases the rows, and without the re-seed the second and third would find
    # nothing and disagree.
    #
    # `left` is what survives, retired rows included, which is what separates
    # "erased" from "still there but hidden". A purge that erased the live row
    # too would agree across all three clients and be very wrong.
    ("a purge erases what was retired and nothing else", "/api/purge", {}, "app"),

    # The *other* other half: taking a delete back. A retired row used to be
    # writable by nobody at any privilege, so a retention window could only
    # ever end in the row being erased. All three clients now generate a
    # `restored` helper from the catalog, so all three have to agree on what it
    # produces — which column it clears, and that it clears nothing else.
    #
    # The answer carries the row's state at three points rather than only the
    # last. "It is live now" is also what an adapter that quietly inserted a
    # fresh row at the same key would report, and `status_after` and
    # `book_id_after` are what tell the two apart.
    ("a retired row can be restored", "/api/restore", {}, "app"),

    # And the mistake anybody makes first: reading the row with
    # `include_deleted`, which hands back the retirement stamp, and writing it
    # straight back. It is refused, and the refusal is its own reason token
    # rather than a row-level-security one — which matters because the old
    # message sent the reader to the grants on a table with no policies at all.
    # Here so the three clients are compared on the refusal too, not only on
    # the path that works.
    ("the soft-delete column is not the caller's to write",
     "/api/restore-unchanged", {}, "app"),

    ("a reader may not ask for retired rows", "/api/query",
     {"table": "shipments", "includeDeleted": True,
      "sort": [{"column": 0, "direction": "asc"}]}, "reader"),

    # A refusal with *structure*, and with more than one entry in it.
    #
    # Every other refusal is compared on `kind` and `reason` — two strings the
    # server hands over whole. This one is compared on `violations`, which no
    # server hands over: each client hand-decodes it out of
    # `ErrorInfo.metadata`, counting up from a `violations` key and reading
    # `check.N`, `column.N`, `message.N`. Three hand-written parsers of one
    # undeclared shape is the most drift-prone thing in these clients, and
    # until this case existed each was checked only against a recording of the
    # bytes. A recording cannot notice that the server started indexing from
    # one.
    ("a write the schema's CHECK refuses", "/api/bad-status", {}, "app"),

    # The generated *decoders*, over rows a real server sent.
    #
    # Every other case here compares what the three clients do with values;
    # this one compares what they do with values *after the generated row type
    # has read them by ordinal*. The decoders had a suite each and both built
    # their rows by hand, so all three agreed with their own idea of what the
    # server sends — and until this route existed, no code path outside a test
    # called a generated decoder at all.
    #
    # A `books` row and a `shipments` row, so the answer covers a u64, a
    # string, an i64, a decimal, a vector, a nullable column and an enumerated
    # one. Integers are spelled as decimal strings because one of the three
    # reads them as `bigint`.
    ("two rows through the generated decoders", "/api/typed", {}, "app"),

    # The same typed refusal, through a *batch*.
    #
    # A batch reports each failure as data inside a successful response, so
    # there are no trailers and no `grpc-status-details-bin`; this was the one
    # path that could not carry `violations` at all, and a form submitted as a
    # batch got the token and the prose. The server puts the same blob in the
    # message body now and each client decodes it with the function it already
    # had.
    #
    # Two rows, refused differently — one breaks two checks and one breaks one
    # — because an adapter that reported the same list for every failed
    # operation would otherwise look right. Nothing is written, so the case
    # leaves the database as it found it.
    ("a batch where two rows are refused differently", "/api/bad-batch", {}, "app"),
]


#: Cases whose right answer *is* a refusal.
#:
#: Everything else must come back without an `error`, and that guard matters
#: more than it looks: three adapters returning the identical error agree, so a
#: case that silently became unserveable would keep passing. That is exactly
#: what happened here — `{"groupBy": "decade"}` was a refusal case ("a grouping
#: that needs a computed column") until all three SDKs grew a way to declare
#: one, and without this list the four new cases that group by a computed
#: decade would have passed just as happily if the feature had never worked.
#:
#: Listed by name rather than by a fifth tuple element, so adding a case is
#: still one line and forgetting to mark it fails loudly rather than quietly.
EXPECTED_REFUSALS = {
    "a reader may not explain",
    "a reader may not explain a grouping",
    "a stranger may not read",
    "no such table",
    "no such filter operator",
    "no such grouping",
    "a grouped join by a decimal added to a plain number",
    "no such foreign key",
    "a stranger may not load a relationship",
    "a page with no limit",
    "a page whose projection drops the key",
    "a page sorted into an order the key does not give",
    "a predicate update with no assignments",
    "a reader may not write by predicate",
    "a reader may not batch",
    # Not a refusal of the *read* — `reader` may read `shipments`. It is a
    # refusal of lifting the soft-delete filter, which is the whole reason
    # `read_deleted` is an action of its own rather than part of `read`.
    "a reader may not ask for retired rows",
    # The row is refused by `shipments.status_known` before it is written, so
    # there is nothing to undo — unlike the purge case above, which has to put
    # the database back.
    "a write the schema's CHECK refuses",
    # Refused by the server, not by any grant: the soft-delete column is
    # written by `delete` and by nothing else. The adapter puts the database
    # back either way, because unlike the CHECK above this one had to retire a
    # row to have something to write back.
    "the soft-delete column is not the caller's to write",
    # `classics` exists; `explain` is not a path that may read through it.
    # Refused by the *server* rather than by any adapter's allowlist — all
    # three share their query builder with `/api/explain`, so all three send
    # the name — which makes this the cross-SDK check of the refusal's
    # wording as well as of its kind.
    "explain cannot name a view",
}


#: Pairs of cases whose answers must **differ** from each other.
#:
#: Three clients agreeing is the whole point of this runner and it cannot see
#: one class of bug: a request field that every client drops. All three then
#: send the same smaller request, get the same smaller answer, and agree
#: perfectly about it.
#:
#: `includeDeleted` is exactly that shape. "The read returned four rows" proves
#: nothing on its own — a server ignoring the flag returns four too, if four is
#: what the plain read returns. The evidence is that asking changes the answer,
#: which needs two cases and a comparison between them, and this is where that
#: comparison lives.
MUST_DIFFER: list[tuple[str, str]] = [
    ("a read that cannot see a retired row", "a read that asks for retired rows too"),
    # A view has the same shape as `includeDeleted` and it is the shape that
    # matters most: "the view returned eight rows" proves nothing about the
    # row policy, because a view carrying its own `TableId` — the design
    # `docs/views.md` §1 spends four paragraphs refusing — has no policy at all
    # and returns *the same eight* to a reader. Three clients would agree
    # about it perfectly.
    #
    # The evidence that the base table's policy ran is that a different caller
    # gets a different answer, which needs two cases and this comparison. Six
    # rows against eight; the two books from 1955 and 1951 are inside the view
    # and outside `modern_only`.
    ("a reader's read through the same view", "a read through a view"),
    # And that the caller's filter *composed* rather than being dropped: a
    # server that ignored it would return the view's own eight rows, which is
    # what this pair forbids. The other direction — a composition that widened
    # to an `OR` — is not visible here, because `year >= 1970` over an `OR`
    # returns more than either, and `crates/slate-serverd/tests/views.rs`
    # asserts that one against a running server instead.
    ("a filter composed with a view's", "a read through a view"),
    # `returning` has the same shape and was demonstrated to have the same
    # hole: every `Returning` in all three clients set to false — twelve call
    # sites — and the run stayed green at 96 cases agreeing. These two cases
    # were already here, adjacent, describing each other in their comments, and
    # nothing compared them.
    ("a predicate delete, returning what it destroyed", "a predicate delete, not returning"),
    # `partition` and `running` are the two window fields with exactly this
    # weakness: a client that dropped either sends a smaller request, gets a
    # smaller answer, and agrees with two other clients doing the same. Nothing
    # refuses without them, so neither is covered by `EXPECTED_REFUSALS`.
    #
    # `rowNumber` for the partition, because it is the function where dropping
    # the clause is most visibly wrong and least visibly an error: unpartitioned
    # it numbers 1..11 straight through, which is a column of plausible
    # integers.
    ("a rowNumber window per author", "a rowNumber window"),
    ("a sum window per author", "a sum window"),
    # And the frame: the same aggregate over the same partition, with and
    # without the window's own order. One is the total on every row and the
    # other is a running value, and the standard says the order is what decides
    # — so a client that always sent one, or never did, is wrong here and
    # nowhere else.
    ("a running sum window", "a sum window per author"),
    ("a running count window", "a count window per author"),
    # And the access path, which has exactly this weakness in its purest form:
    # the two requests are *required* to return the same rows, so an adapter
    # that ignored `path` agrees with two others doing the same on every row of
    # every search above. `access` is the only field that can differ, and
    # nothing refuses a query whose hint went missing — a hint is advice, so
    # `EXPECTED_REFUSALS` cannot cover this either.
    ("a search for 'the' by index", "a search for 'the' by scan"),
    ("a search for 'the games' by index", "a search for 'the games' by scan"),
]

#: What `MUST_DIFFER` is for, and what it is *not* needed for.
#:
#: A request field that every client drops is invisible to "three clients
#: agree": all three send the same smaller request and agree about the same
#: smaller answer. Three separate things protect against that, and knowing
#: which one covers a field is the difference between a pair worth adding and
#: one that is noise:
#:
#: 1. **A refusal case.** If dropping the field turns a refusal into an answer,
#:    `EXPECTED_REFUSALS` catches it — the runner reports the list as stale.
#:    This covers `paged`, which was assumed to need a pair until dropping it
#:    in all three clients was tried: it broke three refusal cases, not the
#:    page contents.
#: 2. **A `MUST_DIFFER` pair.** For a field that changes an *answer* and
#:    nothing refuses without it. `include_deleted` and `returning`.
#: 3. **Nothing.** A field whose absence changes neither, which is a field
#:    doing nothing observable — worth knowing about for its own sake.
#:
#: Adding a field to the protocol means deciding which of these applies, and
#: the only honest way to know is to drop it in all three clients and run.


def normalise(answer: Any) -> Any:
    """Strip the one field that is allowed to differ.

    `sdk` names the adapter and is the only legitimate difference. Everything
    else — including error messages, which come from the server — must match.
    """
    if isinstance(answer, dict):
        return {k: normalise(v) for k, v in sorted(answer.items()) if k != "sdk"}
    if isinstance(answer, list):
        return [normalise(v) for v in answer]
    return answer


class Finding(NamedTuple):
    """One thing that went wrong, and which case (if any) it belongs to.

    `case` is the name of the case that failed, or `None` for a finding that
    is not about a single case — a `MUST_DIFFER` pair is a relationship
    *between* two cases and can fail while both of them pass.

    A list of display strings was what this held before, and the tally counted
    the lines beginning `FAIL`. That gets both numbers wrong the moment a
    finding is not one-to-one with a case: a `MUST_DIFFER` pair returning the
    same answer when all 130 cases agreed printed `129 passed, 1 failed`,
    where the truth is 130 passed and one finding that is not a case at all.
    """

    case: str | None
    lines: list[str]


def tally(cases: int, findings: list[Finding]) -> tuple[int, int]:
    """How many cases passed, and how many findings there are.

    Separate counts because they count different things, which is the whole
    defect this replaces. A case that produces two findings is still one case
    that failed; a finding about no case subtracts from nothing.
    """
    broken = {f.case for f in findings if f.case is not None}
    return cases - len(broken), len(findings)


def must_differ_findings(agreed_by_name: dict[str, str]) -> list[Finding]:
    """Every `MUST_DIFFER` pair that did not, as findings about no case.

    Lifted out of `main` so it can be tested: it takes the agreed answers and
    returns findings, with no adapter anywhere, which is the only reason the
    `case=None` below is checked by anything. It was not, and a mutation that
    attributed these to one of the pair's cases survived a suite that tested
    the arithmetic and not what fed it.

    `case=None` because a pair is a relationship *between* two cases. Both can
    agree across the three SDKs — both pass — and still fail this, because
    what fails is that they agree with each *other*. Blaming either would take
    a passing case off the count.
    """
    findings: list[Finding] = []
    for quiet, loud in MUST_DIFFER:
        if quiet not in agreed_by_name or loud not in agreed_by_name:
            # One of them already failed, or is missing from CASES entirely —
            # the second is worth saying out loud, because a renamed case would
            # otherwise turn this check off silently.
            missing = [n for n in (quiet, loud) if n not in agreed_by_name]
            findings.append(Finding(None, [
                f"FAIL  the must-differ pair ({quiet!r}, {loud!r}) is not "
                f"comparable: {', '.join(repr(n) for n in missing)} produced no "
                f"agreed answer"
            ]))
            continue
        if agreed_by_name[quiet] == agreed_by_name[loud]:
            findings.append(Finding(None, [
                f"FAIL  {quiet!r} and {loud!r} returned the same answer, so "
                f"whatever separates them was dropped by all three clients or "
                f"ignored by the server",
                f"    {agreed_by_name[quiet][:400]}",
            ]))
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verbose", action="store_true", help="print every case")
    for sdk, default in DEFAULTS.items():
        parser.add_argument(f"--{sdk}", default=default, metavar="URL",
                            help=f"the {sdk} adapter's base URL (default {default})")
    args = parser.parse_args()

    adapters = {sdk: getattr(args, sdk) for sdk in DEFAULTS}

    findings: list[Finding] = []
    # Every case's agreed answer, for the `MUST_DIFFER` check below. Only the
    # cases where all three agreed are recorded: a disagreement is already a
    # failure and comparing one of three answers to another case would say
    # nothing about which.
    agreed_by_name: dict[str, str] = {}
    for name, path, body, identity in CASES:
        answers = {
            sdk: normalise(call(base, path, body, identity)) for sdk, base in adapters.items()
        }

        down = [sdk for sdk, a in answers.items()
                if isinstance(a, dict) and "__transport__" in a]
        if down:
            findings.append(Finding(name, [
                f"FAIL  {name}: adapters unreachable: {', '.join(down)}",
                *(f"    {sdk}: {answers[sdk]['__transport__']}" for sdk in down),
            ]))
            continue

        rendered = {sdk: json.dumps(a, sort_keys=True) for sdk, a in answers.items()}
        if len(set(rendered.values())) == 1:
            # Agreement is necessary and not sufficient: see EXPECTED_REFUSALS.
            agreed = next(iter(answers.values()))
            refused = isinstance(agreed, dict) and "error" in agreed
            if refused and name not in EXPECTED_REFUSALS:
                findings.append(Finding(name, [
                    f"FAIL  {name} ({identity}): all three refused it, and this "
                    f"case is supposed to return an answer",
                    f"    {json.dumps(agreed)[:400]}",
                ]))
                continue
            if not refused and name in EXPECTED_REFUSALS:
                findings.append(Finding(name, [
                    f"FAIL  {name} ({identity}): listed as a refusal and all "
                    f"three answered it; the list is stale"
                ]))
                continue
            agreed_by_name[name] = rendered[next(iter(rendered))]
            if args.verbose:
                print(f"  ok    {name}")
            continue

        findings.append(Finding(name, [
            f"FAIL  {name} ({identity}): the adapters disagree",
            *(f"    {sdk:7} {text[:400]}" for sdk, text in rendered.items()),
        ]))

    findings.extend(must_differ_findings(agreed_by_name))

    print()
    # `FAIL  <what>` per finding and a closing `N passed, M failed`, which is
    # the house style every `scripts/test_*.py` prints and `scripts/mutate.py`'s
    # `python` dialect reads.
    #
    # This runner had a format of its own — a bare headline per finding and
    # `130 cases: the three SDKs agree on all of them` — and the cost showed up
    # the first time somebody pointed `mutate.py` at it: the script reported
    # "no test results at all" and refused to score the run, correctly, because
    # neither line matched anything it knew. `test_codegen.py` had the identical
    # problem and its entry settled how to fix it: the summary line rather than
    # another dialect. A runner nobody can mutation-test is a runner whose own
    # correctness is taken on trust, and this one decides whether three SDKs
    # agree.
    #
    # The human-facing line stays, because "130 cases: the three SDKs agree on
    # all of them" says something the counts do not.
    passed, failed = tally(len(CASES), findings)
    if findings:
        print(f"{len(CASES)} cases, disagreements:")
        print()
        for finding in findings:
            for line in finding.lines:
                print(line)
        print()
        print(f"{passed} passed, {failed} failed")
        return 1
    print(f"{len(CASES)} cases: the three SDKs agree on all of them")
    print()
    print(f"{passed} passed, {failed} failed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
