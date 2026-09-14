/**
 * The frontend's contract layer, tested without a browser.
 *
 * These cover the parts of `api.ts` that are decisions rather than plumbing:
 * where the adapter URLs come from, how a refusal is told apart from a result,
 * and how a tagged value renders. The panels themselves are covered by
 * `e2e/explorer.mjs`, which needs a running stack; everything here runs in
 * milliseconds with nothing started, which is the point of splitting them.
 */
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

import {
  adaptersFrom,
  api,
  DEFAULT_ADAPTERS,
  kindOf,
  render,
  TABLES,
  type Tagged,
} from "../src/api.js";

test("with nothing in the environment, the adapters are the demo's ports", () => {
  assert.deepEqual(adaptersFrom({}), DEFAULT_ADAPTERS);
});

test("each adapter URL can be named independently", () => {
  const chosen = adaptersFrom({ VITE_NODE_URL: "http://10.0.0.4:9002" });
  assert.equal(chosen.node, "http://10.0.0.4:9002");
  assert.equal(chosen.go, DEFAULT_ADAPTERS.go, "naming one must not disturb the others");
  assert.equal(chosen.python, DEFAULT_ADAPTERS.python);
});

test("a blank or whitespace URL falls back rather than resolving against the page", () => {
  // `--go ""` from a shell that expanded an unset variable is the realistic
  // way this arrives, and "" as a fetch base silently targets the UI's own
  // origin, which fails as a 404 on a route nobody wrote.
  for (const blank of ["", "   ", "\t"]) {
    assert.equal(
      adaptersFrom({ VITE_GO_URL: blank }).go,
      DEFAULT_ADAPTERS.go,
      `${JSON.stringify(blank)} should have fallen back`,
    );
  }
});

test("a URL is not trimmed away when it merely has spaces around it", () => {
  assert.equal(adaptersFrom({ VITE_GO_URL: "  http://h:1  " }).go, "http://h:1");
});

// --- rendering -------------------------------------------------------------

test("every tagged type renders, and the type stays distinguishable", () => {
  const cases: [Tagged, string, string][] = [
    [{ null: true }, "∅", "null"],
    [{ bool: true }, "true", "bool"],
    [{ str: "x" }, "x", "str"],
    [{ i64: "-9223372036854775808" }, "-9223372036854775808", "i64"],
    [{ u64: "18446744073709551615" }, "18446744073709551615", "u64"],
    [{ f64: "4.500000" }, "4.500000", "f64"],
    [{ bytes: "0a0b" }, "0x0a0b", "bytes"],
  ];
  for (const [value, text, kind] of cases) {
    assert.equal(render(value), text);
    assert.equal(kindOf(value), kind);
  }
});

test("64-bit integers pass through as text, never through Number", () => {
  // The reason they are strings on the wire at all. `18446744073709551615`
  // through JSON.parse becomes 18446744073709552000, and a primary key is
  // exactly where that lands.
  const key: Tagged = { u64: "18446744073709551615" };
  assert.equal(render(key), "18446744073709551615");
  assert.notEqual(render(key), String(Number("18446744073709551615")));
});

test("an absent value is an em-dash-worthy blank, not the string undefined", () => {
  assert.equal(render(undefined), "");
  assert.equal(kindOf(undefined), "");
});

// --- the call layer --------------------------------------------------------

function withFetch<T>(reply: unknown, run: (calls: Request[]) => Promise<T>): Promise<T> {
  const calls: Request[] = [];
  const real = globalThis.fetch;
  globalThis.fetch = (async (url: string, init: RequestInit) => {
    calls.push(new Request(url, init));
    return { json: async () => reply } as Response;
  }) as typeof fetch;
  return run(calls).finally(() => {
    globalThis.fetch = real;
  });
}

test("a body with an `error` key is a refusal, not a value", async () => {
  await withFetch({ error: { kind: "permission-denied", message: "no" } }, async () => {
    const answer = await api.query("go", "stranger", { table: "books" });
    assert.equal(answer.ok, false);
    assert.equal(answer.ok === false && answer.error.kind, "permission-denied");
  });
});

test("a body without one is a value, even when it is empty", async () => {
  await withFetch({ rows: [] }, async () => {
    const answer = await api.query("go", "app", { table: "books" });
    assert.equal(answer.ok, true);
    assert.deepEqual(answer.ok && answer.value.rows, []);
  });
});

test("the identity travels as a header, and is not mixed into the body", async () => {
  await withFetch({ rows: [] }, async (calls) => {
    await api.query("go", "reader", { table: "books" });
    const [request] = calls;
    assert.equal(request!.headers.get("X-Demo-Identity"), "reader");
    assert.deepEqual(JSON.parse(await request!.text()), { table: "books" });
  });
});

test("meta is a GET with no body; a query is a POST", async () => {
  await withFetch({ sdk: "go", leader: true, tables: [] }, async (calls) => {
    await api.meta("go", "app");
    assert.equal(calls[0]!.method, "GET");
  });
  await withFetch({ rows: [] }, async (calls) => {
    await api.query("go", "app", { table: "books" });
    assert.equal(calls[0]!.method, "POST");
  });
});

// --- the schema the UI holds -----------------------------------------------

/**
 * Walk up from this module until a directory holds `what`.
 *
 * Not `new URL("../../" + what, import.meta.url)`, because this file runs from
 * `dist-test/test/` and lives in `test/` — one segment apart — so a counted
 * path is right in the editor and wrong when run. That exact bug has now been
 * written four times across three packages in this repository; `clients/typescript`
 * has a shared `paths.ts` for it, and this package does not depend on that one.
 */
function findUp(what: string): string {
  let directory = path.dirname(fileURLToPath(import.meta.url));
  for (;;) {
    const candidate = path.join(directory, what);
    if (existsSync(candidate)) return candidate;
    const parent = path.dirname(directory);
    if (parent === directory) throw new Error(`no ${what} above ${import.meta.url}`);
    directory = parent;
  }
}

/** The `[[tables]]` blocks of head.toml, as name -> column names.
 *
 * Thirty lines of regex rather than a TOML dependency, and deliberately strict:
 * it throws if it finds no tables, so a parser that stops matching fails the
 * test instead of silently agreeing with an empty expectation. That failure
 * mode is the reason this exists at all — the assertion it replaced compared
 * `TABLES` against a second copy of the same literal, which no drift can break.
 */
function tablesInConfig(): Record<string, string[]> {
  const toml = readFileSync(findUp("head.toml"), "utf8");
  const found: Record<string, string[]> = {};
  for (const block of toml.split(/^\[\[tables\]\]$/m).slice(1)) {
    const name = /^name = "([^"]+)"/m.exec(block)?.[1];
    const columns = /^columns = \[([\s\S]*?)^\]$/m.exec(block)?.[1];
    if (!name || !columns) continue;
    found[name] = [...columns.matchAll(/\{\s*name = "([^"]+)"/g)].map((m) => m[1]!);
  }
  if (Object.keys(found).length === 0) {
    throw new Error("parsed no tables out of head.toml; the parser, not the config, is wrong");
  }
  return found;
}

test("the UI's column names match head.toml, in order", () => {
  // A copy of the schema like every client holds. The server's fingerprint
  // check catches a disagreement at query time; this catches it at test time,
  // and names the table rather than printing two hashes.
  assert.deepEqual(TABLES, tablesInConfig());
});
