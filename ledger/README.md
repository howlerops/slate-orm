# The ledger

One file per change, explaining **what changed and why** — the reasoning that
does not survive in a diff.

A commit that touches anything outside `ledger/` needs an entry. The
`pre-commit` hook enforces it; see [Enforcement](#enforcement).

## `mutations/`

`scripts/mutate.py` writes one JSON file per run here, automatically. They are
not entries and the pre-commit hook does not read them as entries — its pattern
is `ledger/<date>-*.md`, anchored at this directory — but they are inside
`ledger/`, so a commit carrying only records still counts as ledger-only.

Each holds the command, the dialect, the commit, and every case's verdict with
the tests that named it. **Runs that could not score are recorded too**, which
is the point: #285 found that `cargo test -q` suppresses the lines the `rust`
dialect matches, so every mutation scored as a survivor — and then could not
answer which earlier runs that had ruined, because nothing kept them. The
entry written at the time guessed, wrongly, and had to be corrected.

One file per run rather than an appended log, for the same reason entries are
one file each: several agents work here at once and a shared file conflicts on
every commit.

## Why this exists

A diff says what a line became. It does not say what the alternatives were,
which one was measured, what was tried and abandoned, or which invariant the
change was protecting. That reasoning is the expensive part, it is the part
that decays first, and by the time somebody needs it — usually while breaking
the thing on purpose — whoever had it has moved on.

This repository is also worked on by several agents at once, so "ask the person
who wrote it" is not available even in principle.

## One file per entry, and why not one file

`ledger/YYYY-MM-DD-slug.md`. Never a single shared log.

Two workers appending to one `LEDGER.md` conflict on every change, and a
sequential counter (`0001-`, `0002-`) conflicts the same way the moment two
entries are written in the same hour. A dated slug collides only if two people
name the same change the same thing on the same day, which is a real signal
rather than a merge problem.

Sorting is chronological by filename, which is what you want when the question
is "when did this start".

## Writing one

Copy [`TEMPLATE.md`](TEMPLATE.md). The sections are not decoration:

- **What changed** — one paragraph. The diff has the detail.
- **Why** — the actual reason. Not "to improve X"; what was wrong, or what
  became possible.
- **Alternatives rejected** — the most valuable section, and the one people
  skip. What else would have worked, and what it would have cost. If there was
  genuinely only one option, say so and say why.
- **Evidence** — the measurement, the failing test, the mutation that was
  caught. "It seemed faster" is not evidence. If the change is not the sort of
  thing that has evidence (a rename, a doc fix), write `n/a` and move on.
- **What this does not do** — the honest limits. Where the fix stops.

Entries are append-only in spirit: correct one by adding a new entry that
supersedes it and linking back, rather than rewriting history that somebody may
have already read and acted on.

## Enforcement

`.githooks/pre-commit` refuses a commit that changes anything outside `ledger/`
without adding or editing an entry. It is wired up by

```sh
git config core.hooksPath .githooks
```

which the session-start hook does automatically; run it by hand after a fresh
clone if you are not going through that.

Exempt, because requiring an entry would be noise or would block a recovery:
merge commits, reverts, and commits that touch only the ledger.

`git commit --no-verify` bypasses it. That is deliberate — a check with no
escape hatch gets switched off permanently the first time it blocks something
urgent — but a bypassed commit is a commit whose reasoning is now nowhere, so
write the entry afterwards.

The hook itself has a test suite, `.githooks/test-pre-commit.sh`, and CI runs
it on every push. It is there because the hook's own bugs are invisible from
the outside: the first one refused *every* commit in the repository while
printing nothing, and the second reported an entry as having a `## Why` section
with nothing under it when the section was four sentences long. A check that is
wrong is worse than one that is missing, because it is trusted.

Note what CI does **not** do: it does not enforce the ledger. Nothing on the
server side rejects a push whose commits have no entries — the hook is local,
and `core.hooksPath` has to be set for it to run at all. The ledger holds
because the people and agents working here think it is worth holding, not
because it is impossible to skip.

## Reclaiming disk

Not ledger business, but it is the note everyone needs and this is where
`CLAUDE.md` points. Several concurrent builds fill `target/` fast, and the
first symptom is never "disk full" — it is a linker `Bus error`, an
`rustc-LLVM ERROR: IO failure`, or a burst of `E0463: can't find crate` that
looks exactly like broken code.

```sh
python3 scripts/reclaim.py --dry-run   # what would go
python3 scripts/reclaim.py             # go
```

It keeps the newest build of each target, drops the superseded copies, and
removes `incremental/` and `examples/`. It reports both what it freed and what
the filesystem now says is free, because those two differ when another build is
writing — and the filesystem's number is the one that decides whether the next
link succeeds.

This was a snippet to paste into `python3 -c` until #286. Two things it could
not do. It `cd`s into `target/debug/deps` and reports `freed 0.00 GB` from
wherever it lands, which is indistinguishable from an already-clean tree; that
happened twice in one session. And nothing tested it — it keys on filename
**and extension**, where an earlier version keyed on the stem and skipped any
name with a dot in it, so it dedupped the test binaries and left every `.rlib`
and `.rmeta`, which is most of what is on disk. On a tree that version had just
"cleaned", regrouping freed **6.5 GB** more. That is a case in
`scripts/test_reclaim.py` now rather than a warning paragraph here.

**Do not delete `target/debug/build`** — it holds build-script outputs, and
removing it produces hundreds of convincing, fictional compile errors in
dependencies that were fine. If that has already happened,
`cargo clean -p <the-named-crates>` repairs it.

## What this is not

Not a changelog: no audience but the next person working here. Not a substitute
for a commit message: the message says what this commit does, the entry says
why the change exists at all and what it cost to decide. Not a place for status
— nothing here should need updating as work proceeds.

### Editing an entry after the fact: two different things

That last line gets read as "never touch an old entry", and it is not quite
that. Two cases pull in opposite directions, and the difference is whether the
entry was *wrong when written*.

**Withdraw what was false.** An entry claiming a gap that was never there, or a
measurement that does not reproduce, is misinformation with a date on it. The
standard in `CLAUDE.md` is explicit — withdraw the hypothesis and say that you
did — so strike the sentence through, leave it legible, and put the correction
beside it. Deleting it loses the more useful half: that the reasoning was made,
and where it went wrong. Two entries carry corrections like this, one for a
client limitation that the server never had and one for "the repository has no
CI" when the repository had a workflow that had simply never run.

**Leave what merely went out of date.** A "What this does not do" section
describing an honest gap, closed by later work, is not wrong — it was true, and
the entry is dated. Annotating every such section as work proceeds is exactly
the status-tracking this file says the ledger is not, and it scales terribly:
thirty entries all needing a footnote every time something lands. The later
entry describing the fix is the record that it was fixed.

A cross-reference on the way past is fine when it is one line and you are
already editing the file. Going looking for them is not.
