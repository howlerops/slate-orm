# `GROUP BY a, b` on a join or a chain, and an ORDER BY that was off by one key

- **Date:** 2026-09-15
- **Author:** an agent, on `claude/rust-orm-record-layer-gswxlu`
- **Touches:** `crates/slate-wasm/src/{sql,lib}.rs`, `crates/slate-wasm/tests/{sql,taxi,datetime,playground}.rs`, `site/workbench.js`
- **Kind:** feature

## What changed

`JoinSpec::group_by` and `ChainSpec::group_by` are `Vec<u32>` rather than
`Option<u32>`, matching `QuerySpec::group_by`, which has been a list since
grouping arrived. The parser's `GROUP BY` clause loops the way the single-table
one does, the select list checks membership rather than equality, and the group
space is `[keys..., aggregates...]` throughout.

The workbench's "kitchen sink" example no longer apologises for the limitation
in prose, because there is no longer a limitation to apologise for.

## Why

The asymmetry was never a decision. The single-table path grew a second key and
the joined path was not revisited, so `GROUP BY payment, passengers` worked on
`trips` and became a refusal the moment a join appeared. The workbench's own
example said so out loud — it split itself into two statements and explained
that "ORDER BY and a second group key are not available on the join path". The
`ORDER BY` half of that sentence was fixed in task #141 and the sentence was not,
which is its own small lesson about prose that describes a gap.

Nothing below the front end needed changing. `Grouping::by` has always taken a
slice of ordinals, the kernel groups on however many it is given, and
`narrowed_join`/`narrowed_chain` narrow each input's projection to the columns
the keys and aggregates actually read. The restriction lived entirely in one
`Option`.

## Alternatives rejected

**Keep `Option<u32>` and add a second optional key.** Smaller diff, no
deserialisation change. Rejected immediately: it is the same mistake one step
further along, and the third key would need a third field. `QuerySpec` already
shows what the right shape is.

**Keep `Option<u32>` on the wire and flatten a list into it.** There is no wire
here to be compatible with — these specs are the browser binding's own API, and
the only callers are the parser and the crate's own tests. The one real cost of
widening was six test sites sending `"groupBy": 2`, which is a smaller price
than a permanently wrong shape.

**Refuse a duplicate key** (`GROUP BY borough, borough`) rather than
deduplicating it. Defensible, and closer to what a strict SQL engine does.
Rejected because `join_value_ordinal` already deduplicates *computed* keys by
find-or-add — the same call written twice is one computed column — so refusing
the stored case would make two spellings of one mistake behave differently for
no reason a reader could infer. Both now collapse to one key.

## Evidence

Four new tests in `crates/slate-wasm/tests/taxi.rs`, over all 100,000 real
trips, and the two that matter are differentials rather than written-down
numbers.

- `a_join_groups_by_two_keys_and_they_fold_back` groups by `borough, payment` —
  two keys from two different tables, which a single `Option<u32>` cannot hold
  at all — then folds the pairs down to boroughs *in the test* and requires the
  result to equal `GROUP BY borough` alone. It also asserts there were more
  pairs than boroughs, so a fixture where every borough had one payment kind
  could not make the fold vacuous.
- `ordering_by_an_aggregate_looks_past_every_key` is the one that found a real
  defect (below). It requires the counts to be non-increasing *and* the payments
  not to be, so an order that landed on the second key cannot masquerade as an
  order by the count.
- `ordering_by_the_second_key_orders_by_the_second_key` is the mirror image, for
  a key rather than an aggregate, with the same "and the other column is not
  sorted" guard.
- `a_second_key_is_checked_like_the_first` covers three refusals, including that
  the message is singular with one key and plural with two.

### The off-by-one

`join_group_ordinal` resolved the `n`th aggregate to `n + 1`, which is correct
when there is exactly one group key and wrong by `keys.len() - 1` otherwise. It
is now `group_by.len() + n`. The same function resolved a *key* to a constant 0,
which was likewise right when 0 was the only key; it now returns the key's
position.

Neither was reachable before this change — there was only ever one key — so this
is a latent defect introduced and fixed in the same commit rather than one that
shipped. It is written down because the shape is worth recognising: widening a
"there is exactly one of these" assumption leaves every `+ 1` and every hardcoded
`0` behind it silently wrong, and both of those would have produced a correctly
shaped answer in a different order, with nothing to report it.

### Test sites updated

Nine assertions across `datetime.rs` and `playground.rs`: six sending or
asserting a scalar `groupBy` (now `[n]`), and three on refusal wording that went
from singular to plural. All are mechanical; none changed what is being checked.

## What this does not do

**`HAVING` on a join or a chain is still a refusal.** The single-table path has
it; the joined path never did, and this change does not add it — `Grouping` has a
`having` field, so the work is in the parser and in resolving a `HAVING` term
against the group space, which is now correct for several keys and would be the
place to start.

**No mutation testing in this entry yet.** The two previous entries' passes each
found a real missing test, and the ordering tests above were written *because*
the off-by-one was predicted from the shape of the change rather than found by a
mutation. A pass over this code is the obvious next step and has not been run.

**The panel has no second-key control**, because the panel no longer exists —
the workbench replaced it, and a second key is written in SQL like the first.
