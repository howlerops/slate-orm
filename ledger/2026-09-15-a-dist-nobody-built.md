# The deployed example compiled against a `dist/` that a different example had built

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `examples/deployed/run.sh`
- **Kind:** fix

## What changed

`examples/deployed/run.sh` now builds `clients/typescript` before compiling
`examples/deployed/node`. Three lines, one of them the command.

## Why

The deployed run's TypeScript check imports `@slate-orm/client`, a `file:`
dependency on `clients/typescript`. `npm install` on a `file:` dependency
symlinks the directory; it does not run that package's build, and the package's
`main`/`types` point into `dist/`. So the compile needs a `dist/` that nothing
in the script produced.

It passed here anyway, for the worst available reason: `examples/explorer/run.sh`
builds that same `dist/` — it has a comment saying why, since a stale one there
makes the conformance runner report the three SDKs disagreeing — and an earlier
explorer run had left one behind. The deployed script was reading an artifact it
had no part in creating and could not have noticed was missing.

On a clean checkout it is not subtle:

```
src/check.ts(38,8): error TS2307: Cannot find module '@slate-orm/client' or its
corresponding type declarations.
```

which is what the `deployed against object storage` job said on c780810, in the
first CI run that had a Node step in that job at all. The job was added with Go
and Node toolchains in the same commit that added the two checks, so this is the
first time anything ran it anywhere but a machine with a warm tree.

The class is worth naming, because it is the one the repository keeps finding:
**a check that passes because of state a different check left behind is not a
check.** It is the same shape as the workflow that had never fired and the
Python suite that skipped itself — green for a reason unrelated to the thing
being tested.

## Alternatives rejected

**A shared build script for both examples.** One place to fix instead of two,
and it is the obvious refactor. Rejected because the two need different things
at different moments: the explorer builds the client so a long-running *adapter*
is not stale, the deployed run builds it so a one-shot *compile* can resolve the
module. Collapsing them would put a `dist/`-building step behind a name that
means "what the demo needs", and the next script to need one would either grow
a third copy or bend the shared one. Two commands with a comment each is the
cheaper duplication.

**`npm run build` in a `prepare`/`prepack` script on the client.** npm runs
`prepare` for a `file:` dependency install, which would make this work with no
change to either example. Rejected because it makes the client's install hook
responsible for its own build artifacts in a way that only shows up through
consumers: a broken `prepare` fails at every `npm install` in the repository,
including ones that do not want the build, and CI's `npm ci` in
`clients/typescript` already builds it explicitly where it needs it. Making the
dependency edge implicit is the opposite of what this failure argues for.

**Compiling the check against the client's sources instead of its `dist/`**
(project references, or a path mapping). Removes the artifact dependency
entirely. Rejected because it would type-check against a *different* thing than
the three SDK suites and the explorer consume — the built package — so a
`dist/`-only problem (a bad `exports` map, a missing file in `files`) would be
invisible to exactly the run whose whole point is to be end-to-end.

## Evidence

Reproduced by hand before the fix, deliberately: `clients/typescript/dist` moved
aside, then

```
$ cd examples/deployed/node && npx tsc -p tsconfig.json
src/check.ts(38,8): error TS2307: Cannot find module '@slate-orm/client' or its
corresponding type declarations.
```

byte for byte the line CI printed, including the offset. With `dist/` still
moved aside and the new build line run first, the same compile succeeds. That is
the whole of it: the failure is deterministic given a clean tree, and the fix is
the one command.

Not a mutation test — there is nothing here to mutate. The falsifiable claim is
"this script does not depend on another script having run", and the way to check
it is a clean tree, which is what CI is.

## What this does not do

**Nothing else in the repository is audited for the same dependency.** The
explorer builds the client; the three client suites `npm ci` in
`clients/typescript` and build it there. Those are the consumers I know of, and
I checked those. A future script that adds a `file:` dependency on the client
will have this bug again and nothing will stop it — a guard would have to be a
test that runs a script in a tree with no build artifacts, which is a real thing
to want and is not here.

**It does not make the deployed run cheaper to be wrong about.** The whole
script is still ~4 minutes of S3 server, load, three SDKs, kill, restart, three
SDKs again, and a compile error 30 seconds in is the failure mode this fix
addresses by moving every build before anything starts. That ordering was
already deliberate and already commented; the missing build was simply not in
the list.

**The `deployed` job's Node and Go steps have now run exactly once each**, and
that run failed at the compile. The Go check has not yet been observed passing
in CI; the TypeScript one has not yet been observed running at all. Both pass
here. The next run is the first evidence either works on a clean machine.
