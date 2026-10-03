# The npm package would have shipped with no JavaScript in it

- **Date:** 2026-10-03
- **Author:** Claude Code (session: what's next, after the release)
- **Touches:** `clients/typescript/package.json`, `scripts/check_npm_package.py`,
  `scripts/test_check_npm_package.py`, `scripts/mutations.json`,
  `scripts/check.sh`, `.github/workflows/ci.yml`,
  `.github/workflows/release.yml`, `.github/workflows/mutations.yml`,
  `scripts/test_check_sh.py`
- **Kind:** fix

## What changed

**`npm publish` would have shipped five files and no code.** Found while
preparing to turn the registry jobs on, by doing the one thing nobody had
done: asking npm what it would actually send, from a checkout with only
tracked files in it.

```
README.md
package.json
proto/google/rpc/error_details.proto
proto/google/rpc/status.proto
proto/slate/v1/records.proto
```

`main` is `./dist/index.js`. It is not in that list.

`prepack` builds `dist` now, the manifest declares a licence and a
repository, and `scripts/check_npm_package.py` asks npm what the tarball
contains and refuses if a declared entry point is missing from it — in
`check.sh`'s suite, in CI on every push, and as the last step before
`npm publish`.

## Why

`dist/` is build output and is gitignored, correctly. The publish job ran
`npm ci`, then `npm test`, then `npm publish`. **Nothing built `dist`.**
`npm test` compiles with `tsconfig.json`, whose `outDir` is `dist-test`; only
`npm run build` uses `tsconfig.build.json`, and nothing called it.

Locally this is invisible, because anybody who has ever run `npm run build`
has a `dist/` sitting there. On a fresh CI checkout it is not there, and npm
packs what it finds without complaint — `files: ["dist", "proto"]` naming a
directory that does not exist is not an error.

**The consequence is the unfixable kind.** npm refuses a republish of a
version that has been used. A consumer running `npm i @slate-orm/client`
would install a package whose `main` resolves to nothing, and the remedy
would not be to fix and republish `0.1.0` — it would be to burn the number,
publish `0.1.1`, and explain.

That is precisely the failure `scripts/check_versions.py` was built for, one
layer down. That guard holds four declarations of the version to each other
and says nothing whatever about whether the thing wearing the number has any
content in it. Both are needed, and only one existed.

**Nothing had noticed because the job has never run.** `vars.PUBLISH_NPM` is
unset, so the npm job reports "not published" and succeeds — which is the
tri-state working as designed, and is also why a latent defect sat in the
publish path through a release that reported five artefacts.

The Python side was checked the same way and is **fine**: `python -m build`
from a clean checkout produces a 24-entry wheel with every module, the
generated protobuf stubs and `py.typed`. The asymmetry is the cause — Python's
generated code is committed, TypeScript's is compiled.

## Alternatives rejected

**Adding `npm run build` to the publish job.** One line, and it fixes this
workflow and nothing else. Rejected because the defect is in the *package*:
`npm pack` by hand, or any other publisher, produces the same empty tarball.
`prepack` is the hook npm provides for exactly this and makes the package
correct however it is published.

**`prepublishOnly` rather than `prepack`.** Narrower — it does not run for
`npm pack`, so `npm pack` would still produce the broken tarball and the
guard below would then disagree with reality in the safe direction but for
the wrong reason. `prepack` covers both.

**Trusting `prepack` and skipping the guard.** The fix is one line; the thing
worth keeping is the *question*. "Does the tarball contain what the manifest
promises" stays true when somebody adds an export, renames `outDir`, or
rewrites `files`, and none of those is caught by the presence of a `prepack`
script.

**A guard that checks `files` contains `dist`.** That was true the entire
time it was broken. The only honest check is the tarball itself.

**Keeping the guard out of CI and running it only before publishing.** It is
in both, and the release-day-only version is the mistake this repository
wrote an entry about yesterday. On every push it is three seconds; on release
day it is the difference between a red job and a burned version.

**An `unmutated` exemption rather than giving `mutations.yml` a Node.** The
guard shells out to `npm`, and that workflow installs Python only — so the
suite would report *red before any mutation*, which reads as a defect in the
tree rather than a missing toolchain. Thirty seconds of `setup-node` and
`npm ci` buys a guard that is genuinely mutation-tested, against an exemption
for a guard that can perfectly well be mutated.

## Evidence

**The defect, demonstrated rather than argued.** `git archive HEAD
clients/typescript` into a temporary directory — tracked files only, exactly
what CI checks out — then `npm pack --dry-run --json`:

| | files packed |
|---|---|
| clean checkout, as the publish job had it | **5** |
| this working tree, which has a stale `dist/` | 25 |

The second row is why nobody saw it.

**The guard, run against that same clean tree with `prepack` absent:**

```
the npm package would ship without what it promises (5 files packed):
  FAIL  `exports...types` promises ./dist/index.d.ts, which is not in the tarball.
  FAIL  `exports...default` promises ./dist/index.js, which is not in the tarball.
```

**Eight fixture cases**, `scripts/test_check_npm_package.py`, including the
real five-file list as a fixture rather than an invented one, a target nested
under an unusual `exports` condition, a `bin` map, a string `exports`, and two
never-fires cases — a manifest promising nothing, and an empty tarball.

**One real-tree mutation, caught**
(`ledger/mutations/20261003T032929-clients-typescript-package-json.json`):
dropping `dist` from `files` makes npm pack the protos and the README and
nothing else, and the guard refuses by name.

**The first mutation I rostered was a survivor, and the cause was the third
one.** Removing `prepack` — the actual historical bug — survived, because
this working tree has a stale `dist/` on disk and the guard asks npm what it
would pack *now*. The mutation was a real change and the guard genuinely
could not see it. That is a limitation worth knowing rather than a defect to
hide: the guard is exact in a clean checkout, which is every checkout that
publishes, and approximate in a developer's tree. It is written into the
guard's docstring, and the rostered mutation moved to `files`, which is
scored identically everywhere.

**Two findings from the guard's own tooling**, both kept:

- the suite asserted on `main` and was answered about `types`, because
  `promised()` keyed by path and the two share one. It now records every
  place a path was promised and the message names them all.
- `ty` refused `guard.packed = stub`. It was right: the declared type is that
  one function, not any function shaped like it. `main()` takes the pack
  function as an argument now — a declared seam instead of an undeclared one.

**Not measured.** Three seconds of `npm pack --dry-run` per CI run.

## What this does not do

**It does not check that the package *works*.** A tarball containing
`dist/index.js` can still export the wrong thing, or import a dependency it
does not declare. Installing the packed tarball into a scratch project and
importing it is the next step up, and it needs a registry or a file install
that nothing here does yet.

**It does not look at the Go module.** `clients/go` has no build step and no
`files` list — the proxy serves the tagged source — so the class does not
exist there. The nearest equivalent is the proxy resolution check the release
already does.

**`dist-test` is still what `npm test` builds.** The tests exercise a
different compilation of the same sources than the one that ships: a
`tsconfig.build.json`-only error — a wrong `rootDir`, an excluded file —
would pass the tests and ship. The guard catches the shape of that where it
matters (a missing entry point) and not a subtler one.

**The two registries remain off.** This makes turning them on safe; it does
not turn them on, and the names are still unclaimed.
