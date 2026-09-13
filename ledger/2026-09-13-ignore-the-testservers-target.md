# Ignore the Python test server's build output

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `.gitignore`
- **Kind:** repair

## What changed

`.gitignore` now covers `clients/python/testserver/target/`, and the 2,412
files already committed there were rewritten out of the unpushed history.

## Why

`git add -A` swept the test server's build output into a commit, and the push
was rejected: one of its binaries is 102 MB against GitHub's 100 MB limit. The
root `.gitignore` says `/target`, which is anchored to the repository root and
does not match a nested one.

This is the third consequence today of the same structural fact — the test
server is a separate cargo workspace, so the root's arrangements do not reach
it. The first two were a crate that stopped compiling and a catalog that stopped
starting, both unnoticed because `cargo test --workspace` never builds it.

## Alternatives rejected

**`git rm -r --cached` in a new commit.** Does not help: GitHub rejects the push
based on the objects in it, and the blobs would still be in the history being
pushed. The commit had to be rewritten. It was unpushed, so rewriting cost
nothing — had it been pushed, this would have been a much worse morning.

**A bare `target/` rule.** Would match any directory named `target` anywhere,
including one a future table or fixture might legitimately be called. Naming the
path says what is actually meant.

**Moving the test server into the root workspace**, which would fix this and the
other two at once. Still the right answer and still not done here: it puts a
client's fixture into every root build, and this commit is a one-line ignore. It
stays in the earlier entry's "what this does not do".

## Evidence

`git log --stat` over the rewritten range reports zero files under
`testserver/target/`, where the original range had 2,412 in one commit. The push
that was rejected now succeeds.

## What this does not do

Nothing stops the next nested workspace from doing the same thing. A
`git check-ignore` assertion in the pre-commit hook, or a size guard, would —
neither is here.
