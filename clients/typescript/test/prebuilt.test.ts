import assert from "node:assert/strict";
import { mkdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";

import { ROOT, binary } from "./harness.js";

/**
 * The refusal in `harness.ts`, executed.
 *
 * `scripts/test_prebuilt.py` holds all six harnesses that read `SLATE_SERVERD`
 * or `SLATE_TESTSERVER` to refusing a binary older than the source it was
 * built from. It did so by grep, and
 * `ledger/2026-09-30-the-exemptions-i-wrote-without-reading.md` recorded that
 * as the roster's own limit: it can tell that a file mentions a refusal, not
 * that the refusal works, so a harness that defines one and never calls it
 * passes. A harness in exactly that state is what cost a `slate-server`
 * mutation its finding on 2026-09-30.
 *
 * `binary()` is the function `start()` calls, so this drives the real path
 * rather than a copy of it. The shell and Python harnesses are driven directly
 * by `scripts/test_prebuilt.py`, which cannot run this one without Node.
 */


/**
 * A binary inside `target/`, not in the system temp directory.
 *
 * The walk starts at the repository root, so a path outside the checkout
 * compares against nothing and is accepted for a legitimate reason the
 * refusal documents — a released binary with no sources beside it. A test
 * placed there would pass without the refusal ever firing.
 */
function scratch(name: string, mtime: Date): string {
  const dir = path.join(ROOT, "target", name);
  mkdirSync(dir, { recursive: true });
  const file = path.join(dir, "slate-serverd");
  writeFileSync(file, "not a real binary");
  utimesSync(file, mtime, mtime);
  return file;
}

function withServerd<T>(value: string, body: () => T): T {
  const saved = process.env["SLATE_SERVERD"];
  process.env["SLATE_SERVERD"] = value;
  try {
    return body();
  } finally {
    if (saved === undefined) delete process.env["SLATE_SERVERD"];
    else process.env["SLATE_SERVERD"] = saved;
  }
}

test("a prebuilt binary older than the source is refused", () => {
  const file = scratch("prebuilt-test-ts", new Date("2000-01-01T00:00:00Z"));
  try {
    assert.throws(
      () => withServerd(file, binary),
      /was built before/,
      "a binary from 2000 was accepted",
    );
  } finally {
    rmSync(path.dirname(file), { recursive: true, force: true });
  }
});

test("a prebuilt path that is set and missing is refused", () => {
  // Never a silent fall back to `cargo build`: the fallback tests a different
  // binary from the one the caller named.
  assert.throws(
    () => withServerd(path.join(ROOT, "target", "definitely-absent"), binary),
    /does not exist/,
  );
});

test("a prebuilt binary newer than every source is accepted", () => {
  // The never-fires half. Both cases above pass if `binary()` throws at
  // everything, which is also what a broken walk would do.
  const soon = new Date(Date.now() + 60 * 60 * 1000);
  const file = scratch("prebuilt-test-ts-fresh", soon);
  try {
    assert.equal(withServerd(file, binary), file);
  } finally {
    rmSync(path.dirname(file), { recursive: true, force: true });
  }
});
