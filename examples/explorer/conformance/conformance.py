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
from typing import Any

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
    # diff rather than as a rounding nobody notices.
    ("all types", "/api/query",
     {"table": "books", "filter": {"op": "eq", "column": 0, "value": {"u64": "10"}}}, "app"),

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
      )],

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

    ("a committed transaction", "/api/transaction", {"commit": True}, "app"),
    ("a rolled-back transaction", "/api/transaction", {"commit": False}, "app"),
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
    "no such foreign key",
    "a stranger may not load a relationship",
    "a page with no limit",
    "a page whose projection drops the key",
    "a page sorted into an order the key does not give",
    "a predicate update with no assignments",
    "a reader may not write by predicate",
}


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


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--verbose", action="store_true", help="print every case")
    for sdk, default in DEFAULTS.items():
        parser.add_argument(f"--{sdk}", default=default, metavar="URL",
                            help=f"the {sdk} adapter's base URL (default {default})")
    args = parser.parse_args()

    adapters = {sdk: getattr(args, sdk) for sdk in DEFAULTS}

    failures: list[str] = []
    for name, path, body, identity in CASES:
        answers = {
            sdk: normalise(call(base, path, body, identity)) for sdk, base in adapters.items()
        }

        down = [sdk for sdk, a in answers.items()
                if isinstance(a, dict) and "__transport__" in a]
        if down:
            failures.append(f"{name}: adapters unreachable: {', '.join(down)}")
            for sdk in down:
                failures.append(f"    {sdk}: {answers[sdk]['__transport__']}")
            continue

        rendered = {sdk: json.dumps(a, sort_keys=True) for sdk, a in answers.items()}
        if len(set(rendered.values())) == 1:
            # Agreement is necessary and not sufficient: see EXPECTED_REFUSALS.
            agreed = next(iter(answers.values()))
            refused = isinstance(agreed, dict) and "error" in agreed
            if refused and name not in EXPECTED_REFUSALS:
                failures.append(
                    f"{name} ({identity}): all three refused it, and this case "
                    f"is supposed to return an answer"
                )
                failures.append(f"    {json.dumps(agreed)[:400]}")
                continue
            if not refused and name in EXPECTED_REFUSALS:
                failures.append(
                    f"{name} ({identity}): listed as a refusal and all three "
                    f"answered it; the list is stale"
                )
                continue
            if args.verbose:
                print(f"  ok    {name}")
            continue

        failures.append(f"{name} ({identity}): the adapters disagree")
        for sdk, text in rendered.items():
            failures.append(f"    {sdk:7} {text[:400]}")

    print()
    if failures:
        print(f"{len(CASES)} cases, disagreements:")
        print()
        for line in failures:
            print(line)
        return 1
    print(f"{len(CASES)} cases: the three SDKs agree on all of them")
    return 0


if __name__ == "__main__":
    sys.exit(main())
