# Two CI failures the local runs could not see

- **Date:** 2026-09-18
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/batchbench/run.sh`, `examples/explorer/web/src/{api.ts,panels.tsx}`
- **Kind:** fix

## What changed

Two jobs were red and both were real.

**`batching, from a client` could not resolve `@slate-orm/client`.** The node
arm links the TypeScript client by `file:` and imports its built `dist/`.
Nothing rebuilds that — `npm ci` in the client does not — so on a clean
checkout the module does not exist. The benchmark now builds the client first,
which is what `examples/explorer/run.sh` already does, at length and with the
reasoning spelled out.

**`demo frontend units` failed on a table the demo did not know about.** N1
added `editions` to `examples/explorer/head.toml` and did not add it to the
frontend's `TABLES`. A guard test reads the TOML and compares; it went red on
the N1 commit and stayed red for three commits.

## Why the local runs passed

**The benchmark passed here because a `dist/` was lying around** from running
the TypeScript client's own suite earlier in the same session. The benchmark
never built it and never needed to, so the missing step was invisible until a
machine without that history ran it. Reproduced by deleting
`clients/typescript/dist` and running again: the same `TS2307` as CI.

**The frontend units were never run after N1.** Its diff touched `head.toml`
and no frontend file, and the four suites I did run — kernel, three clients,
conformance, e2e — do not include `npm test` in `examples/explorer/web`. The
guard did exactly what it was built for and nobody read it for three commits,
which is the same failure as the formatting job earlier in this branch: a
check that fires into a log nobody opens is a check that has not run.

## The second bug the guard found

The relationships panel from N5 hardcoded `["id", "book_id", "label"]` for the
editions table. The column is `format`. The panel rendered — a wrong header
over the right values — and the e2e passed, because it asserted on the row
count and on the word "edition" rather than on a column name.

It now reads `TABLES["editions"]`, so the guard that compares `TABLES` against
`head.toml` covers the panel too. That is the fix worth having: the hardcoded
list would have been correct-and-unchecked, and the next schema change would
have broken it silently again.

## Alternatives rejected

**A `prepare` script on the TypeScript client, so `npm install` builds it.**
Would fix the benchmark and the demo at once, and npm does not run `prepare`
for a `file:` dependency's linked package — so it would look like a fix, work
on a fresh `npm install` in some layouts, and not work here. Rejected because
a fix that works by accident is worse than an explicit build step.

**Copying the demo's comment into the benchmark.** Rejected: the benchmark
points at the original rather than duplicating four paragraphs that would then
have two places to drift.

**Leaving `TABLES` alone and giving the panel its own column list.** That is
what the bug *was*. The point of a single map is that one guard covers
everything reading it.

## Evidence

The benchmark: `rm -rf clients/typescript/dist` then `./run.sh --rows 20
--runs 2` reproduced CI's `TS2307` before the change and produced the table
after it.

The frontend: 12 unit tests pass, 11 before. The e2e is 22/22 with the
corrected column names.

## What this does not do

**Nothing makes the frontend units part of what I run before pushing.** The
habit failed twice on this branch — once for formatting, once for this — and
the answer is not a longer list to remember. A script that runs every suite is
the obvious thing and is not written here, because the right shape for it is
the same question CI already answers and one more place for the two to
disagree.
