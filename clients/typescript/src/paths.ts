/**
 * Finding a directory by what is in it, rather than by counting `..`.
 *
 * This exists because the same bug was written three times in one day. A
 * module resolves a sibling directory with a fixed number of `..` segments,
 * which is correct from `src/` and wrong from the compiled `dist/src/` — and
 * the failure is not a type error, it is a missing file at the first call.
 *
 * The three were the proto loader, the test harness, and the packaging test.
 * All three now call this, so there is one implementation to get right.
 */
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * The nearest ancestor of `from` — itself included — that `matches`.
 *
 * Throws rather than returning undefined: every caller here needs the
 * directory to proceed, and a `undefined` that flows on becomes a confusing
 * error somewhere further away. `what` names the thing being looked for, so
 * the throw says what was missing rather than only where.
 */
export function findUp(from: string, matches: (dir: string) => boolean, what: string): string {
  let dir = from;
  for (;;) {
    if (matches(dir)) return dir;
    const parent = path.dirname(dir);
    if (parent === dir) throw new Error(`no ${what} found above ${from}`);
    dir = parent;
  }
}

/** The directory holding the module that calls this. */
export function directoryOf(moduleUrl: string): string {
  return path.dirname(fileURLToPath(moduleUrl));
}

/** The nearest ancestor containing every one of `entries`. */
export function findUpContaining(from: string, entries: string[], what: string): string {
  return findUp(from, (dir) => entries.every((e) => existsSync(path.join(dir, e))), what);
}
