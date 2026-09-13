# A TypeScript client, and why its integers are bigint

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `clients/typescript/` (new), `README.md`
- **Kind:** feature

## What changed

A TypeScript package at `clients/typescript`: values, expressions, queries,
sessions, transactions, single-table `Explain`, a typed error taxonomy, and
twenty tests against a real `slate-serverd`. The protocol is loaded at runtime
by `@grpc/proto-loader`, so there is no codegen step and building this package
needs no `protoc`.

## Why

The last item of the README's not-built list, and the third client. Two clients
check that the protocol is a protocol; a third in a language with different
primitive types checks something else — JavaScript has one number type, and
this server has five.

## Alternatives rejected

**`number` for 64-bit integers.** The obvious JavaScript choice and the one
that silently corrupts primary keys: a `number` holds integers exactly only to
2^53, and a u64 key above that comes back as a different key. Values are
`bigint`. A test writes `2^53 + 1` as a key and reads it back, and decoding
through `Number` instead of `BigInt` fails it — which is how that decision is
enforced rather than merely documented.

**Plain JavaScript values instead of a tagged union.** Same argument as the Go
client, sharper here: `7` as an `i64` and `7` as a `u64` are different values
to this server, and JavaScript offers nothing to tell them apart.

**Ahead-of-time codegen with `protoc`.** Rejected because it puts a `protoc` in
the build of anybody who installs this package, for a client whose hot path is
not message construction. Runtime loading costs one file read at startup and
gives `unknown` at the boundary — which this client narrows by hand, and which
is why the value decoder throws on a kind it does not recognise rather than
returning something plausible.

**Shipping without the `.proto`.** Runtime loading means the file has to be in
the tarball, so the package carries its own copy. A copy can drift, and a
drifted protocol definition fails as a *wrong answer* rather than as a build
error — the worst way to fail. `test/proto.test.ts` compares the bundled copy
against `crates/slate-server/proto` byte for byte, and was itself checked by
editing the copy and watching it fail.

**A mock server.** Refused for the reason the Python suite gives.

## Evidence

Twenty tests passing against a real node; `tsc --strict` with
`noUncheckedIndexedAccess` and `exactOptionalPropertyTypes` clean. Four
mutations, all killed:

- Encoding a u64 as an i64 fails fifteen tests.
- Decoding a u64 through `Number` instead of `BigInt` fails the large-key test
  and nothing else, which is exactly the test written for it.
- Ignoring a descending sort fails the ordering test.
- Setting the session watermark once rather than advancing it fails the
  watermark test, which asserts a *second* write moves it further.

One test was wrong and the server was right: `a null value is not a zero`
tried to write a null into `docs.kind`, which is not nullable, and the server
refused. The fixture gained a nullable column, and the refusal became its own
test — a null in a non-nullable column must surface as an ordinary
`invalid-request`, not be swallowed.

Two harness bugs worth recording because they are the same bug twice: both the
proto path and the repository root were written as a fixed number of `..`
segments, which is correct from `src/` and wrong from the compiled
`dist/src/`. Both now search upward for a landmark. A relative depth that is
right in one layout and silently wrong in another is not a path, it is a
coincidence.

## What this does not do

`Join`, `Aggregate` and `ExplainJoin` have no typed surface, matching the Go
client. No vector similarity, no `SchemaCheck`, no computed values.

Nothing compares the three clients against *each other*. All three are tested
against the same server, which catches a client that is wrong about the
protocol and not three clients wrong in the same way.

The package is not published anywhere and its version is `0.0.1`; `npm test`
needs `cargo` on the path, so it is not runnable from a JavaScript-only
checkout.
