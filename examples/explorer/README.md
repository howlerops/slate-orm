# The explorer

An interactive demo of one database seen through three SDKs, in the spirit of
[pgrust.com](https://pgrust.com/): a browser UI over a shared backend, with a
flag that swaps which client library serves the request.

```
./run.sh                 # head node + three adapters + the frontend
./run.sh --headless      # just the backend, for the conformance runner
```

## What is here

```
head.toml           one slate-serverd: schema, roles, and a row policy
backends/go         the Go client, behind HTTP
backends/node       the TypeScript client, behind the same HTTP
backends/python     the Python client, behind the same HTTP
web/                SolidJS + TanStack Query/Table, and a chart
conformance/        do the three answer identically?
CONTRACT.md         the HTTP contract all three implement
```

**One head node, three adapters.** Three SDKs against three databases would
demonstrate nothing; three against one is the demo. It is also what makes the
conformance runner meaningful — a disagreement between adapters cannot be
blamed on the data.

## The part worth reading

`conformance/conformance.py` sends 31 requests to all three adapters and
requires byte-identical JSON. **This is the first thing in the repository that
compares the clients to each other.** Each client's own suite runs against the
same server, which catches *a* client being wrong and cannot catch two quietly
disagreeing about something the server accepts from both.

It found real things on its first run, and they are the reason it exists:

- The three adapters sorted joined rows by three different keys, so the outer
  joins came back in three different orders. Go used `fmt.Sprintf("%v")`, which
  renders a map differently from `json.Marshal`.
- `Query.sort` in the Python client **replaces** the ordering rather than
  appending, so a loop of calls kept only the last key. The answer came back in
  primary-key order and looked exactly like the server ignoring the sort.
- One adapter refused an unknown table locally and another passed it to the
  server, so the same mistake produced two different errors.

None of those would have been caught by any client's own tests.

## What the UI shows

**The SDK switcher** is the headline: the same query, three client libraries,
one answer. If they ever differ, the demo is broken and the conformance runner
says so first.

**The identity switcher** is the more interesting one. `app`, `reader` and
`stranger` differ only in what the *database* grants them:

- `reader` sees 9 of 11 books — a row policy hides everything published before
  1960, and no adapter is involved in that.
- `reader` cannot `EXPLAIN`, because `EXPLAIN` is its own action rather than a
  weaker `read`: a plan is costed against statistics describing rows the policy
  hides.
- `stranger` is refused outright, and the UI shows the refusal rather than an
  empty table — "nothing matched" and "you may not ask" are different answers.

**Everything else** — filters, sorting, projections, the four join types, a
grouped join behind the chart, the plan, and a transaction whose write is
visible only to itself until it commits.

## Running the conformance check

```
./run.sh --headless          # one terminal
python3 conformance/conformance.py --verbose
```

## What this is not

Not a benchmark. The adapters do a JSON round trip the SDKs do not, the head
node is in-memory, and the fixture is eleven books. `docs/performance.md` has
numbers; this has none.

Not production shape. `trusted-header` auth means the adapter asserts an
identity and the head node believes it, which is safe only because nothing off
this machine can connect. A real deployment puts a proxy in front that sets
those headers and strips the client's.
