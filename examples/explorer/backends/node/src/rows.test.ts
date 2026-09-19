/**
 * Hand-written beside a generated file, and it must stay that way: a test the
 * generator emitted would agree with the generator by construction.
 *
 * It exists because the Go and TypeScript decoders shipped compiled but never
 * *run*. `tsc --noEmit` proves they type-check; nothing proved they decode the
 * right column, and reading the neighbouring column type-checks perfectly
 * whenever the neighbour happens to share a type.
 *
 * Run with node's own test runner over the compiled output — no test framework
 * for one file, and the compile is the same `tsc` the backend already does.
 */
import { strict as assert } from "node:assert";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import type { Value } from "@slate-orm/client";

import {
  BooksChecks,
  ShipmentsChecks,
  decodeAuthors,
  decodeBooks,
  decodeEditions,
  decodeSales,
  decodeShipments,
} from "./schema.js";

/** A well-formed `books` row: the ordinals the catalog declares, in order. */
function bookRow(): Value[] {
  return [
    { kind: "uint", value: 7n },
    { kind: "uint", value: 3n },
    { kind: "string", value: "A Book In Flight" },
    { kind: "int", value: 2026n },
    { kind: "float", value: 4.5 },
    { kind: "int", value: 1_700_000_000n },
    { kind: "vector", value: [0.1, 0.2, 0.3, 0.4] },
    { kind: "units", value: 1250n },
  ];
}

test("decodeBooks decodes every column", () => {
  const book = decodeBooks(bookRow());
  assert.equal(book.id, 7n);
  assert.equal(book.author_id, 3n);
  assert.equal(book.title, "A Book In Flight");
  assert.equal(book.year, 2026n);
  assert.equal(book.rating, 4.5);
  // A decimal is a count of the smallest unit; the scale lives in the schema,
  // so 1250n at scale 2 is 12.50 and this type does not know that.
  assert.equal(book.price, 1250n);
  assert.equal(book.embedding.length, 4);
});

test("decodeBooks refuses a transposed row", () => {
  const row = bookRow();
  [row[2], row[3]] = [row[3]!, row[2]!]; // title <-> year
  assert.throws(() => decodeBooks(row), /books\.title/);
});

test("decodeBooks cannot see a same-typed swap", () => {
  // Asserted rather than left implicit. "The decoder catches transposition" is
  // the kind of claim that grows in the retelling: it catches a *tag*
  // mismatch. Same-typed neighbours are what the ordinals in the generated
  // declaration are for, and what the schema fingerprint protects.
  const row = bookRow();
  [row[0], row[1]] = [row[1]!, row[0]!]; // id <-> author_id, both uint
  const book = decodeBooks(row);
  assert.equal(book.id, 3n);
  assert.equal(book.author_id, 7n);
});

test("decodeBooks refuses a short row", () => {
  assert.throws(() => decodeBooks(bookRow().slice(0, 4)), /8 columns/);
});

test("decodeBooks refuses a null in a non-nullable column", () => {
  const row = bookRow();
  row[2] = { kind: "null" };
  assert.throws(() => decodeBooks(row), /not nullable/);
});

test("BooksChecks carries the published rule", () => {
  const rule = BooksChecks["year_is_positive"];
  assert.ok(rule, `no such check, have ${Object.keys(BooksChecks).join(", ")}`);
  assert.equal(rule.column, "year");
  assert.equal(rule.predicate, "year > 0");
  assert.ok(rule.message, "the check should carry the sentence a form shows");
});

// --- every other table -------------------------------------------------------
//
// `books` had all of the above and the other four had nothing: generated,
// compiled, typechecked and never executed. The demo reads `authors`, `sales`,
// `editions` and `shipments` through the client's untyped `Value`s, so a wrong
// ordinal in any of their decoders would have been found by nobody.
//
// One case each rather than the six `books` gets, because what differs between
// tables is the column list and not the decoder's shape — the generator emits
// one shape. What each case asserts is that *this table's* ordinals are the
// catalog's. The transposition, short-row and null cases stay on `books`,
// which is where the shape is checked.

/** Every generated decoder, with a row the catalog says is valid. */
const DECODERS: Record<string, () => void> = {
  decodeAuthors: () => {
    const author = decodeAuthors([
      { kind: "uint", value: 3n },
      { kind: "string", value: "Ursula" },
      { kind: "string", value: "US" },
      { kind: "int", value: 1929n },
    ]);
    // `name` and `country` are both strings and adjacent, so the values differ
    // on purpose: a decoder reading ordinal 2 for `name` would pass a test
    // that used the same string for both.
    assert.deepEqual(author, { id: 3n, name: "Ursula", country: "US", born: 1929n });
  },

  decodeBooks: () => {
    // Covered six ways above; listed so the coverage check below sees it.
    assert.equal(decodeBooks(bookRow()).title, "A Book In Flight");
  },

  decodeSales: () => {
    assert.deepEqual(
      decodeSales([
        { kind: "uint", value: 11n },
        { kind: "uint", value: 7n },
        { kind: "int", value: 430n },
      ]),
      { id: 11n, book_id: 7n, units: 430n },
    );
  },

  decodeEditions: () => {
    assert.deepEqual(
      decodeEditions([
        { kind: "uint", value: 5n },
        { kind: "uint", value: 7n },
        { kind: "string", value: "paperback" },
      ]),
      { id: 5n, book_id: 7n, format: "paperback" },
    );
  },

  decodeShipments: () => {
    // The only generated decoder here with a nullable column, so this is the
    // only place that branch runs at all. Both ways round, because "null
    // becomes null" and "a value comes through" are two branches and a test of
    // one says nothing about the other.
    const live = decodeShipments([
      { kind: "uint", value: 9n },
      { kind: "uint", value: 7n },
      { kind: "string", value: "shipped" },
      { kind: "null" },
    ]);
    assert.deepEqual(live, { id: 9n, book_id: 7n, status: "shipped", deleted_at: null });

    const retired = decodeShipments([
      { kind: "uint", value: 9n },
      { kind: "uint", value: 7n },
      { kind: "string", value: "shipped" },
      { kind: "int", value: 1_700_000_042n },
    ]);
    assert.equal(retired.deleted_at, 1_700_000_042n);
  },
};

for (const [name, run] of Object.entries(DECODERS)) {
  test(`${name} decodes its own ordinals`, run);
}

test("every generated decoder is exercised", () => {
  // The table above is hand-written, which is the whole point — a test the
  // generator emitted would agree with it by construction — and a hand-written
  // list beside a generated file is the thing this repository has watched
  // drift five times. Reading the source turns "somebody remembers" into a
  // failure with the missing name in it.
  const source = readFileSync(new URL("../src/schema.ts", import.meta.url), "utf8");
  const declared = [...source.matchAll(/^export function (decode\w+)\(/gm)].map((m) => m[1]!);
  assert.ok(declared.length >= 5, `found ${declared.length} decoders; the pattern is not matching`);
  for (const name of declared) {
    assert.ok(name in DECODERS, `${name} is generated and nothing runs it; add it to DECODERS`);
  }
  for (const name of Object.keys(DECODERS)) {
    assert.ok(declared.includes(name), `DECODERS names ${name}, which schema.ts no longer declares`);
  }
});

test("ShipmentsChecks carries the enumeration codegen narrows from", () => {
  // The second published constraint, and the one a *type* is generated from:
  // `status` above is `"pending" | "shipped" | "delivered"` rather than
  // `string`, and this is the rule that says so.
  const rule = ShipmentsChecks["status_known"];
  assert.ok(rule, `no such check, have ${Object.keys(ShipmentsChecks).join(", ")}`);
  assert.equal(rule.column, "status");
  // `predicate` is nullable in `CheckRule` — a check declared with no parsed
  // text has none — and this one has it, which is the assertion.
  const predicate = rule.predicate ?? "";
  for (const value of ["pending", "shipped", "delivered"]) {
    assert.ok(predicate.includes(value), `the predicate ${predicate} omits ${value}`);
  }
});
