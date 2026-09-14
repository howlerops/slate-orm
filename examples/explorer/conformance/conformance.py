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
    ("a grouping that needs a computed column", "/api/aggregate",
     {"groupBy": "decade"}, "app"),

    ("a committed transaction", "/api/transaction", {"commit": True}, "app"),
    ("a rolled-back transaction", "/api/transaction", {"commit": False}, "app"),
]


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
