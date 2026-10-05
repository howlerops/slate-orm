# The live explorer shows the whole ORM: its UI on the Worker, every endpoint in a panel, arrays, a unique index, and two head nodes with replicas

- **Date:** 2026-10-05
- **Author:** Claude Code (session: "does it show off ALL that our ORM app can do?" … "lets do all 4")
- **Touches:** `examples/explorer` (`CONTRACT.md`, `head.toml`, the three
  adapters, `conformance/conformance.py`, `web/src/api.ts`, `web/src/index.tsx`, `web/src/panels.tsx`,
  `web/test/api.test.ts`, `README.md`), `examples/edge` (`src/worker.ts`,
  `wrangler.cloudflare.jsonc`, `deploy.sh`, `container/derive.py`,
  `README.md`), `scripts/check_demo_surface.py`, `site/index.html`,
  `site/workbench.js`, `docs/edge-client.md`
- **Kind:** feature

## What changed

The owner asked whether the deployed explorer showed everything the ORM can
do. It did not. This change does four things.

1. **The Worker serves the explorer's web UI** at `/`, built with
   `VITE_HOSTED=edge`: one adapter, the page's own origin, and a node switch
   instead of the SDK switch. The landing page links to it.
2. **Every read endpoint has a panel.** Chains, windows, nearest neighbours
   (with distances computed in the page so the order can be checked), keyset
   pages (with the cursor), and the decimal renderer join the eleven that
   existed. The UI also gains the `analyst` persona, which the adapters have
   had since column grants.
3. **Three features reach the contract:**
   - `posts`, the table with array columns, is in all three adapters'
     allowlists. It was seeded and offered by the UI, and every adapter
     refused it.
   - A unique index `by_name` on `authors.name`, and `/api/unique`, which
     inserts a duplicate and a fresh name.
   - `/api/served-by`, which names the view that answered a read.

   Each has conformance cases, so 143 became 147.
4. **Two head nodes and four replicas on Cloudflare:**
   - Containers `a` and `b` run the same image over one R2 bucket. The first
     to start takes the writer lease, and the other follows.
   - Each node also runs two in-process read replicas.
   - The Worker starts `a` before it will forward to `b`, so `a` leads when
     both start cold.
   - A topology panel shows each node's role and a tally of which replica
     served eight reads, and lets you try a write on each node.

CTEs were the fourth gap named and are **not** in the live app. A CTE is a
`slate-sql` construct, and the head node's wire protocol has no SQL call, so
showing one live would mean adding SQL to the protocol. The workbench on
GitHub Pages runs SQL in the browser, so it gets a CTE example instead.
`crates/slate-wasm/tests/examples.rs` executes it with every other example.

## Why

The live explorer was the API alone, so the bare URL answered `not-found`,
and the owner asked how to use it. Behind that, the demo had been a
deliberate subset:
- `ledger/2026-09-21-the-demo-ui-is-a-subset-on-purpose.md` argued that the
  UI exists to show the two switches, and coverage is the conformance
  runner's job;
- `check_demo_surface.py` held a reason for each endpoint left out.

That argument fits a local demo whose audience can also run the conformance
runner. It does not fit a public deployment whose purpose, as the owner
framed it, is to show what the ORM does. Five of the exemptions' reasons were
also answerable rather than fundamental. For example, "an order nobody can
check by eye" is answered by showing the distances.

## Alternatives rejected

- **Add SQL to the wire protocol so a CTE can be shown live.** That is a
  product decision about the protocol's shape, a second query language for
  three clients, and a security surface for `docs/security-review.md`. It is
  not a demo task. The workbench already parses and plans SQL in the browser,
  including the CTE expansion `docs/ctes.md` describes.
- **The live UI calling the Worker from GitHub Pages, cross-origin.** It
  needs CORS for the Pages origin, and it puts the workbench's in-browser
  kernel and the remote node on one page, where a reader could not tell
  which answered. Same-origin static assets need no CORS, and a link keeps
  the two apart.
- **Make the node switch automatic, sending writes to whichever node leads.**
  The follower's refusal (`not-leader`, naming the leader) is what the panel
  exists to show, and a Worker that hid it would demonstrate less. Choosing
  the leader by startup order is a heuristic. The Worker comment says where
  it stops: if `a` dies while `b` runs, `b` stays a follower, because
  promotion is a restart (`docs/topology.md`).
- **A second container per replica.** `[[replicas]]` are SlateDB readers of
  the same bucket inside the node's process. That is the shape
  `examples/deployed` runs and the one `topology.md` describes.
  Process-per-replica would need a serverd mode that does not exist. Two
  *nodes* already show the cross-process case: a separate process, with no
  writer, reading R2.
- **Leave the default replica poll.** `catch_up` defaults to 250 ms, and the
  poll derives to 50 ms. On R2 every poll is a billed read: two nodes with two
  replicas each would make about 80 a second while awake. `derive.py` sets
  `catch_up = "5s"`, which polls once a second per replica.
- **Unique index id 1.** I wrote it first, and the server refused at load:
  `by_title_text` holds 1, and index ids are global, because an index entry's
  key carries the index id and not the table's. The comment in `head.toml`
  says so.
- **`servedBy` on every `/api/query` answer**, as `CONTRACT.md` claimed it
  already was (no adapter ever sent it). It varies between two runs of one
  read on a node with replicas, so every read case would stop being
  comparable. It has its own endpoint instead, and the stale example is
  corrected.

## Evidence

**Four adapters, one head node:** `./run.sh --conformance` gave "147 cases:
the 4 adapters agree on all of them". Every mutation run below also started
from a green baseline of the same run.

**Two nodes against the real R2 bucket, from this machine,** in
trusted-header mode on loopback, with the container's storage, replicas and
routing:
- the leader built `by_name` over 5 entries in 668 ms;
- the banners say `leader` and `follower (read-only: no writer store, writes
  are redirected)`;
- six reads on each node alternated `reader-1`, `reader-2`;
- the unique index refused the duplicate on the leader, and the follower
  refused both inserts with `not-leader` / `NOT_LEADER`, naming the leader;
- `posts` came back with tagged array elements.

**Live, after deploy** (Worker version `cf3c66c3`, then `fc4023e1` for the
fix below):
- `/` returns the page;
- both nodes' meta, served-by and write answers matched the local run;
- a browser walked the rows, topology and nearest tabs, with no console
  errors.

The nearest tab's distance column was **empty** on the first deploy. The page
read the title and embedding at ordinals 0 and 1 of a projected row, but a
projected row keeps every column's ordinal, so they are at 2 and 6. Fixed and
redeployed. Solaris's distance, 0.0871, agrees with working it by hand.

**Mutations, all recorded under `ledger/mutations/`:**
- **The UI** (`ledger/mutations/20261005T215008-examples-explorer-web-src-api-ts.json`, `ledger/mutations/20261005T215024-examples-explorer-web-src-api-ts.json`). All four caught
  by named tests in `test/api.test.ts`:
  - the hosted base kept at the default port;
  - hosted mode never selected;
  - the node header sent unasked;
  - served-by ignoring the panel's node.
- **The adapters, scored by the full four-adapter conformance run:**
  - Node (`ledger/mutations/20261005T215045-examples-explorer-backends-node-src-adapter-ts.json`), caught: the refusal's reason dropped, and `posts`
    unreadable.
  - Go (`ledger/mutations/20261005T215125-examples-explorer-backends-go-handlers-go.json`), caught: collide sending the fresh name.
  - Python (`ledger/mutations/20261005T215154-examples-explorer-backends-python-adapter-main-py.json`), caught: the reason replaced by the kind, and
    served-by counting nothing.
- **Three results from that first adapter run needed a decision:**
  - *Node: served-by names a constant.* I recorded it as an expected
    survivor, reasoning that every local read is the writer's and so says
    `writer`. **It was caught, and the reasoning was wrong.** The in-memory
    writer calls itself `memory`, not `writer`. The contract had said
    `writer` in two places, and both are corrected. Re-run in `ledger/mutations/20261005T215357-examples-explorer-backends-node-src-adapter-ts.json`,
    caught.
  - *Go: the landed row never removed* **survived.** Each adapter clears key
    9400 before inserting, so a row one adapter leaves behind is gone before
    the next looks, and nothing compared could see it. `/api/unique` now
    reports `left`, read back after the clean-up, in all three adapters.
    Re-run in `ledger/mutations/20261005T215333-examples-explorer-backends-go-handlers-go.json`, caught.
  - *Node: `landed` taken from the insert's answer instead of the table*
    survives, and is recorded as expected (`ledger/mutations/20261005T215357-examples-explorer-backends-node-src-adapter-ts.json`). Against a correct
    server the two agree. The read-back defends against a server that
    refuses and writes anyway, which no case can stage.

**Static checks:** `scripts/check.sh`, with `/tmp/ci-env` on `PATH`, passes 98
of 99. The one failure is `example-runner-guard`, which needs GNU `timeout`,
absent on this Mac; it failed the same way before this change.
`check_demo_surface.py` reports 22 of 28 endpoints in the UI. `codegen.py
--check` passes for all four generated declarations, because the index does
not enter them. `site/check/docs.py` passes. The web suite is 20 of 20.

## What this does not do

- **The Worker's start-`a`-first rule is untested.** No local runtime has
  Containers. That `a` led after this deploy is one observation, not a test
  of the rule.
- **The live database carries rows from earlier conformance runs.** The
  write endpoints seed ids 9100–9301 and leave some behind, which is why the
  page shows 16 books rather than 11, and why four "Predicate" rows top the
  nearest list at distance 0.
- **CTEs are not in the live app.** They are in the workbench, which runs SQL
  in the browser rather than against a head node.
- **The browser e2e (`./run.sh --e2e`) was not run locally.** CI runs it. It
  drives the local three-SDK build, whose switches are unchanged.
- **Two nodes double the containers awake,** while someone uses the page.
  Each sleeps after ten idle minutes.
