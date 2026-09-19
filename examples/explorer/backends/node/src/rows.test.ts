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
import { test } from "node:test";

import type { Value } from "@slate-orm/client";

import { BooksChecks, decodeBooks } from "./schema.js";

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
