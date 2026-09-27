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
import test from "node:test";

import {
  adaptersFrom,
  api,
  DEFAULT_ADAPTERS,
  kindOf,
  render,
  NOT_IN_THE_UI,
  TABLES,
  type Tagged,
  VIEWS,
} from "../src/api.js";
import { CATALOG_TABLES, CATALOG_VIEWS } from "../src/catalog.js";

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
 * What the UI shows against what the catalog holds.
 *
 * This used to parse `head.toml` with thirty lines of regex and compare the
 * hand-written `TABLES` against it. That guard worked and was itself a second
 * implementation of catalog resolution — `scripts/codegen.py`'s docstring
 * spends a paragraph on why re-interpreting the TOML is the thing not to do,
 * and this file was doing it. `src/catalog.ts` is generated from
 * `slate-serverd --print-schema` and re-checked by CI with `--check`, so the
 * comparison against the config now happens there, against the *resolved*
 * catalog rather than against a regex over its source.
 *
 * What is left here is the part generation cannot decide: which of the
 * catalog's tables the UI shows. `NOT_IN_THE_UI` is the roster for that, and
 * these tests keep it honest in both directions.
 */
test("the UI shows every table the catalog has, minus the ones it names", () => {
  for (const [name, reason] of Object.entries(NOT_IN_THE_UI)) {
    assert.ok(
      name in CATALOG_TABLES,
      `NOT_IN_THE_UI names ${name}, which the catalog no longer has`,
    );
    assert.ok(!(name in TABLES), `${name} is in the UI, so it is not ${reason}`);
  }
  const expected = Object.fromEntries(
    Object.entries(CATALOG_TABLES).filter(([name]) => !(name in NOT_IN_THE_UI)),
  );
  assert.deepEqual(TABLES, expected);
  assert.ok(Object.keys(TABLES).length > 0, "a UI showing no tables would pass everything above");
});

test("every view the UI offers reads a table, with that table's columns", () => {
  // Not a column list of its own, which is the point: `docs/views.md` refuses
  // a projection in a view, so a view's ordinals are its base table's and the
  // UI has nothing separate to get wrong. An identity check on the array, so a
  // generator that emitted a copy — which would agree today and drift on the
  // first column added — fails here.
  assert.ok(Object.keys(VIEWS).length > 0, "no views is not a passing state for this test");
  for (const [name, columns] of Object.entries(VIEWS)) {
    const base = Object.entries(CATALOG_TABLES).find(([, theirs]) => theirs === columns);
    assert.ok(base, `view ${name} carries a column list that is no table's`);
    assert.strictEqual(columns, CATALOG_TABLES[base[0]], `view ${name} should be ${base[0]}'s own list`);
  }
});

test("a view the UI hides is hidden by the same roster its table is", () => {
  // `shown` filters views by table name, not by base table. That is right —
  // a view has its own name and its own reason to be hidden — and it is worth
  // a case, because the filter reads as if it were about tables.
  for (const name of Object.keys(CATALOG_VIEWS)) {
    assert.equal(
      name in VIEWS,
      !(name in NOT_IN_THE_UI),
      `view ${name} is shown iff NOT_IN_THE_UI does not name it`,
    );
  }
});
