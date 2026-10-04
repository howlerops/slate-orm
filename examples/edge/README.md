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

## Deploy it to Cloudflare

Done once, on 2026-10-04, to `slate-explorer-edge.<account>.workers.dev`. It is
not part of any test. It needs a Cloudflare account and a head node the
Worker can reach from the internet. The steps that worked:

1. **Seed a persisted store, then serve it with token authentication.** The
   explorer seeds through its `trusted-header` identities, and a node
   reachable from the internet must never run in that mode, because it trusts
   whatever identity a caller claims. So seed first with
   `backend = "local"` on loopback, stop the node, and restart it on the same
   directory with `[auth] mode = "token"`. Give it one `[[auth.tokens]]` per
   persona, with the same principal, tenant and roles as the explorer's
   identities, so policies and column grants behave identically. Read each
   secret from a `secret_file` with mode `600`.

2. **Expose it.** A quick tunnel needs no Cloudflare configuration:

   ```sh
   cloudflared tunnel --no-autoupdate --url http://127.0.0.1:<port>
   ```

   A request through it without a token must get `grpc-status: 16`. Check that
   before going on.

3. **Deploy, then store the tokens as a secret.** The Worker reads
   `SLATE_TOKENS`, a JSON object from persona to token, and sends the
   persona's token as `authorization: Bearer` through the `fetch` option
   `Client.connectWeb` takes:

   ```sh
   npx wrangler deploy --var HEAD:https://<tunnel>.trycloudflare.com
   npx wrangler secret put SLATE_TOKENS < tokens.json
   ```

4. **Compare it.** The conformance runner can point its edge adapter at the
   deployed URL while the other three run locally (`./run.sh --headless`,
   then `conformance.py --edge https://…workers.dev`). Two things are worth
   knowing:

   - Cloudflare's browser-integrity check refuses Python's default
     `urllib` user agent with error 1010 before the Worker runs, so the
     runner needs another user agent.
   - The deployed Worker reads its own database, so a case whose answer
     includes a timestamp written at seeding time differs by however far
     apart the two seeds ran.

   On 2026-10-04 that run agreed on 142 of 143 cases, and the 143rd differed
   only in exactly such a timestamp.

A quick tunnel's URL lasts as long as the `cloudflared` process, and the
deployment as long as the head node behind it. `npx wrangler delete`
removes the Worker.

## What this does not do

- It does not run in a browser. A page that held a head node's identity would
  be a credential in the open; `docs/edge-client.md` says what that would
  need.
- It authenticates only with the tokens it is given, and the demo's
  persona switch (`x-demo-identity`) chooses among them, so anyone who can
  reach the Worker can act as any persona. That is the explorer's point, and
  it is not an access-control design.
