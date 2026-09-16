# The Pages workflow's header described a two-page site, and the site has been eleven files and a font directory since the rebuild.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `.github/workflows/pages.yml` (comments only)
- **Kind:** docs

## What changed

Two comment paragraphs at the top of the deploy workflow. "Two static pages and
a stylesheet" is now what the directory actually holds — a landing page, the
workbench, nine documentation pages, three stylesheets and four self-hosted
fonts — and the published URL is named there, which it was not anywhere in the
workflow that publishes it. The second paragraph's "three static files" is now
"a directory of static files". The mention of the guard living one workflow
over now names `docs.py` beside `quickstarts.py`, because the site rebuild
added the second checker and the comment only knew about the first.

No behaviour changes. The step that lists the files to check for was already
updated when the site was rebuilt; only the prose above it was left behind.

## Why

The header is the first thing somebody reads before touching a deploy, and it
described a site two commits of work out of date. A reader trusting it would
have expected a `site/` holding three files and found forty — which is the
cheap version of the failure, and the expensive version is deciding from a
stale comment that some file is safe to drop.

It also answers a question the repository could not: *where does this actually
publish?* `site/README.md` says, and the workflow that does it did not.

## Alternatives rejected

**Leave it — it is a comment.** This repository's stated position is that a
stale doc is worse than none because it is read as current, and a comment on a
deploy path is read by whoever is about to change the deploy. There is no
version of "just a comment" here.

**Generate the file list into the comment from the check step.** The one place
that must not drift is the existence check, and it already enumerates the files
in executable form. Duplicating that into prose that a script keeps in sync
would be machinery for a paragraph that changes once a year.

## Evidence

The workflow still parses and still has its two jobs, `build` and `deploy`
(`python3 -c "import yaml; ..."`).

The published site is live and confirms the point: `curl` of
`https://howlerops.github.io/slate-orm/` returns `200` with
`<title>slate — query workbench</title>` — the *old* structure, where
`index.html` was the workbench, because Pages publishes from `main` and the
rebuild is on a feature branch. Twenty-three successful runs of this workflow,
most recently 2026-09-16T02:06Z from `main` at `45bc68b`.

## What this does not do

It does not deploy the rebuilt site. Pages triggers on `main` only, so the
landing page, the docs and this branch's subquery work all publish when the
branch merges and not before. Until then the live URL serves the old
single-page workbench, which is correct behaviour and worth knowing when
comparing the site to the repository.
