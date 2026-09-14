# The home page stops describing the database and becomes one: a query editor over the kernel, with the prose moved to the docs

- **Date:** 2026-09-14
- **Author:** Claude (agent session), at the request of the repository owner
- **Touches:** `crates/slate-wasm/src/sql.rs` (new), `crates/slate-wasm/src/lib.rs`,
  `crates/slate-wasm/tests/sql.rs` (new), `site/{index.html,workbench.js,style.css,docs.html,README.md}`,
  `site/check/workbench.py` (was `playground.py`), `site/check/quickstarts.py`,
  `.github/workflows/{ci.yml,pages.yml,release-build.yml}`, `README.md`, `CLAUDE.md`
- **Kind:** feature

## What changed

`index.html` is now an application rather than a document: a schema tree on the
left, a SQL editor, a results grid, and tabs for the plan, the compiled spec
and a log. It loads slate's kernel as WebAssembly on arrival and answers every
statement locally.

The SQL is new, and is deliberately shallow. `crates/slate-wasm/src/sql.rs`
parses a `SELECT`/`INSERT`/`UPDATE`/`DELETE` subset **into the existing
`QuerySpec` and `JoinSpec`** — the types the dropdown panel already built and
the types the three SDKs send on the wire — and hands them to the binding
functions that were already there. The **Spec** tab shows the reader what their
statement compiled to.

Everything that was on the landing page (hero, quickstart with its three SDK
tabs, what it does, how it is built, what it is not) moved into `docs.html`,
reachable from a Docs button in the workbench header.

## Why

The owner asked for it, in these words: a pgAdmin-style editor as the home
page, so you can explore the data and craft queries to test it, with a Docs
button for the writing. The request has a good reason behind it, which the
previous entry in this ledger half-found: the panel was unfindable, and even
once found it was a row of dropdowns that could only ask the questions its
author had thought of. A reader wanting to know whether the planner does
something sensible on *their* query had no way to ask.

The SQL needed a decision, because slate has no SQL and never claimed to. Three
options were put to the owner; they chose a subset compiled to the spec, and
that choice is what makes the rest defensible. SQL here is a *spelling*. It
adds no execution path: `sql::Statement::Select(spec)` goes to the same
`answer_spec` the JSON path goes to, so there is no query the editor can reach
that a client cannot, and no plan the editor can produce that `EXPLAIN` would
not.

## Alternatives rejected

**A parser that builds `slate_kernel::Query` directly.** The obvious shape, and
wrong. It would be a second route into the executor, and the two routes would
drift: a conjunction that the spec path lowers with `Expr::all` and the parser
lowers as nested `And`s is the same predicate and a different plan, and nothing
would notice. Going through the spec costs one `serde` round trip and buys the
round-trip property below.

**A SQL library (`sqlparser` and friends).** It would accept far more grammar
than the spec can express, which is exactly the wrong shape: every construct it
parsed and the spec could not represent becomes an error *after* parsing,
phrased in terms of an AST the reader never wrote. The hand-written parser
refuses at the token that is wrong, in 900 lines, and has no supply chain.

**Keep the visual builder, make it richer.** Safest, and rejected because it
cannot do the thing asked for. "Craft queries to test it" means expressing a
query the page's author did not enumerate. A builder is a menu.

**Let the editor hold the spec JSON directly.** Honest, zero new surface, and
was the other serious option. Rejected by the owner in favour of SQL; the Spec
tab is what keeps the honesty that choice would have had for free.

**Silently AND an `OR`.** Never seriously, but worth writing down: the spec has
no disjunction. `OR` is refused with that as the reason. A query engine that
answers a different question than the one asked is worse than one that declines.

**Leave the prose on the home page, below the workbench.** That is the
arrangement that just failed: a reader who has to scroll past an application to
reach the quickstart will not, and a page trying to be both is neither. Two
pages, one button between them.

**Keep lazy-loading the wasm.** Rejected, and this reverses
`2026-09-14-not-paying-for-the-playground.md` on its own terms. That entry's
reasoning was that a reader who never scrolls to the panel should not pay 600 KB
for it. When the panel *is* the page, a visitor who waits for a shell and is
then asked to press a button has paid the latency and not received the thing.
The entry is left standing rather than edited: it was right about the page it
was written for. The cost is now stated in `site/README.md` instead of hidden.

## Evidence

**The round trip, which is the test worth having** — with one honest caveat
stated up front: it has never caught a real bug. It passed on its first run and
has passed since. What it has caught is every mutation aimed at it, and the two
shrunk cases in `sql.proptest-regressions` are from those mutations, not from
defects. `proptest` generates a
`QuerySpec` over `books` — arbitrary projection, up to three conditions with
operators chosen per column type, sort keys, limit, offset — renders it as SQL
with an independent renderer that lives only in the test file, parses it back,
and requires the same spec. String values are drawn from an alphabet containing
`'`, `;`, `(`, `*` and `-` on purpose.

**Equivalence.** Six queries written both ways must return identical rows *and
identical plan text*. The plan half matters: a front end that dropped a
condition and got lucky on the rows would pass the first assertion.

42 tests in `slate-wasm` (23 existing, 19 new), and 21 browser assertions in
`site/check/workbench.py`, all passing. Each new thing was mutation-tested —
break it, confirm a *named* test fails, restore:

| mutation | check that failed |
| --- | --- |
| `<` parsed as `>` | `a_spec_rendered_as_sql_parses_back_to_itself` |
| stop collapsing `''` inside a string | `a_spec_rendered_as_sql_parses_back_to_itself` |
| drop `OFFSET` on the floor | the round trip, and `sql_and_the_spec_return_the_same_rows_and_the_same_plan` |
| let any column stand in for the primary key | `refusals_name_what_was_wrong` |
| keep running statements after a refusal | `several_statements_run_in_order_and_stop_at_the_first_refusal` |
| report a parse offset without the statement's own | `a_refusal_points_at_the_offending_statement_in_the_buffer` |
| `UPDATE` that applies no `SET` | `update_changes_the_named_columns_and_leaves_the_rest`, `update_moves_a_row_between_index_entries` |
| print an undecoded column as "null" | `a column the plan never read is not rendered as a null` |
| leave the caret where it was on an error | `and the caret moves to the token that was wrong` |
| append a clicked column instead of inserting | `clicking a column inserts it at the caret` |

**One mutation survived**, and is the reason this section is worth reading:
deleting the check that a `table.column` qualifier names the table being
queried changed nothing. `SELECT authors.name FROM books` was still refused —
but by accident, because `books` has no column called `name` either. Two tables
sharing a column name would have made it silently wrong. A test now pins it
(`"SELECT authors.name FROM books"` → *is not a column of `books`*), and with
that test the mutation fails.

**Two real bugs, both caught by machinery rather than by reading.**

1. The first lexer walked `text.as_bytes()` and advanced while `bytes[i] as
   char` looked alphabetic. For `café` that accepts the first byte of `é`
   (`Ã`, alphabetic) and stops on the second (`©`, not), leaving the index
   inside a character — and the next slice panics. A parser's input is by
   definition whatever somebody typed into a text box. Rewritten over
   `char_indices`, with `text_the_lexer_could_choke_on_does_not_panic` to hold
   it.
2. **Reset did not reset.** The browser check reported five rows where four
   were expected: the button emptied the store and then called `run()`, which
   re-executed the editor — which still held the `INSERT` the reader had just
   run. It now clears the panes and says so instead.

**A withdrawn assumption.** I wrote a test asserting that a projected query and
an unprojected one return the same rows, on the strength of a comment in the
old `playground.js` saying "rows always come back whole". They do not: an
index-only scan returns `null` in every column it did not decode. The kernel is
right and the comment was half-right (the arity is preserved, the values are
not). The test now asserts the true thing, which is better than what I meant to
assert — and the grid renders those cells as a muted `·`, because printing the
word "null" for a column that was never read is a false claim about the data.

**The page, measured in a browser** (Chromium, 1280×900, the built bytes):
`SELECT * FROM books WHERE author_id = 2` → `Table Scan ... cost=1.60`;
`SELECT author_id FROM books WHERE author_id = 2` → `Index Only Scan using
by_author ... cost=1.00 decodes=[1]`, eight cells rendered unread. An insert
followed by the same index-only query returns one more row and stays
index-only.

**The wasm grew**: 596 KB gzipped → 689 KB, from the parser and the extra
`Serialize` impls. The build script's budget is 900 KB; it is not close, and
the number is now in `site/README.md` where a reader can see what the page
costs them.

## What this does not do

**The grammar is small, and its edges are refusals rather than features.** No
`OR`, no `HAVING`, no sub-queries, no `ORDER BY` on a join, no column list on
`INSERT`, no predicate `DELETE` (primary key only — deleting more rows than the
reader expected is the worst thing this editor could do). Each one is rejected
with a reason; none is silently approximated.

**The join is the fixture's only join.** `authors JOIN books ON authors.id =
books.author_id`, checked at parse time, because the fixture has one foreign
key and accepting any other pair would return nothing with no explanation.

**`UPDATE` is a read-modify-write**, not an atomic one. It reads the row through
the same query path everything else uses — deliberately, so a row a policy hides
stays hidden — then writes the whole row back. Between those two there is no
lock. Nothing here is concurrent, so nothing can observe it, but a reader who
assumes `UPDATE` maps to one kernel operation is wrong.

**No syntax highlighting, no completion, no query history across reloads.** A
`<textarea>` and a keyboard shortcut. An editor component is a dependency and a
build step, and neither is currently earned.

**The browser check does not assert correctness of results**, only that the
browser reaches them: rows appear, plans say what they should, controls do what
they claim. Whether a query returns the *right* rows is a Rust test, which is
faster and can say which row was wrong.

**Nothing verifies the two pages agree.** `docs.html` still claims things about
the project in prose; if the workbench and the prose diverge, only a reader
notices. The quickstart inside it *is* executed by `site/check/quickstarts.py`,
which is the part most likely to rot.

**Mobile is narrow-screen-tolerant, not designed for.** Below 720 px the tree
stacks above the console and the layout stops being viewport-height. It works;
it is not good.
