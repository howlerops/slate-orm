import assert from "node:assert/strict";
import { test } from "node:test";

import {
  array,
  int,
  str,
  valueFromWire,
  valueKey,
  valuesEqual,
  valueToWire,
  type Value,
} from "../src/value.js";

/**
 * Array values, with no server.
 *
 * The round trip through a real node is covered by the conformance runner;
 * these three cases are about the parts that are this client's own — the
 * wire shape, structural equality and the `Map` key — and each one had a
 * plausible wrong answer.
 */

test("an array round-trips through the wire shape, elements and all", () => {
  const value = array([str("rust"), str("db")]);
  const wire = valueToWire(value) as { arrayValue: { elements: unknown[] } };
  assert.equal(wire.arrayValue.elements.length, 2);

  // `valueFromWire` reads the loader's shape, which tags the set field under
  // `kind`; the encoder does not write that, so it is added here the way the
  // loader would.
  const back = valueFromWire({
    kind: "arrayValue",
    arrayValue: {
      elements: wire.arrayValue.elements.map((e) => ({
        kind: "stringValue",
        ...(e as Record<string, unknown>),
      })),
    },
  });
  assert.deepEqual(back, value);

  // An empty array is an array, not a null and not an absent field.
  assert.deepEqual(valueFromWire({ kind: "arrayValue", arrayValue: { elements: [] } }), {
    kind: "array",
    value: [],
  });
});

test("arrays compare element-wise, not by reference", () => {
  // The obvious implementation compares elements with `===`, which is true for
  // numbers and false for every object — so every non-empty array would be
  // unequal to an identical one, including to a copy of itself.
  assert.ok(valuesEqual(array([str("a")]), array([str("a")])));
  assert.ok(!valuesEqual(array([str("a")]), array([str("b")])));
  assert.ok(!valuesEqual(array([str("a")]), array([str("a"), str("b")])));
  assert.ok(!valuesEqual(array([]), array([str("a")])));
  // Kind is part of equality one level down too: `7` as an int and as a uint
  // are different values to this server.
  assert.ok(!valuesEqual(array([int(7n)]), array([str("7")])));
});

test("an array's key cannot collide with a different array", () => {
  // The case the first version got wrong. An element key is `kind:payload`
  // and a string payload can contain anything, so joining keys with any fixed
  // separator lets one element impersonate two.
  const one: Value = array([str("a|string:b")]);
  const two: Value = array([str("a"), str("b")]);
  assert.notEqual(valueKey(one), valueKey(two));
  assert.ok(!valuesEqual(one, two));

  // And the ordinary requirement: same value, same key; different value,
  // different key.
  assert.equal(valueKey(array([str("a")])), valueKey(array([str("a")])));
  assert.notEqual(valueKey(array([str("a")])), valueKey(array([str("b")])));
  assert.notEqual(valueKey(array([])), valueKey(array([str("")])));
});
