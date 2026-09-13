# The TypeScript package was not importable, and its own suite could not tell

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `clients/typescript/{package.json,tsconfig.json,tsconfig.build.json}`,
  `clients/typescript/test/package.test.ts` (new)
- **Kind:** bugfix

## What changed

`package.json` pointed `exports` at `./dist/index.js`; the build emitted
`dist/src/index.js`. So `npm install @slate-orm/client` produced a package that
could not be imported at all. There is now a build-only tsconfig that emits the
advertised layout, tests compile to `dist-test/`, and a test asserts the package
entry point resolves and imports.

## Why

Found by building the explorer demo's Node adapter, which installs the client
as a dependency. Every one of the 37 tests shipped with it passed, and none of
them could have caught this: they import from `../src/` by relative path and
never go through the package entry point at all.

That is the general shape — **a test suite that reaches past the packaging
cannot see the packaging** — and it is why the fix is a test rather than only a
corrected path.

The `test` script was wrong in the same direction: it ran
`node --test dist-test/*.test.js` against a config that emitted to `dist/`. It
had never run as written; I had been invoking the right path by hand and not
noticing the script disagreed.

## Alternatives rejected

**Pointing `exports` at `./dist/src/index.js`.** One character, and it enshrines
a layout that leaks the source tree into the published path. A consumer's import
specifier should not depend on how the package's own sources are arranged.

**A bundler.** Correct for a package with many entry points and overkill for
one; a second tsconfig is 6 lines and no dependency.

**Trusting `npm pack --dry-run` in CI instead of a test.** Would catch the
missing file and not catch an entry point whose own relative imports are wrong,
which exists and still throws. The test imports it.

## Evidence

39 TypeScript tests pass (37 before, plus two here). Before the fix, both new
tests fail: the first because `dist/index.js` does not exist, the second because
importing it throws. `node -e "import('./dist/index.js')"` now resolves 56
exported names.

The demo's Node adapter — the thing that found this — now compiles against the
installed package.

## What this does not do

The Go and Python clients are not checked this way. Go has no equivalent failure
mode (the module path is the import path). Python's is real — a wrong
`packages` in `pyproject.toml` produces the same class of bug — and is not
tested here.

Three separate files in this repository have now had the same "count the `..`
segments" bug: the TypeScript test harness, the proto loader, and this test. All
three now search upward for a landmark instead. Nothing stops a fourth.
