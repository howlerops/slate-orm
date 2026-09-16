# A feature audit against seven ORMs, and a six-piece plan for the gaps worth closing.

- **Date:** 2026-09-16
- **Author:** an agent working through the record-layer task list
- **Touches:** `docs/orm-comparison.md` (new), `site/docs/roadmap.html` (new),
  `site/docs/nav.js` and the `<noscript>` fallback on all nine other docs pages
- **Kind:** docs

## What changed

A new design note, `docs/orm-comparison.md`, comparing `slate-orm` feature by
feature against SQLAlchemy 2.0, Drizzle, Prisma, Ecto, ActiveRecord, Diesel and
SeaORM; and a new docs page, *Against the other ORMs*, carrying the short form
for a reader who arrives at the site. The docs nav gains a tenth page under
*Honesty*, beside *What it is not*.

Both split every absence into three categories — **missing** (work), **refused**
(a decision with an argument), **forbidden** (the architecture rules it out) —
and both open with what is already competitive, because a gap list with no
baseline is not an audit.

## Why

The question was "what are the best ORMs doing that we are not", and the
honest answer needed checking rather than recalling. Two of the findings were
worth the exercise on their own:

**Predicate writes do not exist.** An update here is a row replacement by
primary key; delete takes primary keys only. `DELETE FROM sessions WHERE
expires_at < now()` cannot be expressed — the caller queries the keys, carries
them back, and deletes one per key, which is N+1 by construction and is not
atomic with the query that found them. The update half is worse: without
`SET views = views + 1`, incrementing a counter is read-modify-write, and two
callers doing that lose one increment. `update_if_unchanged` makes the loss
*detectable*, which is real optimistic concurrency and works, but detecting a
lost update and retrying is a worse answer than not losing it.

**Relationships are Rust-only.** `#[derive(Record)]` emits a `Related` impl and
`load_related` dedupes the key set and issues one read — genuinely better than
the lazy-loader-plus-preload shape every ORM on the list has, because there is
no lazy fallback to trigger by accident. None of it crosses the wire. A Python,
Go or TypeScript caller writes the two-step fetch by hand, and the one who
forgets writes the N+1 the Rust API exists to prevent. We built the good answer
and did not ship it to the three audiences most likely to need it.

## Alternatives rejected

**A feature matrix with ticks and crosses.** The obvious format, and it would
have been actively misleading here. "No lazy loading" and "no `DELETE WHERE`"
are both crosses and they are opposite facts — one is a deliberate refusal that
prevents the commonest bug in the field, the other is a hole. A matrix flattens
that distinction, and flattening it is how a project ends up building something
it had already decided against. Hence three categories instead of two columns.

**Recalling the ORMs' feature sets rather than reading their docs.** Cheaper,
and the failure mode is a plan aimed at a feature that was removed two major
versions ago. The seven were read from current documentation; the claims about
*this* repository were each established by a search named in the note, so the
next reader can re-run them rather than trust a file that will go stale.

**Writing the plan straight into the README's "not built" checklist.** That
list is a good inventory and a bad plan: it says what is absent, not what it
would cost, what it would be tested against, or where it stops. Those are the
three things that decide whether an item is worth starting, so they needed
somewhere with room for prose.

**Building the first item instead of writing the plan.** Tempting, and wrong
order: predicate writes are kernel work touching the write path, index
maintenance and the security predicate at once, and the RLS half — a predicate
delete must not touch a row the policy hides — is the part most likely to be
got wrong quietly. That deserves a written test strategy before code, not after.

## Evidence

Every claim in the note was established by a search, and the searches are named
in the note itself. The load-bearing ones:

- `grep -n "message DeleteRequest" -A 14` on the proto: `repeated Row
  primary_keys` and nothing else. The `UpdateRequest` comment states the
  row-replacement rule in the protocol's own words.
- `grep -rn "fn delete_many\|delete_where" --include=*.rs crates/` → nothing.
- `grep -cin "relat\|preload\|include"` on the proto → 1, in an unrelated
  comment. The Python client's public methods are `insert, update, delete, get,
  query, join, aggregate, explain, transaction, transact`.
- The proto's RPC list is 13 calls, with no chain, no batch and no cursor
  field — so chains, keyset pagination and `RETURNING` are kernel-only too.
- `Expr` carries `And`, `Or` and `Not`; aggregates are `Count, CountColumn,
  Min, Max, Sum, Avg, CountDistinct` — no window functions. `ValueType` has no
  `Array`. `grep -rcn "validate\|before_save\|Changeset" crates/slate-orm/src/`
  → nothing.
- `#[derive(Record)]` accepts `has_many` and `belongs_to` and emits a `Related`
  impl at `crates/slate-derive/src/lib.rs:629` — but no `through`, which is
  what makes many-to-many a real gap rather than an assumed one.

The docs site holds together: `site/check/docs.py` passes with the tenth page
added, which covers nav agreement, orphans, theme loading, HTML nesting and
link rot.

The page was also rendered in a real browser, because the sidebar, the table of
contents and the previous/next pager are all built by `nav.js` at run time and
a page that parses can still arrive with no navigation at all. Six assertions,
all passing: the sidebar lists all ten pages, this page is the highlighted one,
the TOC has the seven `<h2>`s, the pager links back to *What it is not*, the
fourteen-row gap table rendered, and there were no console or page errors.

No mutation testing: there is no behaviour here to mutate.

## What this does not do

**It does not build anything.** The six pieces are a plan, in order, with a test
strategy each. Nothing in the write path has changed.

**The browser render was a one-off, not a check.** `site/check/docs.py` is
static and `site/check/workbench.py` drives only the workbench, so nothing in
CI renders a documentation page. That is a real hole — `nav.js` builds the
navigation for all ten pages and a JavaScript error would empty the sidebar
site-wide with every static check still green — and it is not closed here.
Closing it means a browser check over the docs pages, which is its own task.

**The plan leaves out validations, changesets and lifecycle hooks**, which are
the largest thing the list has that we do not. That is deliberate and stated on
both pages: it is a design question rather than a missing function — where does
validation live when three clients in three languages share one catalog? — and
the honest answer is that we have not worked it out.

Window functions, CTEs, views, arrays, full-text search and set operations are
all real absences, all named, and none is in the six.
