# Three caveats that said "there could be another"; there is not, and now the count is written down

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `docs/caveat-status.json`
- **Kind:** process

## What changed

Four open caveats re-read against the tree and re-recorded. No code. Three of
them said some version of *"and there could be a second one somewhere"*, which
is a claim with a number, and none of the three had the number:

| caveat | said | is |
| --- | --- | --- |
| `…proto-roster…`: *"there could be a third"* tree of protos | unquantified | **2** trees, 6 files, symmetric |
| `…one-allocator…`: *"nothing stops a third copy appearing"* | unquantified | **1** port allocator in the tree |
| `…path-in-a-command…`: the site's two checkers do not know about each other | unquantified | **0** links name a repository source file |
| `…not-in-was-refused…`: *"No client can express it"* | a capability claim | **all three** clients can |

All four move from `open` to `narrowed`, each with the count as the `by` and
"nothing keeps it that way" as the residual.

## Why

Open caveats went **123 → 126** over five commits today, every one of which
closed something. That is not a paradox and it is not a problem with the work:
each entry records its own gaps honestly, so closing two caveats that cost four
new ones is a *net gain in what is known* and a net loss on the counter.

What actually moves the counter is the other direction — re-reading caveats the
tree has since answered. This repository already has a name for why they
accumulate: **same-session overtaking**, a session writing down what it did not
do and then doing it. What this batch found is a second, quieter source. A
caveat of the form *"there could be another X"* is written at the end of a
change, when nobody has counted the Xs, and it is **true forever and unfalsifiable
as written**. It never gets re-read, because re-reading it does not settle it.
Counting does.

The counts are the interesting part.

**Two proto trees, not "at least two".** `git ls-files '*.proto'` is six files
in exactly two directories, and the two hold the same three paths. The caveat
worried about a copy vendored somewhere else; there is not one.

**One port allocator.** `grep -rl ip_local_port_range` over the tree is
`scripts/free_ports.py` and nothing else, which is what the dedup earlier today
was for — and the caveat was written in the same breath, about a copy that
might arrive later.

**Zero links, and three false ones.** This is the one that changed my mind
rather than confirming it. The caveat was mine, from this morning: *a path
written as a link rather than as code is read by neither checker*. The site has
**33 distinct `href`s, 21 relative**, and none names a repository source file —
they are all site pages and stylesheets. But three of them (`docs/index.html`,
`docs/limits.html`, `docs/quickstart.html`) *would* match a tracked file by the
suffix rule, resolving to `site/docs/index.html` and friends — by accident,
because the link is relative to `site/` and the guard resolves from the root.

So widening `check_cited_files.py` to read `href`s would not close a gap. It
would add three resolutions that are right by coincidence, in a guard whose
whole subject this morning was *a suffix that resolves to the wrong file reads
as resolved*. The two checkers not knowing about each other turns out to be
the correct arrangement and not an omission, and the reason is a measurement
rather than a preference.

**And one that was simply wrong.** *"No client can express it"*, about `NOT IN`
— every one of the three clients has a negation helper: `slate.not_` in Python,
`slate.Not` in Go, `not` in TypeScript, so `not_(col.in_([…]))` is the
expression. The bullet's own second sentence said as much (*"a client that wants
this builds the `Not` itself, which is possible and undocumented"*) and its
first contradicted it. What is true is the *undocumented* half, and — worse,
and not noticed when it was written — **no test in any of the three clients
uses the negation helper at all.** Three doors, nobody walking through any of
them.

## Alternatives rejected

**Close all four rather than narrow them.** A measured zero is not a guarantee:
nothing stops a third proto tree, a second allocator or a link-style citation
arriving tomorrow, and `closed` in this tracker means answered rather than
currently-absent. Narrowing is the honest verdict and the residual field is
exactly the place for "nothing keeps it that way".

**Leave them open and write nothing.** They would be re-read next month by
somebody who also would not count, because the caveats do not ask to be counted
— they ask to be worried about. Writing the number down converts each from a
standing worry into a fact with a date on it, which is what `checked` is for.

**Widen the citation guard to `href`s anyway, for the future.** Rejected on the
measurement above: it would introduce three wrong-for-the-right-reason
resolutions today in exchange for a class that has no instances. That is a
worse guard, not a wider one.

**Fix the `NOT IN` gap here rather than re-record it.** It is the one of the
four with real work behind it — a test in each client and a line in the docs —
and it does not belong in an entry whose subject is counting. Left narrowed,
with the missing tests named in the residual so the next session finds the
shape rather than the worry.

## Evidence

Every number above was taken from this tree, today:

- `git ls-files '*.proto'` → 6 files under `crates/slate-server/proto` and
  `clients/typescript/proto`, three paths each, no third directory.
- `grep -rln ip_local_port_range` over `*.sh *.py *.ts *.go`, excluding
  `target/` → `scripts/free_ports.py`, one file.
- The site's HTML, parsed for `href="…"` → 33 distinct, 12 absolute or
  in-page, 21 relative, 0 naming a tracked repository file *as a repository
  path*, 3 matching one by suffix coincidence.
- `slate.not_` (`clients/python/src/slate/expr.py:462`), `slate.Not`
  (`clients/go/slate/query.go:141`), `not`
  (`clients/typescript/src/query.ts:143`) — and `grep` for any use of them
  across `clients/python/tests`, `clients/go/slate` and
  `clients/typescript/test` → **nothing**.
- `python3 scripts/caveats.py` before: 126 open. After: 122.
- `sh scripts/check.sh`: 87 passed, all of them.

## What this does not do

**It counts four, and there are a hundred and twenty-two.** The "there could be
another" shape is worth a sweep of its own — the phrasing is recognisable and
`caveats.py --open` prints them all — and this entry did the four it met rather
than the class.

**A count is true on the day it is taken.** That is what `checked` records and
why these are narrowed rather than closed, but nothing re-takes them. A third
proto tree arriving is caught by `check_proto_copies.py` only if it is one of
*the* two trees; a second port allocator is caught by nothing.

**The `NOT IN` work is named, not done.** Three clients can express it, nothing
documents it and no test exercises it. That residual is the actionable one in
this batch and it is still open inside a narrowed caveat, which is the weakest
place for a real task to live.
