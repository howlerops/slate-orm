# CI's ruff is newer than mine, and now that is written down

- **Date:** 2026-09-20
- **Author:** Claude, after a green local `ruff check` turned the scripts job red
- **Touches:** `scripts/test_check_handlers.py`, `CLAUDE.md`
- **Kind:** fix

## What changed

Two type annotations reordered, from `str | None | dict[str, str]` to
`str | dict[str, str] | None`, and a paragraph in `CLAUDE.md` saying that CI's
`ruff` is newer than this container's — which it already said about `clippy`
and `rustfmt` and not about `ruff`.

## Why

`3e8dc7d` went red on one job. `RUF036`, "`None` not at the end of the type
union", twice in a file I had run `ruff check .` over before pushing. `ruff
0.15.8` here does not raise it; CI's does.

`CLAUDE.md` has two careful paragraphs about exactly this shape — "CI's clippy
is newer than yours, and `-D warnings` makes that fatal", and the same for
`rustfmt`, each with the commits it cost. `ruff` was not among them, so the
next person meets it as a surprise rather than as a documented cost. It is one
paragraph and it is the cheapest thing in this commit.

## Alternatives rejected

**Pin `ruff` in CI to this container's version.** It would make local and
remote agree, at the price of freezing the lint set to whatever this image
happens to carry — which is the opposite of what the other two toolchain notes
conclude. They say to treat a green local run as necessary and not sufficient,
and to read the fix out of the job's log. The gap is the cost of tracking the
current release, and the repository has twice decided that is the right trade.

**Install a second `ruff` to check against.** The clippy note already answers
this: usually not possible here, and the disk note says why. `ruff` is a single
binary and would be cheaper than a Rust toolchain — but it would need pinning
to whatever CI resolves *today*, which is the same moving target one step
removed.

**Fix the two lines and say nothing.** What a two-character diff invites. It
would leave the next occurrence as expensive as this one, on a repository whose
`CLAUDE.md` exists mostly to stop that.

**Add `RUF036` to the local configuration so it fires here.** Fixes this lint
and none of the others CI's newer version knows about. The general problem is
the version, not the rule.

## Evidence

The job's own output, which is the whole finding:

```
RUF036 [*] `None` not at the end of the type union.
  --> scripts/test_check_handlers.py:46:24
  --> scripts/test_check_handlers.py:171:15
Found 2 errors.
```

Locally, before the fix:

```
$ ruff --version
ruff 0.15.8
$ ruff check scripts/test_check_handlers.py
All checks passed!
```

Both statements are true at the same time, which is the point. After the
reorder, `scripts/check.sh` 25/25 — all four `ruff` and `ty` runs included.

## What this does not do

**It does not close the gap, and says so.** There is no local command that
finds the next lint CI's `ruff` knows and this one does not. The note tells you
what to do when it happens, in the same terms the clippy note uses.

**It does not check `ty` for the same hazard.** `ty` is versioned with the
client's dev dependencies, so it is pinned where `ruff` is not — I believe that
from reading `pyproject.toml`, not from seeing the two versions disagree.

**Two occurrences, one rule.** I did not sweep the tree for other annotations
that put `None` first; `ruff --fix` with a newer version would, and running one
is the thing that is not possible here.
