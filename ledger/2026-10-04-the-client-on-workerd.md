# The TypeScript client on Cloudflare Workers, held to the conformance cases on `workerd`

- **Date:** 2026-10-04
- **Author:** Claude Code (session: "build a client app that runs in cloudflare … a better real-world test than the WASM one")
- **Touches:** `crates/slate-serverd` (`listen.grpc_web`, `tests/grpc_web.rs`),
  `clients/typescript` (a transport seam, `web.ts`, `node.ts`, `transport.ts`,
  generated codecs, the `./edge` entry, the harness, two new test files),
  `examples/explorer/backends/node` (split into `adapter.ts` and a Node shell),
  `examples/edge` (new), `examples/explorer/run.sh`, `head.toml`,
  `conformance.py`, `.github/workflows/ci.yml`, `docs/edge-client.md`,
  `.gitignore`, `scripts/check_generated_is_used.py`, `scripts/mutations.json`,
  `scripts/test_check_sh.py`
- **Kind:** feature

## What changed

The owner asked for a client app on Cloudflare as a better real-world test
than the WASM build, which runs the kernel in a browser and so never tests a
client talking to a head node. `docs/edge-client.md` is the design. Built:

1. **The head node speaks gRPC-web**, opt-in, with `listen.grpc_web = true`.
   This is `tonic-web`, outermost in the layer stack, with HTTP/1.1 accepted
   only when the setting is on.
2. **The TypeScript client has a transport seam.**
   - `Client.connect` uses native gRPC, unchanged.
   - `Client.connectWeb(url, identity, { fetch })` speaks gRPC-web over `fetch`.
   - A new `@slate-orm/client/edge` entry exports the same API and never loads
     grpc-js, which would import `node:http2`.
3. **The codecs are generated as static code at build time**, by a pinned
   `protobufjs-cli`, and checked for freshness.
4. **`examples/edge` is a Worker** implementing the explorer's contract with
   the Node adapter's own endpoint code, moved into a transport-free
   `adapter.ts`. It joins the conformance runner as a fourth adapter, under
   `wrangler dev`, which is `workerd`.
5. **The TypeScript suite runs twice in CI**, once per transport.

## Why

The WASM playground tests the kernel. An application on Workers, Deno or in a
browser would talk to a head node over the network through this client, and
until now nothing tested that. The client could not have done it anyway:
`@grpc/grpc-js` needs `node:http2` and `@grpc/proto-loader` needs a disk.

**Running the real runtime found a defect no amount of Node testing could.**
After the transport seam, the whole suite passed over gRPC-web in Node, 211 of
211. The first four-adapter conformance run failed *every* case on the Worker
with "Code generation from strings disallowed for this context". protobufjs
compiles each message's encoder at runtime with `new Function`. Node allows
that and `workerd` refuses it. The static codecs are the fix. That failure is
the argument for the test being the conformance runner on `workerd`, not a
handful of requests to a Worker in Node.

## Alternatives rejected

- **A REST gateway in front of the head node.** It is a second protocol, with
  its own mapping of every message to keep in step, and the conformance runner
  exists because three clients of *one* protocol already disagreed.
- **Connect instead of gRPC-web.** Connect also carries over `fetch`, but its
  Rust server is a third-party crate. gRPC-web is served by `tonic-web`,
  released alongside `tonic`.
- **gRPC-web on by default.** Accepting HTTP/1.1 on the gRPC port is a change
  to what a node exposes, and should not happen to a deployment that did not
  ask for it.
- **A JSON descriptor loaded at runtime.** This was the first design, and
  `workerd` refused it: building types from a descriptor still compiles
  encoders with `new Function`.
- **A hand-written protobuf runtime** interpreting the descriptor. It would
  avoid `new Function` and be a second implementation of protobuf to keep
  agreeing with proto-loader. Generated code is protobufjs's own, written out.
- **A new Worker written for the test.** It would test whatever its author
  thought of. Reusing `adapter.ts` means the Worker answers the same 143 cases
  as three independent clients. The difference under test is runtime,
  transport and entry point, which is exactly what is new.
- **Skipping the interceptor tests in web mode.** "A skip is green". The
  harness's interceptor argument became a transport-neutral observer instead
  (a grpc-js interceptor natively, a `fetch` wrapper on the web), so both
  tests run over both transports.
- **Installing `long` by hand**, on the theory that protobufjs found it through
  an `eval` Workers refuse. A mutation removing it survived the four-adapter
  run (`ledger/mutations/20261004T174224-clients-typescript-src-transport-ts.json`).
  Reading `util/minimal.js` showed protobufjs 7.6 uses a plain
  `require("long")`, which the bundler resolves. The install was removed and
  the hypothesis withdrawn in the design note.

## Evidence

**The suite over both transports**, after the change. Each run is 219 tests,
the original 211 plus 8 new:

```
native:  ℹ tests 219  ℹ pass 219  ℹ fail 0
web:     ℹ tests 219  ℹ pass 219  ℹ fail 0
```

**Four adapters on one head node.** Before the static codecs, the edge adapter
failed every case with the `new Function` refusal. After them, `./run.sh
--conformance` gave "143 cases: the 4 adapters agree on all of them".

**gRPC-web on the wire.** `crates/slate-serverd/tests/grpc_web.rs` sends
hand-written HTTP/1.1 over a raw socket. It pinned a finding the TypeScript
transport is written to: a refusal before any message is *trailers-only*, and
gRPC-web sends its status as an HTTP header with an empty body, not in a
trailer frame. A client that read only the trailer frame would lose every
refusal's status. Mutations,
`ledger/mutations/20261004T174541-crates-slate-serverd-src-serve-rs.json`:
- HTTP/1.1 refused with the setting on is caught;
- HTTP/1.1 accepted whatever the setting is caught;
- the web layer never installed is caught.

**The client transport.** Mutations against the full suite in web mode:
- the header status ignored, the trailer status ignored, `grpc-message` not
  decoded, and a partial frame yielded early:
  `ledger/mutations/20261004T174752-clients-typescript-src-web-ts.json`;
- a body with no status read as success:
  `ledger/mutations/20261004T174918-clients-typescript-src-web-ts.json`;
- longs as numbers, the web decoder dropping defaults, and the web decoder
  rendering enums as numbers:
  `ledger/mutations/20261004T174959-clients-typescript-src-transport-ts.json`;
- the method table drifting from `proto/`:
  `ledger/mutations/20261004T175114-clients-typescript-src-descriptor-ts.json`;
- the server not told to speak gRPC-web:
  `ledger/mutations/20261004T172619-clients-typescript-test-harness-ts.json`.

Every mutation in those records was caught. One survivor arrived on the way and
was answered rather than excused. With a failure in the *trailer frame* read
as success, the whole suite survived
(`ledger/mutations/20261004T172431-clients-typescript-src-web-ts.json`),
because this server sends every refusal trailers-only. `test/web.test.ts`
scripts `fetch` to cover that shape: a message followed by a failing trailer,
frames split at every byte boundary from 1 to 7, a header-only status, a body
with no status, and a node that does not speak gRPC-web.

Two runs are kept for the record but not cited as evidence. One `transport.ts`
run reported `baseline-red`
(`ledger/mutations/20261004T172543-clients-typescript-src-transport-ts.json`),
and three reruns of the same baseline were green; the cause was not found. One
`web.ts` run ended `interrupted`
(`ledger/mutations/20261004T172859-clients-typescript-src-web-ts.json`)
because a shell heredoc mangled an anchor; `mutate.py` refused it, and it was
re-run. Earlier runs against the JSON-descriptor design
(`ledger/mutations/20261004T173035-clients-typescript-src-transport-ts.json`,
`ledger/mutations/20261004T173131-clients-typescript-src-web-ts.json`,
`ledger/mutations/20261004T173303-clients-typescript-src-transport-ts.json`,
`ledger/mutations/20261004T173310-clients-typescript-src-descriptor-ts.json`)
describe code since replaced, and were re-run against the final code above.

**Both transports decode alike.** `test/descriptor.test.ts` decodes a
`QueryResponse` carrying every arm of `Value`, 64-bit values past a double's
precision, a nested message and an enum, through proto-loader and through the
static codec, and requires deep-equal objects.

**The refactored Node adapter** still agrees with Python and Go on all 143
cases. That was checked before the Worker joined.

**Three guards had to follow the change, and each would otherwise have gone
stale.**
- `check_generated_is_used.py` pins that the Node adapter attaches its
  generated schema with `.declaring(`. That call moved to `adapter.ts` in the
  split, and the guard went red on the old path. Leaving it there would have
  stopped it guarding anything.
- `npm install` reformatted `clients/typescript/package.json`, which moved
  the anchor of `check_npm_package`'s roster mutation. That is the first of
  the lies `mutate.py`'s docstring lists, and `check_mutations_roster.py`
  refused it.
- `check_build_output.py` required the new package root to ignore its
  `dist/`.
- `scripts/test_read_deliberate.py` records how the ledger signal ranks a
  known closing entry. This entry says "playground" and joined that list in
  first place, moving the recorded rank from 16 of 19 to 17 of 20. The test
  asks for a worse rank to be written down rather than tuned away, so `RANKS`
  carries the new figure and a note saying why.

## What this does not do

- **It is not deployed to Cloudflare.** It runs under `workerd` locally and in
  CI. Deploying needs an account and a head node reachable from the internet;
  `examples/edge/README.md` gives the steps, untested.
- **No conformance case decodes a 64-bit integer above 2^53 from the server**
  on `workerd`. The TypeScript suite checks one on Node over both transports.
- **The web layer's position is argued, not tested.** Nothing checks that
  `/metrics` counts a failed gRPC-web call as a failure.
- **The Worker does not authenticate.** It sends the demo's identity headers.
  A deployment would put a token in a Wrangler secret and add it in the
  `fetch` wrapper.
- **No browser client.** A page holding a head node's identity is a credential
  in the open, and that needs its own design.
