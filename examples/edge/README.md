# The explorer on Cloudflare Workers

The explorer's HTTP contract (`../explorer/CONTRACT.md`) as a Cloudflare
Worker. It uses the TypeScript client over **gRPC-web**, the only way a Worker
can reach a head node, because its only network primitive is `fetch`.

The endpoints are the Node adapter's own `adapter.ts`, unchanged, so the
conformance runner holds this Worker to the same cases as the Python, Go and
Node adapters, byte for byte. What differs is everything underneath:

- the runtime is `workerd`, not Node;
- the transport is gRPC-web over `fetch`, not HTTP/2;
- the client entry point is `@slate-orm/client/edge`, which loads nothing from
  Node.

`docs/edge-client.md` has the design.

## Run it locally

`wrangler dev` runs the Worker in `workerd`, the open-source runtime Cloudflare
deploys, on your machine:

```sh
# a head node that speaks gRPC-web (the explorer's head.toml turns it on)
cargo run -p slate-serverd -- --config ../explorer/head.toml

npm ci
npx wrangler dev --var HEAD:http://127.0.0.1:7421
curl -s localhost:8787/api/meta
```

Or let the explorer start everything and compare all four adapters:

```sh
cd ../explorer && ./run.sh --conformance
```

## What running it on `workerd` found

The whole TypeScript suite passed over gRPC-web in Node, and the Worker still
failed every request. protobufjs builds its encoders and decoders at runtime
with `new Function`, Node allows that, and Workers refuse it with "Code
generation from strings disallowed for this context". The client now ships
codecs generated at build time (`clients/typescript/scripts/generate-codecs.mjs`).
That failure is why this example exists as a conformance adapter and not as a
demo: nothing short of running the real runtime would have shown it.

## Deploy it to Cloudflare, head node included

`deploy.sh` puts the whole thing on Cloudflare:

- this Worker, which also serves the explorer's web UI at `/`;
- **two head nodes**, `a` and `b`, as Cloudflare Containers running
  `slate-serverd` from the repository's own image, each with two in-process
  read replicas;
- their database in one **R2** bucket, through the S3 API.

Nothing runs on your machine afterwards. It was done on 2026-10-05.

```sh
npx wrangler r2 bucket create slate-explorer
R2_ENV=~/.config/slate/r2.env TOKENS=tokens.json CLOUDFLARE_ACCOUNT_ID=<id> sh deploy.sh
```

**What it needs:**

- **Docker.** It builds the image for `linux/amd64`, which is what Containers
  run, on any host. The root `Dockerfile` cross-compiles from arm64 for this.
- **An R2 API token scoped to the bucket.** Wrangler's login cannot create
  one: the API answers error 9109 to its OAuth session. Create the token in
  the dashboard under *R2 → Manage R2 API Tokens*, with Object Read & Write on
  the bucket, and put its two values in a file as `ACCESS_KEY_ID=` and
  `SECRET_ACCESS_KEY=`, mode `600`.
- **A JSON file of bearer tokens,** one per persona: `{"app": "…", …}`.

**What it does:**

1. Derives `container/head.toml` from `../explorer/head.toml`
   (`container/derive.py`). The tables, grants, policies and views are the
   ones the conformance runner exercises. It changes three sections:
   - `[listen]` binds every interface;
   - `[auth]` is `token`, with `transport = "tls-terminated-upstream"`, because
     the Worker terminates TLS and reaches the container over Cloudflare's
     internal network;
   - `[storage]` is S3 with `from_env = true`, so the image holds nothing
     account-specific.
2. Builds the image and sets three Worker secrets from the files, on stdin,
   never echoed: `R2_ACCESS_KEY_ID`, `R2_SECRET_ACCESS_KEY` and
   `SLATE_TOKENS`. The Worker passes them into the container as environment
   variables.
3. Builds the explorer's UI with `VITE_HOSTED=edge`, which drops the SDK
   switch (there is one adapter here) and adds a node switch, and sends every
   call to the page's own origin.
4. Deploys with `wrangler.cloudflare.jsonc`: the UI as static assets, with
   only `/api/*` running the Worker, and up to two container instances.
   `wrangler.toml` remains the Worker-alone shape that `wrangler dev` and the
   conformance runner use.

**Two nodes, one writer.** Each node campaigns for a writer lease in the
bucket when it starts; the first wins and writes, and the other comes up as
a read-only follower. A follower serves reads from its replicas and refuses
writes, naming the leader. The Worker starts `a` before it ever forwards to
`b`, so `a` leads whenever both start cold. A request picks its node with
`x-demo-node: a | b` (default `a`), and the answer names it in `x-slate-node`.

**Replicas, and what they cost on R2.** Each node also runs two read replicas,
`reader-1` and `reader-2`: separate readers of the same bucket, polling its
manifest. `/api/served-by` names which one answered. The default catch-up
budget would poll every 50 ms, which on R2 is a billed request each time, so
`container/derive.py` sets `[routing] catch_up = "5s"`, one poll a second per
replica while a node is awake.

**Seeding.** The node starts on whatever the bucket holds. The bucket was
seeded once by running `slate-serverd` locally against it in `trusted-header`
mode on loopback (`backend = "s3"` with R2's endpoint, region `auto`) and
running the explorer's seeder. That run also tested the parts most likely to
fail on R2:
- the node took leadership through a lease in the bucket;
- it resigned on `SIGTERM`;
- a restart took the lease at generation 2 and read back every seeded row.

**What was checked live on 2026-10-05, after the second deploy** (two nodes,
replicas, the UI):
- `/` serves the explorer's page, and its panels answer from the Worker; a
  browser walked the rows, topology and nearest tabs with no console errors;
- node `a` reports `leader: true` and `b` `leader: false`;
- on both, four reads in a row named `reader-1`, `reader-2`, `reader-1`,
  `reader-2`;
- on `a`, a duplicate author name is refused `already-exists` /
  `UNIQUE_VIOLATION`; on `b`, every write is refused `not-leader`, naming
  `a`'s lease holder.

The same two-node shape was run first from this machine against the same
bucket, which is also where the new unique index was built: 5 entries in
668 ms, by the node that took the lease.

**What was checked live after the first deploy,** with the tunnel and every
local process stopped:
- `/api/meta` reports the node leader on the first request;
- the 11 seeded books come back from R2, and the reader sees 9 under its
  policy;
- the analyst gets `born` as null and is refused a filter on it;
- the stranger is denied.

The conformance runner against the live URL, with the other three adapters
local, agreed on 142 of 143 cases. The 143rd differs in one cell: the
soft-delete timestamp of a row retired during seeding, which records when
each database was seeded. Cloudflare's browser-integrity check refuses
Python's default `urllib` user agent (error 1010), so the runner needs
another user agent.

### Or: a head node elsewhere, through a tunnel

The first deployment, on 2026-10-04, kept the head node on a workstation.
It ran in token mode behind a quick tunnel
(`cloudflared tunnel --url http://127.0.0.1:<port>`), with the Worker
deployed by `wrangler deploy --var HEAD:<tunnel URL>`. It lasts only as long
as the two local processes. `deploy.sh` replaced it under the same Worker
name.

`npx wrangler delete -c wrangler.cloudflare.jsonc` removes the Worker and its
container. The R2 bucket is left for `wrangler r2 bucket delete`.

## What this does not do

- It does not run in a browser. A page that held a head node's identity would
  be a credential in the open; `docs/edge-client.md` says what that would
  need.
- It authenticates only with the tokens it is given, and the demo's
  persona switch (`x-demo-identity`) chooses among them, so anyone who can
  reach the Worker can act as any persona. That is the explorer's point, and
  it is not an access-control design.
