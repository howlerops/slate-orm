import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

import { directoryOf, findUpContaining } from "../src/paths.js";

/**
 * Is this package importable the way it says it is?
 *
 * It was not. `package.json` pointed `exports` at `./dist/index.js` while the
 * build emitted `dist/src/index.js`, so `npm install` produced a package that
 * could not be imported at all — and every test in this suite passed, because
 * they import from `../src/` by relative path and never go through the package
 * entry point.
 *
 * That is the whole class of bug: a test suite that reaches past the packaging
 * cannot see the packaging. It was found by building something that installed
 * the package, which is the only way it could have been.
 */

/**
 * The package root, found by what is in it rather than by counting `..`.
 *
 * This file runs from `test/` in source and `dist-test/test/` once compiled,
 * so a relative depth is right in exactly one of those. Third place in this
 * repository to get that wrong before getting it right — hence the shared
 * helper.
 */
const ROOT = findUpContaining(
  directoryOf(import.meta.url),
  ["package.json", "tsconfig.build.json"],
  "@slate-orm/client package root",
);

test("the package's exports point at files the build produces", () => {
  const manifest = JSON.parse(readFileSync(path.join(ROOT, "package.json"), "utf8")) as {
    exports: Record<string, Record<string, string>>;
    main: string;
    types: string;
  };

  const referenced = new Set<string>([manifest.main, manifest.types]);
  for (const entry of Object.values(manifest.exports)) {
    for (const target of Object.values(entry)) referenced.add(target);
  }

  // The build has to have run. `npm test` runs the test compile, not the
  // package build, so this says which command is missing rather than failing
  // as a bare "file not found".
  if (!existsSync(path.join(ROOT, "dist"))) {
    execFileSync("npx", ["tsc", "-p", "tsconfig.build.json"], { cwd: ROOT });
  }

  for (const target of referenced) {
    assert.ok(
      existsSync(path.join(ROOT, target)),
      `package.json references ${target}, which the build does not produce`,
    );
  }
});

test("the entry point actually imports and exports the public surface", async () => {
  // Not just "the file exists": a `dist/index.js` whose own relative imports
  // are wrong exists and still throws on import.
  const entry = path.join(ROOT, "dist", "index.js");
  const module_ = (await import(entry)) as Record<string, unknown>;

  for (const name of ["Client", "newJoin", "uint", "isKind", "SlateError"]) {
    assert.ok(name in module_, `the package does not export ${name}`);
  }
});
