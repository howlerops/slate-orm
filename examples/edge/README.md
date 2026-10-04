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

This is not part of any test. It needs a Cloudflare account, and a head node
the Worker can reach from the internet.

1. **Expose a head node.** It must run with `listen.grpc_web = true`. It also
   needs real authentication, not the explorer's `trusted-header` mode, which
   trusts whatever identity a caller claims. Use `[auth] mode = "token"` and
   give the Worker a token. A Cloudflare Tunnel in front of a node on your own
   machine is the quickest way to get a public address:

   ```sh
   cloudflared tunnel --url http://127.0.0.1:7421
   ```

2. **Point the Worker at it** and deploy:

   ```sh
   npx wrangler login
   npx wrangler deploy --var HEAD:https://<your-tunnel>.trycloudflare.com
   ```

   The adapter sends the demo's identity headers, which a `token`-mode node
   ignores. A Worker that is meant to be used would hold a token in a Wrangler
   secret (`wrangler secret put SLATE_TOKEN`) and send it as
   `authorization: Bearer`. That is a change to `src/worker.ts` this example
   does not make.

## What this does not do

- It does not run in a browser. A page that held a head node's identity would
  be a credential in the open; `docs/edge-client.md` says what that would
  need.
- It does not authenticate. See step 2.
