# The caveat that came true in an hour, because I did not open the file

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `.dockerignore`, `scripts/check_workspace.py`
- **Kind:** fix

## What changed

`.dockerignore` admits `examples/helpdesk/`, and
`scripts/check_workspace.py` reads `.dockerignore` as well as `Dockerfile`:
**every workspace member must be inside a path the Dockerfile copies *and*
inside a path `.dockerignore` admits.**

The verdict on
`ledger/2026-09-30-the-image-that-copies-part-of-the-tree.md`'s caveat *"It
does not read `.dockerignore`"* moves from `deliberate` to `closed`.

## Why

An hour ago that entry added a rule holding the Dockerfile's `COPY` list to
the workspace `members` list, and recorded a caveat saying the rule did not
read `.dockerignore`, with this reasoning:

> Nothing currently excludes such a directory, and a second parser is cost
> against a failure nobody has had.

**That was a claim about a file I had not opened.** `.dockerignore` is an
*allow-list*:

```
*

!Cargo.toml
!Cargo.lock
!crates/
!clients/python/testserver/
```

— and its own comment says why, at length: *"Everything is excluded and the
few things the build needs are allowed back, rather than the other way round:
a deny-list forgets the next large directory somebody adds, and the symptom
is a slow build rather than a failure, so nobody notices."* The reasoning I
wrote for not reading it is the reasoning that file exists to refute, and it
is written at the top of the file in plain English.

So the push that fixed the first half failed on the second, four minutes
later, with twenty-three of twenty-four jobs green:

```
ERROR: failed to solve: failed to compute cache key:
failed to calculate checksum of ref …: "/examples/helpdesk": not found
```

The `COPY` was there. The context was not.

**The lesson is not "add the second parser".** It is that a caveat's reason
has to be checked the same way a finding is. *"Nothing currently excludes
such a directory"* is a statement about the tree, testable by reading one
twenty-line file, and I wrote it in an entry arguing that a list maintained
by hand in a place where being wrong is silent needs a guard rather than a
comment. Two such lists, and I guarded one and reasoned about the other.

`CLAUDE.md` puts it as *"a finding must be demonstrated, not argued"*. The
same applies to the **absence** of one, and it is the easier half to get
wrong: nobody asks for a demonstration that a problem does not exist.

## Alternatives rejected

**Deleting the `*` and making `.dockerignore` a deny-list,** which removes
the whole class: every directory is in the context unless named, so a new
member needs no line here at all. Rejected because the file's own comment
already weighed and rejected it — a deny-list forgets the next large
directory, and the symptom is a slow build nobody notices. Trading a loud
failure for a silent cost is the wrong direction, and the loud failure now
has a guard.

**A full `.dockerignore` parser** — patterns, `**`, ordering, later lines
overriding earlier ones. Rejected because the file has exactly one shape:
`*`, then `!` lines, then three `**/…` exclusions of build output. A parser
for the general grammar would be a hundred lines defending against inputs
this repository does not write, and would be *less* safe: a subtle mismatch
between it and BuildKit passes silently, which is this failure again. What
is here reads the one shape and **refuses** anything else, with a message
saying what changed and what to do. A guard that cannot read its input says
so rather than passing.

**Moving `examples/helpdesk` out of `members`.** The third time this option
has come up today and the third rejection: a crate outside the workspace is
one `cargo test --workspace` never builds, which is how three separate
things rotted in the testserver
(`ledger/2026-09-14-the-testserver-joins-the-workspace.md`).

**Letting CI be the check.** It is a check — that is how both halves were
found — and it costs a full run and a push each time, and it fires after the
branch is public. The point of `scripts/check.sh` is the failures you can
find in seconds without a network, and both of these are static facts about
two files.

## Evidence

**The run that found it:** CI run 534 on `7d06967`, job `the container
image`. Twenty-three of twenty-four jobs green — including the new
`helpdesk` job, which built the runner in release and put the whole
application on SlateDB over an object store, so the first half of that
commit did work.

**Two mutations, both caught**
(`ledger/mutations/20260930T175903-dockerignore.json`):

| mutation | caught by |
|---|---|
| `!examples/helpdesk/` replaced with `!README.md` — the real mistake's shape | `check_workspace`: *".dockerignore admits no path containing examples/helpdesk"* |
| the bare `*` line deleted, turning it into a deny-list | `check_workspace`: *"no bare `*` line, so it is a deny-list and the rule cannot say what it admits"* |

The second is the never-fires half made loud: without it, converting the
file to a deny-list would leave a rule that reads a file it does not
understand and reports nothing.

**The guard, on the real tree:**

```
$ python3 scripts/check_workspace.py
14 crate(s), all members of the root workspace, and all 14 member(s) inside
a path the Dockerfile copies and the .dockerignore admits
```

**Not measured.** Nothing here is about speed. Whether admitting one more
directory slows the context upload was not timed; the directory is 70 KB
against a `crates/` tree already in the context.

## What this does not do

**It still reads one shape of `.dockerignore`.** `admitted()` understands
`*` plus `!` lines and nothing else — no patterns, no ordering rules, no
`**` in a re-inclusion. Any other shape is refused rather than
mis-parsed, so the failure mode is a person reading a message, but that
person then has to teach the parser rather than being helped by it.

**It checks that a member is admitted, not that it is admitted *whole*.**
`**/target/` and `**/node_modules/` still exclude subdirectories inside an
admitted member, which is correct for those two and would be wrong for a
member that needed a directory called `target`. Nothing has one, and the
guard would not notice if something did.

**Two rules that could have been one question.** "Is this member in the
image's build context" is the real question, and it is answered here by
checking two files independently and hoping their conjunction is what
BuildKit computes. The way to be sure is to run `docker build` — which is
the CI job that found both of these, and which `check.sh` cannot run.

**The correction is not a guard against the class it belongs to.** Nothing
stops the next caveat from stating an unchecked fact about the tree as its
reason. The tracker records the reason and cannot judge it; this entry is
the record that one was wrong, and the habit is the only mechanism.
