# Asking every finding where else its capability lives, and writing the answer down

- **Date:** 2026-09-20
- **Author:** Claude, finishing the sweep the previous entry only started
- **Touches:** `crates/slate-kernel/src/security.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

Documentation only, and two specific claims rather than a tidy-up. A bound in
`security.rs` that was true of the mechanism it described and false of the
method it was attached to. And a table in `docs/security-review.md` recording,
per finding, where else that capability lives and what holds the fix there —
including two rows where the answer is "I enumerated by reading".

## Why

Five of the eight findings were fixed once and then found open on a second
path: a second `Catalog` constructor, a second write path, a second
`Authenticator`, two more `Grouper` sites, four more handlers. Every one was
judged equivalent by reading before it was found by probing.

Finding 2's fix entry left the general question open — *does any other
finding's fix sit behind one path of several?* — and closing it for `upsert`
answered one eighth of it. This is the rest, and it produced no further code
defect. That is the result, and it is worth stating plainly rather than
padding: **3 and 5 turned up nothing, 4 is closed as inherent, and 7's second
path was already closed earlier today.**

The one thing it did produce is a stale claim, in the place a reader is most
likely to trust. `security.rs` bounds finding 4's oracle with:

> It is same-tenant only. A tenant-scoped table puts the tenant in the key
> prefix, so a key in another tenant is a different key and there is no
> collision to observe. (Finding 2 was a separate path where the same bit *did*
> cross tenants; that one is fixed.)

Both sentences are true about *collisions*, and the paragraph is attached to a
list that names `upsert` — which was, at the moment those words were written,
disclosing across tenants by a different mechanism entirely. Not a collision: a
read issued before the policy was decided, so the error said which answer the
read had given. The parenthetical's "that one is fixed" was true of
`write_many` and false of the method three lines above it.

A reader checking whether their own multi-tenant deployment was exposed would
have read that paragraph, found the bound, and stopped.

## Alternatives rejected

**Delete the bound.** It is correct about the collision oracle and that is
finding 4's actual subject, so removing it loses a true and useful statement to
avoid an adjacent false one. Kept, with the adjacent thing said.

**Say only "fixed" and move on.** What the parenthetical already did. The
sentence that stops this recurring is not "it is fixed" but "no collision to
observe does not mean nothing to observe, and the ordering of the read against
the policy is the other half" — because the *next* path will also not be a
collision.

**Record the sweep in this ledger entry only.** The ledger is where reasoning
goes and `docs/security-review.md` is where a reader goes. A per-finding answer
that lives only in a dated file nobody greps is an answer that will be asked
again.

**Claim the sweep is complete.** It is not, and the table says which rows are
weak. Findings 3 and 4 were enumerated by reading the callers and the wire
surface — which is exactly the method that failed five times. What is different
is that this enumeration is small, specific and written where it can be
checked; the five that failed were never written down at all. Asserting more
than that would be manufacturing the confidence the sweep exists to avoid.

## Evidence

**Finding 3, enumerated.** All six kernel `explain*` methods —
`explain`, `explain_join`, `explain_chain`, `explain_grouped`,
`explain_grouped_join`, `explain_grouped_chain` — call one helper,
`reads().authorize_explain(context, ..)`, and each passes *every* table of its
plan, not the first:

```
explain:               authorize_explain(context, &[table])
explain_join:          authorize_explain(context, &[left, right])
explain_chain:         authorize_explain(context, tables)
explain_grouped:       authorize_explain(context, &[table])
explain_grouped_join:  authorize_explain(context, &[left, right])
explain_grouped_chain: authorize_explain(context, tables)
```

That is a chokepoint rather than six copies of a check, which is the shape the
other findings did *not* have. On the wire, `estimated_rows` appears at nine
sites in `slate-server`, and all nine are inside an explanation conversion; the
RPC surface has exactly three explain RPCs and all three authorise
`Action::Explain` first. No query, get, join or aggregate response carries an
estimate.

**Finding 5 shares finding 1's fix**, and finding 1's second constructor was
closed this morning — so the `Restrict` arm is refused by `from_tables` and by
`insert`, in both insertion orders, which
`a_catalog_assembled_by_insert_is_refused_in_either_order` asserts.

**Finding 4 is unchanged by today's `upsert` fix, and I checked rather than
assumed.** Its oracle is same-tenant: a free key succeeds, a key held by an
RLS-hidden row is refused. Hoisting `check_row(Action::Insert)` above the read
does not touch that sequence — the caller's own row passes the `WITH CHECK`,
and the hidden row is still found by the read. Both pinning tests,
`an_upsert_leaks_the_same_bit_as_an_insert` and
`an_upsert_cannot_overwrite_a_row_the_policy_hides`, pass unchanged.

**The review's own recommendation 2 was right and unbuilt.** It said: *"A
differential test that runs each pair against a hostile row and asserts the
same error variant would have caught it and would catch the next one."* The
path it would have caught is `upsert` — one half of exactly such a pair. That
is now struck through and marked done, with the note that the review named the
test, described what it would catch, and was correct on both counts.
Recommendation 4, on resource budgets, is struck through too: that is finding 7
and it closed earlier today.

`cargo test -p slate-kernel --doc`: 3 passed. `cargo test -p slate-kernel`: no
failures. `python3 site/check/docs.py`: every relative link resolves.
`scripts/check_cited_tests.py`: 9 documents, every cited test resolves — which
matters here because the new table cites four test names.
`scripts/check.sh`: 27/27.

## What this does not do

**It is a reading, not a probe, for findings 3 and 4.** Stated in the table and
restated here because it is the load-bearing caveat. Nothing executes to
confirm that `estimated_rows` has no tenth site, or that a future RPC will not
carry one; the enumeration is a grep and a count I did by hand.

**It does not examine side channels.** Plan *choice* and response *timing* are
both derived from the same global statistics as `estimated_rows`, and both are
observable to a caller with no `Explain` grant. Recommendation 3 in the review
names them; nothing here narrows them, and I did not try to measure whether
they are exploitable.

**No new check enforces the table.** `check_write_paths.py` holds finding 2's
row and `check_handlers.py` holds 6 and 8's. Rows 3, 4 and 5 are prose, and
prose goes stale — which is the failure this very entry is about. The honest
version of that guard would be a roster of explain methods checked against
`authorize_explain`'s callers, and I did not build it, because the six are
already one call away from a single helper and a roster of callers of one
function is a thing the compiler can be made to say better than a grep can.

**The `security.rs` correction was not mutation-tested.** It is a doc comment;
there is no behaviour to break. What stands behind it is
`no_key_naming_write_path_answers_differently_for_another_tenants_key`, which
was mutation-tested in the commit before this one.
