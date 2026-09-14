# A trunk called main, and the documentation that assumed there wasn't one

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `CLAUDE.md`, `README.md`, `ledger/README.md`,
  `.github/workflows/pages.yml`, and the `main` branch
- **Kind:** repair

## What changed

`main` now exists, at the same commit as the working branch. The repository had
**exactly one branch**, and it was the feature branch — which was therefore
also the default, and therefore what the public repository page showed.

`CLAUDE.md` gained a section on what runs and where, because it had no mention
of CI at all. `README.md`'s status banner said the Go and TypeScript SDKs "are
not built", which stopped being true some time ago. `pages.yml` lost a `paths`
filter that would have meant the site never published.

## Why

The instruction files are how the next agent decides what to do, and they were
describing a repository that no longer exists: no CI, no Go or TypeScript
client, no trunk. Every one of those omissions pushes work in a wrong
direction — an agent that does not know CI runs will not think about what CI
needs, and the whole point of the last two days is that a check nobody runs is
a check nobody has debugged.

## What the sweep found

**The README's status banner was flatly wrong on the front page of a public
repository.** "The Go and TypeScript SDKs are not built." Both are built,
tested against a real daemon, in CI, and required to agree with Python. The
banner has been wrong before in exactly this way — `PROTOCOL-FINDINGS.md`
records the last time, when it claimed the head node was not built while the
head node sat in the crate table sixteen lines below.

**`pages.yml` would not have deployed the site.** Its trigger was `push:
branches: [main], paths: ["site/**", ...]`. Creating `main` is a push to
`main`, and it did not fire — a branch-creation push has no diff for a `paths`
filter to match. So the site would have waited for a future commit that
happened to touch `site/`, and if none came it would simply never have
published, silently. The filter is gone: three static files cost seconds to
deploy and the deploy is idempotent.

That is the third `paths`-filter trap in two days, after the one that would
have made a tagged release publish nothing. All three are now written up where
somebody editing a trigger will see them.

**`slate-testserver` was missing from the crate table**, having become a
workspace member.

**The Status list's defect count was stale** — eight when the real number
reached eleven.

Once the filter was gone the workflow fired, and failed — `Get Pages site
failed ... Not Found`, because Pages was not enabled for the repository. The
action takes `enablement: true`, which turns it on rather than failing, and
that is now set. Leaving it at the default meant the deploy was permanently one
Settings visit away from working, with a bare `Not Found` as the only clue.

## Alternatives rejected

**Annotating the closed caveats across all thirty-three ledger entries.**
Tempting, and wrong: `ledger/README.md` says in as many words that the ledger
is "not a place for status — nothing here should need updating as work
proceeds". Thirty entries needing a footnote every time something lands is
exactly the failure that rule exists to prevent, and the later entry describing
a fix is already the record that it was fixed.

But this repository *also* demands that a false claim be withdrawn, and today
involved doing both. So `ledger/README.md` now distinguishes them: withdraw
what was wrong when written, leave what merely went out of date. Two entries
carry corrections of the first kind — a client limitation the server never had,
and "the repository has no CI" when the workflow existed and had never run.

**Rewriting the old entries' prose instead of striking it through.** Deleting a
wrong sentence loses the more useful half: that the reasoning was made, and
where it went wrong. Struck through and answered in place.

**Making `main` the default branch from here.** Not possible, and not for want
of trying: `PATCH /repos/{owner}/{repo}` returns `403 Repository settings
writes are not permitted through this proxy`. Branch *creation* is permitted —
which is why `main` exists — but the default-branch setting, like enabling
Pages, is the repository owner's to change.

**Opening a pull request to merge the branch into `main`.** Unnecessary:
`main` was created *from* the branch, so the two are identical and there is
nothing to merge. A pull request would have been an empty one.

## Evidence

`main` and `claude/rust-orm-record-layer-gswxlu` both at `7aa2529`, confirmed
through the branches API. CI ran on `main` on creation and is green there, so
the trunk is not merely a pointer — it has been checked.

`.githooks/test-pre-commit.sh`: 14 passed. The three workflow files parse, and
`pages.yml`'s trigger is now `{push: {branches: [main]}, workflow_dispatch:}`
with no path filter.

## What this does not do

Whether Pages actually enables itself is not yet known at the time of writing —
`enablement: true` needs the workflow token to be permitted to change that
setting, and repository *settings* writes are exactly what was refused for the
default branch. If it is refused here too, the failure names the setting and
somebody has to visit Settings → Pages once.

`main` is not yet the **default** branch, and this session cannot make it one.
Until somebody changes it in Settings → General, the public repository page
still opens on the feature branch. Everything else — CI on every push, Pages
from `main`, releases from a tag — already behaves as though `main` is the
trunk.

The two branches will drift the moment anything is pushed to one and not the
other. Nothing here keeps them in step; this commit makes `main` correct once,
not continuously.

The sweep covered the files that describe *current state*: `CLAUDE.md`, the
seven READMEs, and the workflow comments. It deliberately did not re-audit
`docs/correctness.md`, `docs/performance.md` or `docs/topology.md` line by
line — they are argument and measurement rather than status, and nothing in the
last two days contradicted a number in them. That is a judgement, not a check,
and it is worth saying which one it was.
