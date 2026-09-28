# The gap table had stopped describing the tree, in nine places

## What changed

`docs/orm-comparison.md` is the canonical answer to "what is left to build". Nine
of its claims were false, and a tenth thing it never claimed at all. Also
corrected: `docs/ctes.md`, `docs/views.md` and `site/docs/roadmap.html`, each of
which described the CTE work as unbuilt an hour after it was built.

**Stale — the tree contradicts the doc:**

| Was | Is |
| --- | --- |
| Views "designed, not built … none yet" | Built: `[[views]]`, `views.rs`, `authorized_read_source`, `as_view`, codegen |
| A check "cannot name the column … cannot report more than one … is not published" | All three closed: `CheckFailure.column`, `violations: Vec<_>`, `--print-schema` |
| "none of the eighteen RPCs" | Nineteen |
| "12 of the adapters' 24 endpoints" | Eleven — a miscount on the day it was written, not drift |
| N4: "Go does too" (carries generated rpc types) | Deleted by N4 itself; `internal/pb/` holds only `slate/` |
| P5: an include list whose depth is data, "nothing here parses one" | N1 shipped `repeated RelatedStep path`, `max_relation_depth` |
| Migrations: "no column names … anywhere in the keyspace" | `TableState.schema: Option<StoredSchema>`, format 2 |
| Factories: lead says "half open" | Its own body says "Built, in Rust" |
| CTE row: the parser "refuses `WITH` by name" | A CTE read once expands; seven narrower shapes refused |

**Decided rather than corrected.** *Generated migrations* moves to Refused. What
Alembic `autogenerate`, Drizzle Kit and Prisma Migrate emit is not a diff — it is
a committed, ordered, **editable** artifact applied later, and Alembic's whole
value is that the file is a draft you fix because it cannot tell a rename from a
drop plus an add. Here the catalog *is* the target, so there is nothing to edit;
the diff is computed at boot, so there is no chain to order; and the only
data-touching step is an index backfill, so there is no statement list to hold.
`--plan` gives the informational half. The half that needs a file is refused by
the architecture that makes the catalog the single source.

**Removed.** *Set operations* had a row in Missing whose own text said Refused,
while the Forbidden section already carried the `UNION` half. One entry now, in
Forbidden, covering all three and naming the by-name refusal.

**Added.** A row for `OR` in a predicate. All seven ORMs express a disjunction
and this one could not until recently — so it was a real gap against all seven
and the original audit simply had no row for it.

## Why

A gap table is read to decide what to work on. Every stale row costs either wasted
work on something already built, or a reader concluding the document cannot be
trusted and going to the source — at which point the document is overhead.

Eight of the nine were found by reading rather than by any check, which is the
finding under the finding: `sh scripts/check.sh` is 71-for-71 green on every one
of them, because no guard can see a true sentence that has stopped being true.
The one mechanism that could — `scripts/retired_claims.json` — only fires on a
phrase somebody registered, and these were never registered because nobody
noticed them going false.

## Alternatives rejected

**Strike the four plan-item problem statements the audit also flagged.** An
audit pass wanted "nothing here bounds the count" and three like it struck,
because P2's and N3's *Stops at* lines had been struck when they went false.
Rejected, because the two are not the same kind of sentence: a *Stops at* line
describes the shipped thing as it is now, so it must be struck; a plan item's
problem statement describes what was wrong *before* the item and is history by
construction. The inconsistency was that this had never been written down —
so it is written down now, in "How to read this", rather than resolved by
striking four sentences that are correctly unstruck.

**Leave the `OR` row out.** It is not staleness — the audit never had the row —
and adding rows for capabilities the original audit missed edges toward rewriting
its history. Taken anyway, because a gap table that silently gains a capability it
never listed is one nobody can audit against, and the row says plainly that the
audit missed it.

**Regenerate the document from the tree.** The recurring fix for a doc that goes
stale. It cannot work here: most of the cells are *arguments* — why a thing is
refused, what it would cost — and nothing in the tree holds those. What could be
generated is the built/not-built column, which is the part a reader trusts least.

**Register every corrected phrase in `retired_claims.json`.** That guard exists
for exactly this and now reads `.html`. Not taken for all nine, because its
value is catching a phrase *reintroduced later*, and a sentence rewritten in
place is not coming back. Registering nine dead phrasings would grow the file
nine entries for one imagined failure each.

## Evidence

Each correction was verified against the tree before it was written, not from the
audit's say-so:

- `grep -c "^  rpc " crates/slate-server/proto/slate/v1/records.proto` → **19**.
- `examples/explorer/web/src/api.ts` fetches **11** distinct `/api/…` paths;
  `examples/explorer/backends/go/main.go` registers **24**.
- `clients/go/internal/pb/` contains only `slate/`; `clients/go/slate/error.go`
  imports `google.golang.org/genproto/googleapis/rpc`.
- `records.proto` has `repeated RelatedStep path`; `session.rs` has
  `max_relation_depth`, defaulting to `Some(4)`.
- `crates/slate-schema/src/error.rs` has `CheckFailure { check, column, message }`;
  `crates/slate-kernel/src/record.rs` collects `violations: Vec<CheckFailure>`;
  `crates/slate-serverd/src/main.rs` emits `checks`.
- Views: `config.rs` `views: Vec<View>`, `views.rs`, `service.rs`
  `serving_views`, `examples/explorer/head.toml` `[[views]]`.

`python3 site/check/docs.py` — *the docs site holds together*, after the
`roadmap.html` edit.

**No mutation run.** This commit changes prose in four documents and nothing
executable. There is no behaviour to break; the guards that read these files
(`check_site_claims.py`, `check_cited_docs.py`, `check_retired_claims.py`) are
already mutation-tested against their own suites, and mutating prose they read
would test them again rather than anything written here.

## What this does not do

**Nine corrections, and nothing that stops the tenth.** Every one was found by a
person reading. The structural fix — generating the verdict column, or a guard
that pairs each row with a symbol that must exist — is rejected above for the
first and unbuilt for the second, so the next feature to ship will make some row
false and nothing will say so. The honest statement is that this document needs
re-reading whenever something in it is worked on, enforced by nothing.

**The four kept plan-item problem statements are now defensible rather than
verified.** The convention distinguishing them from a *Stops at* line is written
down, but nothing checks that a sentence filed as history actually sits under a
current-voice block. A plan item whose block was never written would read as a
present-tense claim and match no rule.

**The migrations decision rests on knowledge of three tools, not on running
them.** None of Alembic, Drizzle Kit or Prisma Migrate is installed here and this
container has no network. The doc says so where the decision is stated. If
Drizzle Kit's snapshot diff is closer to `--plan` than described, the decision is
worth reopening.

**The `OR` row is the only audit omission looked for.** It surfaced because a
sweep noticed the disjunction work had no row, not because anything enumerated
what the seven comparison ORMs have and this table lacks. There may be other
capabilities every one of them has that the original audit never listed, and
nothing here searched for them.
