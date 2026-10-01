# Two verdicts a guard had already answered, and the moment it does not cover

- **Date:** 2026-10-01
- **Author:** Claude Code (session: drive the live caveat backlog to zero)
- **Touches:** `.githooks/pre-commit`, `.githooks/test-pre-commit.sh`,
  `scripts/mutate_guard.py`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

Three things, found while working the backlog rather than while looking for
any of them.

**`scripts/check_build_output.py` had already answered two `narrowed`
caveats**, and nobody had connected it back. Both asked for a guard against
the next nested workspace's build output; that guard has existed since
2026-09-18, written for a different entry.

**The pre-commit hook refuses a path inside a build directory**, at any
depth. This is not a new capability — it is the same rule at an earlier
moment, which for this particular failure is the moment that matters.

**`scripts/mutate_guard.py` runs a `.sh` runner**, so the hook suite can be
mutation-tested at all. It could not be before.

## Why

The two caveats:

> Nothing stops the next nested workspace from doing the same thing. A
> `git check-ignore` assertion in the pre-commit hook, or a size guard,
> would — neither is here.
> — `ledger/2026-09-13-ignore-the-testservers-target.md`

> Nothing stops a *fourth* nested workspace from being created outside the
> root one — the demo's three backends already are. The size guard catches
> the symptom, not the cause.
> — `ledger/2026-09-14-guards-against-the-recurring-mistakes.md`

`check_build_output.py` runs two rules, and the second is the one these
asked for: *every package root's `node_modules` and `dist` are ignored*. Its
own docstring describes the failure one step earlier than the one that
happened — output merely *waiting* to be committed — which is the cause the
second caveat says the size guard misses. It was written for
`ledger/2026-09-18-untrack-the-benchmarks-build-output.md` and the two
earlier caveats were never re-read against it.

**That is the third time this has happened and the reason the whole-frame
re-read exists.** `ledger/2026-09-29-three-verdicts-that-were-wrong-not-three-gaps.md`
priced it: the cost of a stale verdict is somebody building a feature that
already exists. These two were `narrowed` rather than `open`, so the
re-reads that found the stale `open` ones never looked at them — which is
`ledger/2026-09-30-the-open-frame-read-against-the-tree.md`'s own caveat
about the 124 narrowed rows, arriving exactly where it said it would.

**And the hook is still worth adding, for one reason.** `check_build_output.py`
runs in `check.sh` and in CI, which is *after* the commit exists. The
accident it is about cost a rewritten history: `git add -A` swept 2,412 files
including a 102 MB binary, and the refusal arrived at `git push`. A guard
that fires after the commit does not save you from that; it tells you to
rewrite. The hook fires at the commit, which is the last moment the fix is
`git rm --cached`.

## The assertion the caveats asked for cannot be written

Both name `git check-ignore`, and it does not work: `check-ignore` answers
about paths git is *not* tracking, and a staged path is staged. Pointing it
at the staged list returns nothing, always, which is a guard that passes on
every input — the never-fires shape this repository keeps finding.

What makes the cause visible is the **shape of the path**. The root
`.gitignore` says `/target`, anchored, so `clients/python/testserver/target/`
matched nothing; every build-output directory this repository's toolchains
write has a name that means build output wherever it appears. So the rule is
the name at any depth, which is a rule about a convention rather than about
one workspace — and that is what catches the *next* one.

## Alternatives rejected

**Closing the two caveats and leaving the hook alone.** The guard answers
them, so this would have been defensible and it is what the tracker would
have recorded. Rejected because reading the two entries makes the moment
explicit: both describe a failure whose whole cost was that the commit
already existed. Closing them against a check that runs later would be
answering a different question with the right words.

**A blanket `**/node_modules/` in `.gitignore`.** The one-line version, and
`.gitignore`'s own comment already rejected it: it would hide a
`node_modules` somebody genuinely meant to vendor. `check_build_output.py`'s
docstring says the per-directory style is kept deliberately and the guard is
the enforcement that choice was missing. The hook inherits that reasoning
and inherits the override (`--no-verify`, and the message says so).

**Unindenting the hook suite's `FAIL` lines** so `mutate.py`'s `python`
dialect could read them. One character per line, and rejected: the two-space
indent matches `check.sh`'s output on purpose, so the suite reads the same
whether run alone or as one of ninety-three. The adapter exists for exactly
this — `mutate_guard.py`'s docstring says the guards "agree on an exit code
and on nothing else" — and teaching it one more runner is the shape that
already works.

**Matching `target` without the trailing slash.** Simpler, and it refuses
`src/targeting.rs`. There is a test for that now, because the first thing a
guard like this must not do is refuse ordinary source.

## Evidence

**The guard that already answered them, on the real tree:**

```
$ python3 scripts/check_build_output.py
ok    no tracked file lives under a build directory
ok    every package root has its node_modules and dist ignored
no build output is tracked, and all 5 package roots have their output ignored
```

**Three new hook cases**, and the third is the control:

| case | outcome |
|---|---|
| `nested/target/debug/thing`, under the size limit | refused |
| `pkg/node_modules/dep/index.js` | refused |
| `src/targets/mod.rs` and `src/targeting.rs` | allowed |

**Three mutations, all caught**
(`ledger/mutations/20261001T023657-githooks-pre-commit.json`): the rule never
fires; the rule anchored to the root like the `.gitignore` it replaces; the
trailing slash dropped, which makes `targeting.rs` build output.

**And the first attempt at those three scored `UNREADABLE`**, which is the
third finding. `mutate.py`'s `python` dialect anchors `FAIL` at column zero
and the hook suite prints `  FAIL  …`, so a mutation of the hook could not be
scored at all — the same class as
`ledger/2026-09-29-a-roster-of-test-names-is-weaker-and-worth-having.md`'s
three checks run by hand. `mutate_guard.py` now runs a `.sh` under `sh` and
re-emits the verdict at column zero, which is what the adapter is for.

**Not measured.** The hook adds one `grep -E` over the staged list, which is
already walked twice.

## What this does not do

**`BUILD_DIRS` is a roster, in two places now.** The hook's list and
`check_build_output.py`'s list are the same names written twice and nothing
holds them together, which is this repository's most-met failure. They are
in different languages — `sh` and Python — and the hook deliberately has no
dependencies, so sharing them means the hook reading a Python file or a third
file both parse. Recorded rather than solved; the cost of drift is one of the
two catching something the other does not, which is a weaker failure than
either catching nothing.

**The hook can be bypassed and the message says so.** That is the same trade
the ledger rule makes, for the same reason: a check with no escape hatch gets
switched off the first time it blocks a recovery.

**It does not read `.gitignore` at all.** A directory that is correctly
ignored and force-added with `git add -f` is refused by name rather than by
the ignore rule, which is right for the five names and wrong for a sixth
nobody has written down — the roster failure above, stated for this half.

**The stale-verdict class is not fixed by this.** Two `narrowed` rows were
answered by a guard for eighteen days and nothing noticed, and nothing here
would notice the next one. The whole-frame re-read is the mechanism and it
has only ever been run over the `open` frame; the `narrowed` one is being
read now, which is how these two were found, and that is a session rather
than a check.
