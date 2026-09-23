import assert from "node:assert/strict";
import { after, test } from "node:test";

import { start, type Serving } from "./harness.js";
import {
  type Query,
  type Session,
  contains,
  str,
  uint,
  usingIndex,
  usingTableScan,
} from "../src/index.js";

/**
 * `contains`, through a real node and onto a real inverted index.
 *
 * The kernel's own suite proves a text index holds one entry per term and that
 * a search over it agrees with a scan. This is the TypeScript half of the
 * other claim: that a caller can express the search, that the *server* does
 * the splitting, and that `text = true` in a TOML schema declares an index a
 * query can reach.
 *
 * The table below is this file's own, which is what makes it a test of the
 * TOML surface as well as of the client: `slate-serverd` reads this text and
 * builds the index from it.
 *
 * Every search that claims the index says so with a hint, and
 * "the index and a scan find the same rows" checks the two plans differ before
 * it compares their rows. A text index is not chosen by cost at four rows —
 * `docs/full-text.md` measures the crossover at roughly one row in 24,000 — so
 * an unhinted search here would be a table scan and would pass with the index
 * deleted.
 */
const FULLTEXT_TABLES = `
[[tables]]
name = "articles"
id = 62
columns = [
  { name = "id",    type = "u64" },
  { name = "title", type = "str" },
]
primary_key = ["id"]

[[tables.indexes]]
name = "by_title_text"
id = 62
columns = ["title"]
text = true

[[security.grants]]
role = "app"
tables = ["articles"]
actions = ["everything"]
`;

const ID = 0;
const TITLE = 1;

/**
 * Prose chosen so the terms overlap three different ways: "rust" and "the"
 * appear in several, "storage" in two, "engine" in one. A corpus where each
 * term picked out exactly one row would make a conjunction and a disjunction
 * indistinguishable.
 */
const ARTICLES: Array<[number, string]> = [
  [1, "Rust and the storage engine"],
  [2, "The storage layer, revisited"],
  [3, "rust: a retrospective"],
  [4, "Nothing to do with either"],
];

const servers: Serving[] = [];
after(() => {
  for (const s of servers) s.stop();
});

async function articled(): Promise<Session> {
  const server = await start(FULLTEXT_TABLES);
  servers.push(server);
  const session = server.client().session();
  await session.insert(
    "articles",
    ...ARTICLES.map(([id, title]) => [uint(id), str(title)]),
  );
  return session;
}

/** A `contains` over the articles, on the text index unless told otherwise. */
const search = (text: string, hinted = true): Query => ({
  table: "articles",
  filter: contains(TITLE, text),
  sort: [{ column: ID }],
  hint: hinted ? usingIndex("by_title_text") : usingTableScan(),
});

async function foundIds(session: Session, query: Query): Promise<number[]> {
  const found: number[] = [];
  for await (const row of session.query(query)) {
    const id = row[ID];
    assert.ok(id?.kind === "uint", `id came back as ${id?.kind}`);
    found.push(Number(id.value));
  }
  return found;
}

test("one term finds every row holding it", async () => {
  const session = await articled();
  assert.deepEqual(await foundIds(session, search("rust")), [1, 3]);
});

test("the server lowercases the search", async () => {
  // The client sent `RUST`; the rows hold `Rust` and `rust`. The fold happened
  // on the server, because `contains` does none.
  const session = await articled();
  assert.deepEqual(await foundIds(session, search("RUST")), [1, 3]);
});

test("two terms are a conjunction, not a disjunction", async () => {
  // Rows 1 and 3 hold "rust"; rows 1 and 2 hold "storage". Only row 1 holds
  // both, and an OR would return three.
  const session = await articled();
  assert.deepEqual(await foundIds(session, search("rust storage")), [1]);
});

test("punctuation is a separator and not part of a term", async () => {
  // Row 3 is written `rust: a retrospective`, so its first term is `rust`, and
  // a search for `rust:` must find it. If the client sent terms rather than
  // text this is the case where its splitting and the server's would have to
  // agree by luck.
  const session = await articled();
  assert.deepEqual(await foundIds(session, search("rust:")), [1, 3]);
});

test("a substring of a term is not a term", async () => {
  // `stor` is a prefix of `storage` and matches nothing. This is the line
  // between `contains` and `like("%stor%")`, and the reason both exist.
  const session = await articled();
  assert.deepEqual(await foundIds(session, search("stor")), []);
});

test("the index and a scan find the same rows", async () => {
  // The oracle: two access paths, one answer. The plans are compared first,
  // without which this would pass with the hint ignored and both sides
  // scanning.
  const session = await articled();
  for (const text of ["rust", "the storage", "rust storage engine", "nothing"]) {
    const indexed = search(text);
    const scanned = search(text, false);

    const byIndex = (await session.explain(indexed)).access;
    const byScan = (await session.explain(scanned)).access;
    assert.notEqual(
      byIndex,
      byScan,
      `both paths planned the same way for ${text} (${byIndex}), so this compares a scan against a scan`,
    );
    assert.ok(byIndex.includes("by_title_text"), byIndex);

    assert.deepEqual(await foundIds(session, indexed), await foundIds(session, scanned), text);
  }
});
