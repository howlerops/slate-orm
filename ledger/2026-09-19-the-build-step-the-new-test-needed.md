# The build step the new test needed

## What changed

One step in the three-SDK CI job: build `clients/typescript` before running the
node adapter's decoder test.

## Why

The decoder test added an hour ago turned that job red, three commits running.
It failed in one second on

```
src/main.ts(79,8): error TS2307: Cannot find module '@slate-orm/client'
```

— not on anything it was testing.

The node adapter resolves `@slate-orm/client` to a symlink into
`clients/typescript` and imports its **built** `dist/`. `npm ci` does not
produce that: there is no `prepare` script, so nothing builds the client as a
side effect of installing it. `run.sh` builds it explicitly, with a comment
saying why — *"Nothing rebuilds that on its own, so a change to the client
reaches the adapter only if somebody remembers — and the symptom is an adapter
running yesterday's client… It did, once."*

Somebody did not remember. The new test runs **before** `run.sh` in that job —
deliberately, so a broken decoder fails before a hundred conformance cases do —
which made it the first step in CI ever to need the build, and it had no reason
to know that.

It passed locally because this container had a `dist/` from an earlier `npm run
build` in the same session. That is the whole failure: a stale artifact made
the local run answer a question CI was going to ask differently.

## Alternatives rejected

**A `prepare` script on `clients/typescript`.** npm runs `prepare` when a
`file:` dependency is installed, so this would fix every consumer at once and
need no CI knowledge — genuinely the more robust option, and rejected for two
reasons. It would build the client on every `npm ci` anywhere in the repo,
including jobs that never touch it; and `run.sh` already establishes the
explicit-build convention with a comment explaining it, so a second, implicit
mechanism would mean two things doing one job and a reader having to know both.

**Move the decoder test after `run.sh --conformance`.** The build would already
have happened. It also gives up the ordering the test was placed for: a decoder
that reads the wrong column should fail in two seconds, not after a full
conformance run reports a hundred confusing failures.

**Have the test import from `src/` instead of the package.** It would then test
something no caller uses. The adapter imports the built package; so should its
test.

## Evidence

Reproduced before fixing, by deleting `clients/typescript/dist` and running the
adapter's `npm test` — the identical `TS2307`. Then the fix, verified the way
CI will run it: `dist` deleted, `npm run build` in `clients/typescript`,
`npm test` in the adapter. Six tests, six passing.

`yaml.safe_load` on the workflow, because a workflow file that does not parse
is a job that does not run.

## What this does not do

It does not stop the next step that needs the built client from being added
without the build. The `prepare` script would have; the explicit convention
relies on someone reading `run.sh`, and this entry is now the second place that
says so.

It does not address why the local run disagreed with CI. A stale `dist/` in a
working tree is indistinguishable from a fresh one, and nothing warns about it
— the general lesson is the one `CLAUDE.md` already draws about `ty` and about
clippy: a green local check is necessary and not sufficient, and this is a
third instance of the same shape.
