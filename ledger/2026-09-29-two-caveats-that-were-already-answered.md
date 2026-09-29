# Two caveats that were already answered, and a fix that was not needed

## What changed

Two verdicts in `docs/caveat-status.json`, from `open` to `closed`. No code.

- `2026-09-14-frontend-tests-and-configurable-ports.md`: *"The bar chart is
  asserted on count and threshold, not on geometry."* The e2e has read
  `getBoundingClientRect().width` off every `.chart .row .fill` since
  **2026-09-14**, in the same commit that fixed the first real CI runs.
- `2026-09-21-refusing-a-view-everywhere-else-is-free.md`: *"It leaves the
  refusal's wording wrong."* Every path that could produce it resolves through
  `Head::table` before the converter runs, so the wrong wording is
  unreachable — demonstrated by a mutation, not by reading.

A change was written for the second and **reverted**: `views::no_such_table`
single-sourced out of `service.rs`, and `join_from_proto` and
`aggregate_from_proto_query` taking the view registry. It compiled, it passed,
and it was dead code.

## Why

Both verdicts were re-read on 2026-09-28 and both were recorded *"still true"*.
Neither was.

The bar chart one is the plainer mistake. The entry's caveat is about the demo
frontend, `examples/explorer/web` holds a unit suite in `test/` and a browser
suite in `e2e/`, and the re-read looked at the first. The assertion it wanted —
*"a CSS change rendering every bar at zero width passes"* — is the second thing
`e2e/explorer.mjs` checks about the chart, under a comment that names the same
failure in nearly the same words. Fourteen days between the assertion landing
and the caveat being confirmed as unmet.

The view one is more interesting, because the entry's own reasoning had moved
underneath it. `docs/views.md` §3a's build order left a known cost: a declared
view, refused everywhere but `query`, reported as *"no table named `notes`"* —
a name the operator typed themselves, reported as a typo. A later entry gave
`Head::table` a `no_such_table` that names the view and its base table. Three
resolutions in `convert.rs` still said the old thing, so the fix looked
half-applied, and that is what the caveat was re-confirmed against.

It is not half-applied. `authorize_join_inputs` resolves **every** input's name
through `authorized_table` → `table` → `no_such_table` before the converter is
called, because a converter that resolves without a `SecurityContext` was
finding 8 on four handlers. Rule 3 of `check_handlers.py` requires that
ordering of every such call site. So the security fix closed the wording gap as
a side effect, and the property is enforced rather than incidental.

**That is not something I established by reading**, which is the point of
writing this down. I read the same code and concluded the fix was needed,
wrote it, and only the mutation said otherwise.

## Alternatives rejected

**Keep the change anyway, as defence in depth.** It cost a parameter on two
public converters and an unreachable branch, and the entry being closed
rejected exactly this shape of thing in 2026-09-21: *"a reader who finds five
explicit refusals concludes the refusal is maintained by those five, and the
sixth path gets added without one."* A second copy of a message that a guard
already routes through one function is that argument again, one level down.

**Keep the `views::no_such_table` move alone**, dropping the converter
parameters. It reads better — the function beside the map it consults — and it
is still churn for nothing: `service.rs` is the only caller, the doc comment is
already there, and moving it would orphan a witness needle for no behaviour.

**Close the view caveat without the mutation**, on the reading above. That
reading is *how the change got written*: the same code, read carefully twice,
supported both conclusions. A caveat closed on reading is exactly what this
repository keeps catching itself doing — the entry being closed is one, and so
is the 2026-09-28 re-read that got both of these wrong.

**Add a test asserting the converters' branch is unreachable.** There is
nothing to assert: the branch is unreachable because of what its callers do,
and `check_handlers.py` rule 3 already holds them to it. A test would pin the
handlers' ordering, which that rule pins better.

## Evidence

**The bar chart.** `examples/explorer/web/e2e/explorer.mjs`, in
`the grouped join draws a bar per author`: widths from
`getBoundingClientRect()`, a refusal if any is zero, a refusal if different
counts drew identical bars, and a check that the widest bar is the largest
count. `git log -L 400,422` dates the block to `09c543d`, 2026-09-14. The
caveat was confirmed unmet on 2026-09-28.

**The view wording.** Two mutations,
`ledger/mutations/20260929T063056-crates-slate-server-src-convert-rs.json`,
reverting each `convert.rs` resolution to `no table named`. **Both survived**
`cargo test -p slate-serverd --test views`, 7 passing either way — and that
suite is the one that would see it: `only_the_query_path_knows_what_a_view_is`
asserts all six refusals, `join` and `aggregate` among them, contain
``notes` is a view over `docs``. They pass because the handler refused before
the converter was reached.

Two survivors, and neither is a missing test. This is the third cause
`mutate.py`'s message names first: the mutation was a change to code nothing
executes.

The revert is `git checkout` of four files; `cargo check -p slate-server
--all-targets` is clean and the tree is identical to before.

## What this does not do

**It does not explain how one re-read missed both.** They were checked the same
day by the same pass. The bar chart failure is legible — the wrong directory —
and the view one is not: the code genuinely reads as a half-applied fix unless
you follow the caller. No count is offered for how many of that day's other
re-reads are wrong, because two is not a sample and guessing at a rate would be
worse than saying so.

**It adds no rule that would have caught either.** A caveat naming a browser
assertion could in principle be witnessed against `e2e/` as well as `test/`,
and `check_closed_caveats.py` does hold closed caveats to a witness in the
tree — but only once they are closed. Nothing witnesses an *open* one against
the thing it claims is absent, and that is the guard this pair argues for. It
is not written.

**It leaves the entry's other caveat open.** *"It does not check the property
it relies on"* — nothing fails if a future `Catalog` grows a second
name-to-`TableDef` lookup a view could satisfy. `check_handlers.py` rule 1
covers the handlers, not the catalog. Unchanged by any of this, and now the
only open caveat that entry has.
