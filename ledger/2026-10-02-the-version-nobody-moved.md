# The version nobody moved, and two crates that would not have moved with it

- **Date:** 2026-10-02
- **Author:** Claude Code (session: cut a release)
- **Touches:** `Cargo.toml`, `Cargo.lock`, `crates/slate-serverd`,
  `crates/slate-wasm`, `clients/python/pyproject.toml`,
  `clients/python/testserver`, `clients/typescript`,
  `scripts/check_versions.py`, `scripts/test_check_versions.py`,
  `scripts/mutations.json`, `docs/releasing.md`
- **Kind:** process

## What changed

**The tree declares `0.1.0`.** It had declared `0.0.1` since the repository
was scaffolded, including through the one release that has been cut — `v0.0.1`
on 2026-09-14 — so every commit since September has been unreleased work
sitting behind a number that is already taken and that no registry will let
anybody reuse.

**Two of fourteen workspace members would not have come with it.**
`slate-wasm` and `clients/python/testserver` wrote `version = "0.0.1"` in their
own `[package]` instead of `version.workspace = true`. Both now inherit, and
`scripts/check_versions.py` refuses a member that does not.

**`docs/releasing.md` no longer says nothing has been published.**

## Why

The bump is the prerequisite for a release and nothing else; the interesting
part is what was found on the way to it.

`check_versions.py` was written on 2026-09-29 around a single failure: a tag
that says `v0.2.0` on a tree whose `package.json` says `0.0.1` publishes
`0.0.1`, successfully, to a registry that refuses a reused number. It holds
three declarations together — the workspace, npm, PyPI — and all three were
right. What it could not see is the layer underneath: a *member* crate's
`[package]`, which inherits the workspace number only if it says so.

That is the same shape as the `publish` default closed yesterday in
`ledger/2026-10-01-a-default-nobody-chose.md`, and the opposite of it. There,
silence meant the dangerous answer. Here, silence is the *safe* answer —
`version.workspace = true` is inheritance and a literal is the opt-out — so the
mistake takes a deliberate keystroke and then never announces itself again.
`slate-wasm` has carried its literal since the crate was created; nothing
noticed because nothing has bumped the workspace since the crate existed.

**Nothing would have broken on release day**, which is why it is worth a rule
rather than a note. Neither crate is published. What would have happened is
that `Cargo.lock` would record two numbers, `cargo metadata` would report two,
and the next person adding a crate would copy whichever manifest they happened
to open — a repository that has not decided, wearing the face of one that has.

## Why 0.1.0 and not 0.0.2

`v0.0.1` shipped two binaries and nothing else: the image, the Go module tag,
npm and PyPI were all added to `release.yml` two weeks *after* that tag, so
this will be the first release that even attempts them. Three SDKs, a SQL
front end, views, CTEs, windows, arrays, full-text search and the whole record
layer arrived in between. `0.0.2` would describe a patch. The release stays a
**prerelease** either way — `release.yml` marks it so unconditionally, and the
README's status banner is the argument for that.

## Alternatives rejected

**Tagging `v0.0.1` again.** It is what `README.md` names, so it would have
needed no bump at all. Impossible rather than unwise: the tag exists, the
GitHub release exists, and `softprops/action-gh-release` would have updated a
September release in place with October binaries — the one failure
`check_versions.py` exists to prevent, arriving through the door it does not
watch.

**Bumping only the three files `docs/releasing.md` lists.** That is what the
page said to do, and it is not enough: six `path = …, version = "0.0.1"`
requirements in the root manifest and a seventh in `crates/slate-serverd`
pin the path dependencies, and `cargo metadata` refuses the whole workspace
until they agree —

```
error: failed to select a version for the requirement `slate-server = "^0.0.1"`
candidate versions found which didn't match: 0.1.0
```

Loud, immediate, and nothing like the silent class this file is about. The
page now lists them, along with the two lockfiles.

**Putting the inheritance rule in `check_workspace.py`.** That is where the
`publish` rule went and the two are the same shape — a per-member manifest
line, anchored at the start of a line for the same reason. It went into
`check_versions.py` because that guard's whole subject is *one number
everywhere*, and a reader who finds a version disagreement will open it
first. The cost is a second `members`-list parser, which this repository has
counted as a hazard before
(`ledger/2026-09-21-two-of-three-lists-were-already-guarded.md`); it is
accepted because importing across `scripts/` would make either guard
unrunnable alone, and a member missing from one list fails the other guard.

**Matching `version.workspace` anywhere in the manifest** rather than at the
start of a line. Same argument as the `publish` rule, and the same
counter-example: a manifest's comments talk about versions.

**Leaving `clients/python/testserver` alone.** It is a test fixture, not a
shipped crate, and an argument exists for pinning a fixture's version. It is
not an argument anybody made — the literal is the Cargo default from the day
the crate was detached — and an exemption nobody chose is the thing this entry
is about.

## Evidence

**Five mutations, all caught.**

Two against the real tree
(`ledger/mutations/20261002T191357-clients-typescript-package-json.json`,
`ledger/mutations/20261002T191357-crates-slate-wasm-cargo-toml.json`):

| mutation | what refused |
|---|---|
| the npm package's version drifts from the workspace | `check_versions`: `package.json says 0.1.1` |
| `slate-wasm` restates its version instead of inheriting | `check_versions`: `writes its own version instead of` |

Three against the guard, scored by its own suite
(`ledger/mutations/20261002T191421-scripts-check-versions-py.json`):

| mutation | caught by |
|---|---|
| the inheritance rule never fires | `a member restating its own version fails` |
| the anchor is dropped, so any line starting `version` counts | `a member restating its own version fails` |
| the empty-members refusal is dropped | `a workspace with no members fails rather than passing on nothing` |

The second is the one that needed checking before it was believed: `^version`
really does match `version = "0.0.1"`, so the loosened guard *passes* a
restated member rather than behaving identically — a change, not one of the
`&x.clone()` lookalikes CLAUDE.md warns about.

**The suite is 12 cases, from 9**, and the two new ones need a member crate on
disk, so the fixture grew a `member=` knob rather than a fourth manifest.

**A stale anchor in `scripts/mutations.json`, found by the bump.** The npm
suite's case anchored on `"version": "0.0.1",`, which this change rewrites, so
the next roster run would have found the pattern zero times. `mutate.py`
refuses that rather than scoring it, which is the first of the six lies in its
docstring working as designed — the anchor moved under an edit and the tool
said so instead of passing against unmutated code.

**The regex is wider than `check_workspace.py`'s, deliberately.** That one
anchors the closing bracket of `members` at the start of a line, which fits
today's manifest and raises `has no members list to read` on
`members = ["a", "b"]` — a single line cargo accepts. Found by writing the
fixture, which is single-line; fixed by reading to the first `]` instead,
because a member name cannot contain one. `check_workspace.py` is left as it
is: changing a second guard's parser on the way past is how an unrelated job
goes red.

**Not measured.** Two regex passes over fourteen small files.

## What this does not do

**Nothing has been released at the time of writing.** This entry is the bump;
whether `release.yml` can actually publish five artefacts is unknown, because
four of its jobs have never run. `docs/releasing.md` says so, and the result
belongs in a later entry written from the run rather than from the plan.

**The rule does not check `[workspace.package]` inheritance of anything
else.** `edition`, `license`, `rust-version` and `repository` are all
inheritable and all unchecked; a member restating `license` is the same shape
and nobody would notice. Version is checked because it is the one a release
moves.

**It cannot see a member outside `members`.** `check_workspace.py`'s subject,
and a crate that is in neither list is invisible to both — which is what the
testserver's three rotted defects were about before it joined the workspace.

**`clients/go` still has no version to check.** The Go proxy reads one from a
`clients/go/vX.Y.Z` tag, which `release.yml` creates, and `check_versions.py`
has nothing to compare it against until after the release exists.
