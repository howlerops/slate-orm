# A HAVING brackets the way a WHERE does, through the same parser

- **Date:** 2026-09-28
- **Author:** Claude, in a session asked to finish what the caveat tracker still listed
- **Touches:** `crates/slate-sql` (`sql.rs`, `lower.rs`, `lib.rs`), `crates/slate-wasm`, `site/docs/limits.html`, `scripts/retired_claims.json`
- **Kind:** feature

## What changed

`HAVING (count(*) > 5 OR count(*) < 2) AND count(*) <> 3` parses, lowers and
runs, on a single table, a join and a chain alike. The three spec shapes gain a
`having_predicate: Option<PredicateSpec>` beside the two flat lists, mirroring
what `QuerySpec::predicate` already is for a `WHERE`, and `group_predicate`
composes all three.

The parser was not copied. `disjunction`, `conjunction` and `primary` now take
a `Clause` enum saying what a leaf resolves against — a column for a `WHERE`, a
select item in group space for a `HAVING`, the joined group space for a join or
a chain — and everything above the leaf is shared: precedence, the refusal of a
bare `AND`/`OR` mixture, the flattening, and the unclosed-bracket message,
which now names the clause it came from instead of always saying `WHERE`.

## Why

`ledger/2026-09-25-or-in-having-too.md` left this as its residual: *a HAVING
still takes no brackets*. The reason recorded there was that the
group-condition lists had no nested form to lower onto.

That was true of the **spec** and was never true of the **kernel**.
`Grouping::having` is an `Expr` and has been since it was written; the executor
evaluates it with `grouping.having.admits(&group.as_row())`. So the missing
piece was one more field on three structs and a recursive walk, not an
operator, a plan node or anything the kernel had to learn. The limit was an
artefact of which clause parentheses reached first.

It also made the mixing refusal awkward: it tells the reader to write the
brackets they mean, and in a `HAVING` those brackets were themselves refused.
The advice named a fix that did not exist.

## Alternatives rejected

**Replace the two flat lists with the tree.** Tidier, and rejected for the
reason `QuerySpec::predicate` rejected it: every consumer — the Spec tab, the
panel, the round-trip property test, `slate-serverd` resolving a view, and
every spec JSON on disk — reads `having` and `having_any_of`. Making
`HAVING count(*) > 100` recursive costs every existing reader to buy nothing
for the query almost nobody writes. The cost of keeping three fields is that
two of them can describe one thing, which
`a_flat_having_never_lands_in_the_nested_field` is there to forbid.

**Copy the three recursive-descent functions and swap the leaf.** About thirty
lines, no new type, and it is exactly how this front end got into trouble
before: `OR` arrived in the `WHERE` and the `HAVING` as two pieces of work and
they disagreed for a week about whether a chain had it — which is the entry
this one closes. A second bracket-matching loop would have had its own
unclosed-bracket message, its own precedence, and its own opportunity to drift.
The `Clause` enum costs one `match` per leaf.

**A `Group` node in `PredicateSpec`, so the spec records where the brackets
were.** Rejected because `((a))` and `a` must produce the same spec — a view
stored as text has to resolve to one query whichever way its author bracketed
it — and a node recording parenthesisation makes them different.
`a_redundant_bracket_in_a_having_produces_the_same_spec_as_no_bracket` pins it.

**Leave it, and re-file the caveat as `deliberate`.** Defensible while nobody
had asked for the shape. It stops being defensible once the cost is known to be
a field and a walk, and the refusal is already telling readers to write
brackets.

## Evidence

Nine tests written before the implementation, all red against the blanket
refusal — seven in `crates/slate-sql/tests/front_end.rs` (58 → 68 with the
tenth below) and two through the browser binding in
`crates/slate-wasm/tests/having.rs`.

**Eleven mutations. Nine caught, two survived and both are now caught.** The
runs, oldest first — `mutate.py` records every one, including the two it
refused to score:

- `ledger/mutations/20260928T175343-crates-slate-sql-src-sql-rs.json` (4 cases, one survivor)
- `ledger/mutations/20260928T175419-crates-slate-sql-src-sql-rs.json` (the survivor, re-run against its new test)
- `ledger/mutations/20260928T175434-crates-slate-sql-src-lower-rs.json` (3 cases, one that did not compile)
- `ledger/mutations/20260928T175452-crates-slate-sql-src-lower-rs.json` (that one, rewritten as a real change)
- `ledger/mutations/20260928T175506-crates-slate-wasm-src-lib-rs.json` (refused: the anchor matched 0 times — lie 1, before `cargo fmt`)
- `ledger/mutations/20260928T175557-crates-slate-wasm-src-lib-rs.json` (2 cases)
- `ledger/mutations/20260928T180423-crates-slate-wasm-src-lib-rs.json` (the chain survivor)
- `ledger/mutations/20260928T180548-crates-slate-wasm-src-lib-rs.json` (the two-table join site, to locate it)
- `ledger/mutations/20260928T180655-crates-slate-wasm-src-lib-rs.json` (the chain survivor, re-run against its new test)


| mutation | caught by |
|---|---|
| the unclosed-bracket message hard-codes `WHERE` again | `an_unclosed_bracket_in_a_having_says_having_and_not_where` + 1 |
| a nested single-table `HAVING` is dropped | 4 tests |
| a nested join `HAVING` is dropped | `a_bracketed_having_reaches_a_join_and_a_chain` |
| a bracketed `OR` lowers as an `AND` | 2 tests |
| a bracketed `AND` lowers as an `OR` | 2 tests |
| the nested tree is conjoined away | 2 tests |
| the binding's single-table guard cannot see a nested `HAVING` | `a_bracketed_having_reaches_the_kernel_and_is_not_silently_dropped` |
| the binding's single-table call site passes `None` for the tree | the same |
| the binding's two-table-join call site passes `None` | `a_bracketed_having_reaches_a_grouped_join_too` |
| **an ORed single-table `HAVING` lands in the ANDed field** | **nothing — survivor** |
| **the binding's chain call site passes `None`** | **nothing — survivor** |

The survivor is the finding. `an_ored_having_reaches_a_chain` covered the
chain and the browser's suite covered the binding, so this crate's own surface
— the one `front_end.rs` exists to pin independently of the browser — had no
single-table ORed case at all. The mutation turns "either arm" into "both
arms", which no assertion about *which list is empty* can see:
`a_flat_having_never_lands_in_the_nested_field` asserts
`having.is_empty() || having_any_of.is_empty()` and passes either way.
`an_ored_single_table_having_is_a_disjunction_and_not_a_conjunction` closes it,
checking the lowering and not only the spec.

**The second survivor is why the join case is not enough.** The binding has
*three* `group_predicate` call sites — single table, two-table join, and chain
— and a grouped `trips JOIN zones` takes the second, so the join test left the
third untouched. It was found by mutating each site separately rather than
once: mutating "the join path" would have scored the two-table site and said
nothing about the chain. `a_bracketed_having_reaches_a_grouped_chain_too` uses
three inputs — the fixture's two tables with `zones` under an alias — and each
of the three sites is now named by a test that fails when it alone is broken.

One mutation was malformed on its first attempt — an `if false` match guard,
which does not compile — and `mutate.py` reported `NOTHING RAN` rather than
scoring it as caught. It was rewritten as a real change and caught.

Two guards in this repository fired on this change and were right to:
`the_where_and_having_lines_admit_or` failed when the grammar block's `HAVING`
line stopped listing its connectives inline, and `check_retired_claims.py`
refused the retired phrase until this file existed.

Suites: `cargo test -p slate-sql` 72 (68 + 4 unit); `cargo test -p slate-wasm`
230 across 15 binaries, including the 11 in `having.rs`. `slate-server` and
`slate-serverd` build clean and touch none of these fields.

## What this does not do

**It does not bracket a join's `WHERE`.** That clause is still `AND` only, and
for a reason that has not changed: its conditions are split by side so each
scan is narrowed before the hash join runs, and neither a disjunction nor a
tree spanning both sides can be split that way. It is now the *only* clause in
this front end that takes no brackets, which is why `limits.html` says "one
clause" rather than "two".

**The nested `HAVING` is a filter and never an access path.** A grouped read
folds every row before a group exists, so this could not have become a scan
bound whatever shape it took. No plan changed and no measurement was taken,
because there is nothing here for one to be about.

**The parser recurses once per bracket, with no depth limit.** So does the
`WHERE`'s, and nobody wrote that down when it was built; this is the first
statement of it. What makes it tolerable rather than a hole is who can reach
the parser: `sql::parse` has exactly two callers — the browser binding, where
the text is what the visitor typed into their own tab, and
`crates/slate-serverd/src/views.rs`, where it is view SQL from a TOML file the
operator wrote, read once at boot before the daemon serves. No caller-supplied
text reaches it over the wire; the gRPC surface takes a built spec, and
`MAX_EXPRESSION_DEPTH` bounds *that*
(`ledger/2026-09-18-a-depth-limit-this-server-states-rather-than-inherits.md`).
A limit here would be worth having the day SQL becomes something a client can
send, and is not worth guessing a constant for before then.
