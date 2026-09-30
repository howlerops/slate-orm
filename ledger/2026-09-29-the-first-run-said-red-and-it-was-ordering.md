# The first run said red, and it was ordering rather than a toolchain gap

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `.github/workflows/ci.yml`
- **Kind:** fix

## What changed

`ledger/2026-09-29-fourteen-checks-that-ran-only-here.md` put fourteen steps
into CI and recorded, as its first caveat, that it did not prove they passed:

> The first CI run after this change is the one that says; if it is red, that
> is the finding rather than a regression.

Run 502 was red. **One step of the fourteen failed**, and this moves it four
steps later in the same job.

## Why

`examples/explorer/backends/node`'s `tsc --noEmit` reported thirteen errors.
Twelve are `TS7006: implicitly has an 'any' type` and one is the cause:

```
src/main.ts(97,8): error TS2307: Cannot find module '@slate-orm/client'
  or its corresponding type declarations.
```

The adapter depends on the TypeScript client by path —
`"@slate-orm/client": "file:../../../../clients/typescript"` — and that
package's `types` is `./dist/index.d.ts`, which `npm run build` produces. The
step went in after `go test ./schema/` and the build is four steps further
down, so it typechecked against a package with no declarations. Every value
from the client became `any`, and `noImplicitAny` turned one missing module
into thirteen errors.

**It passed here, and that is the point rather than an excuse.**
`clients/typescript/dist` already existed on this container from an earlier
run, so the local `check.sh` resolved the client and typechecked cleanly. The
whole argument of the previous entry is that a check running only where someone
happens to have run it is not a check, and the first thing CI did with a
newly-wired one was demonstrate it — on my own step, in the same hour.

Nothing about the toolchain gap CLAUDE.md warns about was involved. The
prediction in that caveat named the right risk and the failure came from
somewhere else, which is worth recording: the caveat was right to be there and
wrong about why.

## Alternatives rejected

**Build the client earlier in the job.** Moves the same dependency rather than
respecting it, and `npm run build` sits where it does because the steps between
it and `npm ci` do not need it. Reordering a build to suit a check is the
tail wagging the dog when moving the check costs one hunk.

**Point the adapter's `tsconfig` at the client's sources instead of `dist`.**
Would make the typecheck independent of the build, and would also make the
adapter compile against code the published package does not ship — the thing
`dist` exists to distinguish. A check that passes against sources and would
fail against the artefact is worse than one with an ordering requirement.

**Drop the step and roster it in `ONLY_LOCAL`.** The cheap retreat, and wrong:
the step found nothing here because `dist` was stale-but-present, and in CI it
is the only thing typechecking that adapter at all — `npm test` compiles what
its tests import.

## Evidence

Run 502 on `2ccf840`: **failure**, one job (`three SDKs, one database`), one
step (`Run npx --no-install tsc -p tsconfig.json --noEmit`). Every other job
green, including the nine guards and the four other toolchain checks on their
first CI run.

Read off the job's step list and log rather than the run's conclusion, which
says only that something failed.

The cause was confirmed rather than inferred: `clients/typescript/package.json`
declares `"types": "./dist/index.d.ts"`, and `clients/typescript/dist` exists
on this container, which is why the local run of the same command exits 0.

No mutation for this one. The change is a move within a YAML file, and the
thing that would catch a regression is CI itself running the step in the right
place — a mutation asserting "this step comes after that step" would be a test
of the line order rather than of the requirement.

## What this does not do

**`check.sh` has the same hidden prerequisite and still claims otherwise.** Its
header says it "needs no built binary, no browser, no container and no
network", and `ts-adapter-types` needs `clients/typescript/dist`. On a clean
checkout that step fails for the reason CI just did, and the sentence would be
wrong. Nothing checks it, and it is not fixed here: the fix is either a build
in a script whose contract is that it builds nothing, or a fifth exception to a
sentence that is currently absolute.

**Thirteen errors, one cause, and nothing says so.** A reader of that log sees
twelve `any` complaints and one missing module, in that order in the file and
the reverse order in importance. `tsc` has no mode that reports only the root,
and nothing in this repository post-processes the output.

**The other four toolchain checks passed once.** `gofmt` in two trees, the
client's `tsc`, the demo's `go vet` — one green run each. That is better than
the zero they had this morning and is not a claim that they are stable across
runner images.
