# The TypeScript client on an edge runtime, and why that means gRPC-web

The demo's WASM build runs the *kernel* in a browser: the planner, the
executor, an in-memory store. It is a good test of the kernel and no test at
all of the thing an application on Cloudflare Workers, Deno Deploy or a
browser would actually do, which is talk to a head node over the network
through the published client. This note is about that, and it settles four
things.

## What it does today, measured

`clients/typescript` imports `@grpc/grpc-js`, `@grpc/proto-loader`,
`node:crypto`, `node:path`, `node:fs` and `node:url`. Two of those cannot be
had on an edge runtime at any price:

- **`@grpc/grpc-js` speaks HTTP/2 through `node:http2`.** Workers implement a
  subset of Node's APIs under `nodejs_compat`, and outbound HTTP/2 is not in
  it. Outbound traffic is `fetch`, and `fetch` exposes no response
  *trailers*, which is where native gRPC puts the status of every call.
- **`@grpc/proto-loader` reads `.proto` files from disk at runtime.** There is
  no disk.

So the client does not run there, and no amount of bundling changes that. The
transport and the schema loading have to move.

## 1. The protocol is gRPC-web, served by the head node itself

gRPC-web is gRPC with the trailers moved into the response body, framed so an
HTTP/1.1 `fetch` can carry it. `tonic-web` is a layer, maintained alongside
`tonic`, that translates it to native gRPC before the request reaches the
service.

- **Not a REST gateway.** A second protocol with its own mapping of every
  message is a second thing to keep in step, and the conformance runner exists
  because three clients of *one* protocol already disagreed. The edge client
  speaks the same messages as the Node one, and the server answers both from
  the same handlers.
- **Not Connect.** Connect also carries over `fetch`, and its Rust server is a
  third-party crate. gRPC-web is served by the project that already serves
  native gRPC here, so it has no separate release calendar.
- **Opt-in, in `[listen]`.** `grpc_web = true` makes the node accept HTTP/1.1
  and gRPC-web on its one port. A node configured before this exists keeps
  refusing HTTP/1.1, because accepting a new protocol is a change to what
  is exposed and should not happen to a deployment that did not ask.
- **Outermost.** The web layer sits outside the admission limit, the stream
  limit and the observer, so every layer that reads a status reads a native
  one. Put inside them, `ObserveLayer` would look for `grpc-status` in
  trailers that gRPC-web has moved into the body, and count every gRPC-web
  failure as a success.

No CORS. A Worker calls the node server to server. A *browser* calling a head
node directly would need CORS headers and a decision about which origins may
present which identity, and that is a different feature with a security
question of its own.

## 2. The client gains a transport seam, and Node keeps the one it has

Every call in `client.ts` goes through two private methods: one unary, one
server-streaming. They take a method name, a request object and the call's
metadata, and they hand back response objects. That is the seam. A
`Transport` with those two operations has two implementations:

- **`grpc-js`**, which is what `Client.connect` uses today, unchanged. A Node
  application sees no difference.
- **`grpc-web`**, which frames each message, posts it with `fetch`, and reads
  the status out of the trailing frame. `Client.connectWeb(url, identity, {
  fetch })` builds one, and a caller can pass their own `fetch` so a Worker's
  service binding or a test double works the same way.

Both produce the **same objects**, or the clients are not one client.
proto-loader converts with protobufjs under a fixed set of options (longs as
strings, enums as strings, defaults, oneofs), so the web transport converts
with protobufjs under the same options, imported from the one place both
read them. A test asserts the two transports decode the same bytes to deep-equal
objects.

## 3. The codecs are generated as static code at build time, and checked

~~protobufjs can build its types from a JSON descriptor, which a bundler
inlines and an edge runtime can load without a filesystem.~~ **That was the
plan, and `workerd` refused it.** The descriptor loaded without trouble, but
building a type from one makes protobufjs *compile* that type's encoder and
decoder at runtime with `new Function`. Node allows that and Workers do not.
The whole TypeScript suite, 211 tests, passed over gRPC-web in Node, and the
first conformance run with the Worker failed every one of its cases with "Code
generation from strings disallowed for this context". Nothing short of the
real runtime would have shown it, which is the argument for §4.

So the codecs are generated ahead of time. `pbjs -t static-module` writes the
same functions protobufjs would have compiled as ordinary source, and
`scripts/generate-codecs.mjs` produces three files, all committed:

- `src/generated/records.ts`, camelCase as proto-loader loads it;
- `src/generated/status.ts`, the two `google/rpc` files with case kept, as
  `details.ts` reads them;
- `src/descriptor.ts`, a table of each RPC's two types and whether it streams.

`protobufjs-cli` is pinned exactly, because it generates committed code. A
freshness test regenerates all three and compares them byte for byte, the
same arrangement the Go and Python stubs have.

One further hypothesis did not survive. The first draft installed the `long`
package by hand, on the theory that protobufjs found it through an `eval`
that Workers would also refuse, leaving every 64-bit integer decoded as a
rounded JavaScript number. A mutation removing the install survived the
four-adapter run. Reading `util/minimal.js` showed that protobufjs 7.6 uses a
plain `require("long")`, which the bundler resolves, so the install was
removed.

The Node-only imports go behind the transport. `node:crypto` provided
`randomUUID` for request ids, and `crypto.randomUUID()` is a global on every
target here. `node:path`, `node:fs` and `node:url` located the `.proto` files
for proto-loader, which only the `grpc-js` transport still uses.

## 4. The test is a fourth adapter in the conformance runner, on `workerd`

`examples/edge` is a Worker that implements the explorer's HTTP contract
(`examples/explorer/CONTRACT.md`) with the TypeScript client over gRPC-web.
It runs under `wrangler dev`, which is `workerd`, the open-source runtime
Cloudflare deploys. It joins the conformance runner beside the Python, Go and
Node adapters, so the edge client is held to the same cases, byte for byte,
instead of to tests written for it.

That is a stronger claim than "it runs on Cloudflare". A Worker that ran and
rendered a `u64` through a JavaScript number would pass a hand-written test
that checked small ids, and fail the conformance case that checks
`9007199254740993`.

**Deploying to Cloudflare is not part of the test.** It needs an account, and
so does not belong in CI. It has been done, entirely on Cloudflare:
- two head nodes run as Containers from the repository's image, one leading
  and one following, each with two read replicas;
- their database is in R2, through the S3 API;
- the Worker reaches them through a Durable Object binding, and serves the
  explorer's web UI beside its API.

The conformance runner against the deployed URL agreed on 142 of 143 cases,
the 143rd differing only in a seeding timestamp. `examples/edge/README.md`
and `deploy.sh` have the steps.

## What this note does not do

**It does not make the browser a client.** The same transport would work from
a page, but a page holding a head node's identity headers is a credential in
the open. That needs a deliberate design: tokens scoped to a browser session,
CORS, and which origins may present them.

**No conformance case decodes a 64-bit integer above 2^53 from the server.**
The TypeScript suite checks one on Node over both transports. On `workerd`,
the four-adapter run would see one only if a seeded row held one, and adding
one would shift the answers of other cases.

**The web layer's position is argued, not tested.** §1 says it must sit
outside the observer so a gRPC-web failure is counted as a failure. Nothing
checks `/metrics` after a failed gRPC-web call.

**It does not cover client-streaming or bidirectional calls.** gRPC-web
cannot carry them, and the protocol here has none. Every RPC in
`records.proto` is unary or server-streaming, which is checked when the
transport is built rather than assumed.

**The Python and Go clients are not changed.** Neither has an edge runtime
worth the name to run on.
