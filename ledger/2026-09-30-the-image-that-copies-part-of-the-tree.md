# The image copies part of the tree, and cargo reads all of it

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `Dockerfile`, `scripts/check_workspace.py`
- **Kind:** fix

## What changed

`Dockerfile` copies `examples/helpdesk` into the build stage, and
`scripts/check_workspace.py` gained a third rule: **every workspace member
must be inside a path the Dockerfile copies.**

## Why

Adding `examples/helpdesk` to the root `members` list turned the
`container image` job red on run 532, with twenty-two of twenty-three jobs
green:

```
error: failed to load manifest for workspace member `/src/examples/helpdesk`
referenced by workspace at `/src/Cargo.toml`
Caused by:
  failed to read `/src/examples/helpdesk/Cargo.toml`
  No such file or directory (os error 2)
```

The image's build stage copies three things — `Cargo.toml Cargo.lock`,
`crates`, and `clients/python/testserver` — and then runs
`cargo build -p slate-serverd`. Cargo resolves the **whole** workspace before
it builds one package, so a member the image does not copy stops the build
even though the built binary does not depend on it.

That list is the shape this repository keeps meeting: a roster maintained by
hand, in a place where being wrong is silent. Nothing local reproduces it,
because every other command in this tree runs in a complete checkout —
`cargo build`, `cargo clippy --workspace`, `scripts/check.sh` and the twenty
other jobs all had the file. The one job that builds from a partial copy is
the one that broke, and it broke on a change that touched nothing it
contains.

`CLAUDE.md` already records the general form: *"three lists this repository
maintains by hand"*, and the answer each time was a guard rather than a
comment. `check_workspace.py` is where it belongs because it already reads
`members` and already exists to catch a crate in the wrong relationship to
the workspace — this is the same question asked of a different consumer.

## Alternatives rejected

**Taking `examples/helpdesk` back out of `members`,** which would have fixed
the job in one line. Rejected because the reason it is a member is the
reason the testserver was made one: a crate outside the workspace is one
`cargo test --workspace` never builds, and the testserver had three
separate things rot in it while it was detached —
`ledger/2026-09-14-the-testserver-joins-the-workspace.md`.
`check_workspace.py`'s own docstring says so. Trading a
red job for a crate nobody builds is the wrong direction.

**`COPY . .` in the Dockerfile.** One line, and no list to maintain. Rejected
because it destroys the layer cache the file is arranged around: the
manifests are copied first precisely so a source-only change does not
re-resolve the dependency graph, and copying the whole tree — `site/`,
`clients/`, `ledger/`, every `node_modules` the `.dockerignore` does not
catch — makes every commit a full rebuild. The comment above the `COPY`
lines says what they are for.

**A `.dockerignore`-driven copy of everything cargo needs.** The same idea
with the list inverted, and inverted is worse: an exclusion list that is
wrong fails *open* — it copies too much and the build still works, slowly —
so nothing ever tells you. An inclusion list that is wrong fails closed and
loudly, which is why the guard can check it at all.

**Leaving it as a comment in the Dockerfile.** The comment is there too, and
it is not enough: a comment is read by somebody editing the Dockerfile, and
the person who breaks this is editing `Cargo.toml`. The guard fires on the
half where the mistake is made.

**A rule that parses `.dockerignore` as well.** The rule as written asks only
whether a `COPY` names the member or one of its parents, and a member inside
a copied directory that `.dockerignore` excludes would pass here and fail in
CI. Not written because nothing in `.dockerignore` currently excludes a
directory holding a member, and a second parser for a second file is cost
against a failure nobody has had. Recorded below as what this does not do.

## Evidence

**The run that found it:** CI run 532 on `d87d9f6`, job `the container image`,
`buildx failed with: … exit code: 101`, the manifest error quoted above. The
other twenty-two jobs on that commit were green, which is what makes this
worth a guard rather than a fix.

**The guard, before and after,** on the real tree:

```
$ python3 scripts/check_workspace.py
14 crate(s), all members of the root workspace, and all 14 member(s)
inside a path the Dockerfile copies
```

**A mutation, caught** (`ledger/mutations/20260930T174058-dockerfile.json`):
the `COPY examples/helpdesk examples/helpdesk` line replaced with a copy of
`README.md`, which is the shape the real mistake had — a Dockerfile that
copies *something* and not the member. `check_workspace` refused, and the
run restored clean.

**And the first attempt at that mutation scored SURVIVED, wrongly**, which
is worth recording because it is `scripts/mutate.py`'s own documented hazard
met while using it. The command was an inline adapter —
`subprocess.run([...check_workspace.py]); print('0 passed, 1 failed')` — and
the `python` dialect reads *two* patterns: a report line, and `^FAIL\s+name`
for each failure. The adapter printed the report and no `FAIL` line, so the
run saw "a suite reported, nothing failed" and scored a survivor. The guard
had refused correctly the whole time. `scripts/mutate_guard.py` is the
wrapper that exists for exactly this and it is what the roster uses; using
it instead scored the same mutation `caught` immediately. A hand-rolled
harness that lies is the first failure mode in `mutate.py`'s docstring, and
the lie costs a test that was never missing.

## What this does not do

**It does not read `.dockerignore`.** A member inside a copied directory but
excluded by an ignore rule would pass this and fail in CI, exactly as before.
Nothing currently excludes such a directory, and a second parser is cost
against a failure nobody has had.

**It checks one Dockerfile, named by path.** A second image, or this one
renamed, is a rule that silently checks nothing — so the absence of
`Dockerfile` is itself a refusal, with a message saying which of the two
things happened. That is the never-fires half, and it has no fixture test:
`check_workspace.py` is the one guard in `scripts/` with no
`test_check_workspace.py`, which predates this change and is not fixed by it.

**It says nothing about what the image copies that it need not.** The rule is
one-sided — every member is copied — and a `COPY` of a directory no build
reads would pass. That is a slow image rather than a broken one, and the
guard is about the failure that is silent.
