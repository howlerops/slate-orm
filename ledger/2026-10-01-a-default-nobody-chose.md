# A default nobody chose, and a roster that could only hold one suite per guard

- **Date:** 2026-10-01
- **Author:** Claude Code (session: finish the backlog, the docs and the examples)
- **Touches:** `scripts/check_workspace.py`, `scripts/mutations.json`,
  `scripts/check_mutations_roster.py`
- **Kind:** fix

## What changed

**`check_workspace.py` refuses a workspace member whose manifest does not say
whether it publishes.** `publish` defaults to **true**, so a crate that says
nothing is one `cargo publish` would push to crates.io. All fourteen members
say `publish = false` today; the rule is for the fifteenth.

**`check_mutations_roster.py` keys its suites by `(guard, file)`** rather than
by guard. A guard that watches two trees needs a suite per tree, because
`mutate.py` takes one `file` per spec, and `check_workspace.py` is the first:
its member-list rule mutates the root manifest and its publish rule mutates a
member's.

## Why

An open caveat from
`ledger/2026-09-29-everything-this-repository-ships-can-now-be-published.md`:

> **Nothing stops the *next* crate defaulting to publishable.** A fourteenth
> member added later defaults to publishable, and nothing checks
> `scripts/check_workspace.py`'s member list for the declaration.

The asymmetry is what makes this worth a rule. Most defaults in this
repository fail loudly when they are wrong — a missing member is a crate
`cargo test --workspace` does not build, and the next thing to rot in it says
so. This one fails by **succeeding**: a crate appears on crates.io under this
project's name, and the only signal is somebody noticing. crates.io does not
let you take a version back.

So the rule is only that the manifest *states* it. `publish = true` passes,
because that is a decision somebody made. Silence does not, because it is a
default somebody inherited — which is the distinction
`ledger/2026-09-13-defects-found-by-review.md` drew about `TCP_NODELAY`, and
the one `GOTOOLCHAIN=auto` taught again three weeks later — a setting that had
been quietly downloading a newer compiler for a fortnight because nobody had
written down which value they wanted.

## The roster rule the second suite tripped

Adding the mutation turned `check_mutations_roster.py` red:

```
two suites name the same guard. One of them is being run and the
other is being ignored, and which is an accident of file order.
```

**Half of that sentence was wrong, and the half that was right was about this
file.** `run_mutations.py` filters `s["guard"] in wanted` over a *list*, so it
runs both suites perfectly well. What was ignoring the second one was
`check_mutations_roster.py` itself, whose first line is
`{suite["guard"]: suite for suite in roster["suites"]}` — a dict that drops
the earlier of two entries sharing a key. The guard was describing its own
blind spot and attributing it to the runner.

Keyed by `(guard, file)`, the collision rule still has teeth and now means
what it says: two suites naming one guard *and* one file are two names for one
thing. Two naming one guard and two files are a guard that watches two trees.

## Alternatives rejected

**Refusing `publish = true` outright**, so that no member can ever be
publishable. Shorter, and it would encode `docs/releasing.md`'s current
answer. Rejected because that answer is about *today's* crates — "libraries
this repository consumes by path, committing to an API that changes weekly" —
and a rule that cannot be satisfied by the decision it is about stops being a
check and becomes a policy somebody will delete the first time they want to
publish something. The check is on the *statement*, which is the thing that
cannot be wrong.

**Reading the manifests with `tomllib`.** Correct, and it would also accept
`publish` under a table where it means nothing. Rejected because the rest of
this file reads manifests with anchored regexes on purpose: what the rule
wants is the line a reader checking by eye would look for, and `^publish\s*=`
is exactly that line.

**Matching `publish` anywhere in the file rather than at the start of a
line.** One character shorter and demonstrably wrong — see Evidence. Every
member's manifest carries a comment saying *"publishing them commits to an API
that changes weekly"*, so an unanchored pattern passes on a manifest that has
the explanation and not the setting, which is the exact shape a careless edit
produces.

**Putting the new case in the existing `check_workspace` suite.** The obvious
move, and `mutate.py` cannot do it: a spec has one `file`, and this case
mutates a member's manifest while the existing case mutates the root's. The
choice was then between changing the roster's key and leaving this rule
unmutated with a note; the key is one line and the note would have been a
caveat that outlives it.

**Leaving the roster's collision message as it was.** It would still have
fired on the real collision. Rejected because the sentence blames
`run_mutations.py` for something `run_mutations.py` does not do, and a reader
hitting it would go and read the wrong file — which is the
names-what-it-found-not-what-the-reader-wanted class
`ledger/2026-10-01-the-other-messages-that-name-what-they-found.md` spent a
session on.

## Evidence

**The rule found nothing**, which is the honest first result: all fourteen
members already declare `publish = false`. Reported as such rather than
dressed up.

**One tree mutation, caught**
(`ledger/mutations/20261001T153824-crates-slate-derive-cargo-toml.json`):
deleting `publish = false` from `crates/slate-derive/Cargo.toml` while keeping
the comment above it that explains why it is there. `check_workspace.py`
refuses.

**Two guard mutations, both survivors, and both were my error**
(`ledger/mutations/20261001T153802-scripts-check-workspace-py.json`).
Disabling the rule, and loosening its anchor, both survived — because
`mutate_guard.py` runs the guard against the **real tree**, where the rule
passes either way. A rule that is right about today's tree cannot be broken by
breaking the rule; it is broken by breaking the tree. That is the whole reason
`scripts/mutations.json` exists and
`ledger/2026-09-29-mutating-the-real-tree-found-a-class-nothing-checked.md`
is the entry about it, and I ran the wrong kind of mutation first anyway.

**The anchor is load-bearing, demonstrated rather than argued.** With the
pattern loosened to `re.search(r"publish", text)` *and* the setting deleted
from `crates/slate-derive/Cargo.toml`, leaving its comment:

```
$ python3 scripts/check_workspace.py
14 crate(s), all members of the root workspace, … and each saying whether it publishes
$ echo $?
0
```

The guard passes on a manifest that explains why it does not publish and does
not say so. Two files had to move at once, which is why `mutate.py` could not
score it and it was run by hand with the restore outside the command.

**Two mutations of the re-keyed roster rule, both caught**
(`ledger/mutations/20261001T153956-scripts-mutations-json.json`): pointing the
new suite at the same file as the old one, which is a real collision and is
refused by name; and emptying the new suite's `why`, which the roster's own
rule catches.

**And one that scored `NOTHING RAN`**
(`ledger/mutations/20261001T153944-scripts-check-mutations-roster-py.json`):
reverting the key to the guard alone makes the `covered` comprehension unpack
a string into two names and the guard raises, which `mutate_guard.py` reports
as no verdict rather than as a refusal. Correct behaviour — a guard that
crashed is not a guard that passed — and it means the re-keying's own defence
is the two roster mutations above rather than a direct one.

**Not measured.** Two regex searches over fourteen small files.

## What this does not do

**It does not read the root manifest's `[workspace.package]`.** A `publish`
set there would apply to members that say `publish.workspace = true`, and the
rule would refuse such a member for not declaring it. Nothing in the tree
writes one — `Cargo.toml`'s `[workspace.package]` holds version, edition,
rust-version, license, repository and authors, and
`git grep -n 'publish' -- '*/Cargo.toml' Cargo.toml` finds the setting only as
`publish = false` in each member — and the refusal would at least be loud and
wrong rather than quiet and wrong. It is a false positive waiting for a
legitimate pattern, and the fix when it arrives is one more alternative in the
regex.

**It says nothing about whether `publish = false` is the right answer.**
`docs/releasing.md` argues that, the rule only insists somebody argued it. A
member that says `publish = true` because a careless edit changed `false` to
`true` passes here and is caught by nothing.

**The roster still runs one mutation per suite and one suite per file.** A
guard watching three trees needs three suites, which is now possible and is
still entries somebody maintains by hand, which is the class
`ledger/2026-09-21-two-of-three-lists-were-already-guarded.md` counted and
`ledger/2026-09-21-a-tools-docstring-is-not-a-measurement-of-the-tool.md`
priced.
