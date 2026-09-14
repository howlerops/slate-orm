# The two settings a workflow could not reach

- **Date:** 2026-09-14
- **Author:** Claude (Opus 5)
- **Touches:** `README.md`, `site/README.md`
- **Kind:** docs

## What changed

`main` is the default branch and GitHub Pages is enabled — both done by the
repository owner, because both were refused to everything else that tried.

So the documentation that described them as pending is now wrong, and the
README gains the two things that were not true until this moment: a CI badge
that is not aspirational, and a link to a site that actually exists.

## Why

This is the same rule the last four commits have been enforcing on everything
else: a document that describes a state the repository is no longer in gets
read as current and sends the next person somewhere wrong. "GitHub Pages, until
somebody turns it on" was an accurate line in a *Not built* list this morning
and is a false one now.

A badge is worth adding only because it is now honest. `ci.yml` existed for
months with zero runs; a green badge over a workflow that has never fired is
the most efficient lie a README can tell.

## What could not be done from here, and now has been

Two separate refusals, from two different principals, both for repository
settings:

- `PATCH /repos/{owner}/{repo}` with `{"default_branch": "main"}` →
  `403 Repository settings writes are not permitted through this proxy`
- `actions/configure-pages@v5` with `enablement: true` →
  `Create Pages site failed. Error: Resource not accessible by integration`

Neither is a bug and neither had a workaround worth having. What they did buy
was a better failure: the Pages job now prints the exact setting to change as
the last line of its log, instead of a bare `Not Found` from the Pages REST
API, which is what a person actually needs when they are the only one who can
fix it.

## Alternatives rejected

**Annotating the two ledger entries that say these are pending.** They were
true when written and are dated. `ledger/README.md` — as of this morning —
distinguishes withdrawing a claim that was *false when written* from annotating
one that merely went out of date, and says to leave the second kind alone. This
entry is the record that they landed; that is what the ledger is for. Editing
the old ones would be the status-tracking the file explicitly is not.

**Leaving the badge off.** Considered seriously, because badges are usually
decoration and this README is unusually free of it. Kept because this one
carries a fact that was recently false and is easy to get wrong again: the
badge goes red the moment `main` breaks, which is the single cheapest signal
this repository now has.

**Waiting to add the site link until the first deploy succeeded.** Rejected as
the wrong order — this push *is* what triggers the deploy, since `pages.yml`
runs on every push to `main`. The link is verified immediately after, and if
the deploy had failed the right move would have been to fix it rather than to
have withheld the line.

## Evidence

`default_branch: main` and `has_pages: true`, both read back from the
repository API rather than assumed. (`GET /repos/{owner}/{repo}/pages` itself
returns 403 through the proxy — settings *reads* are blocked too — so
`has_pages` on the repository object is the available confirmation.)

The Pages deploy triggered by this commit, and the published site, are checked
immediately below in the session that made this change.

## What this does not do

The release upload is still the one thing in this repository that has never
executed: `softprops/action-gh-release` needs a `v*` tag, and pushing a tag is
refused here with `403` exactly as the settings writes were. Its build half —
both targets, the cross toolchain, `--check` on the shipped binary — runs on
every push.

Nothing keeps `main` and the working branch in step. They are equal at this
commit because they were pushed that way, not because anything enforces it.
