# A many-to-many is a composition, not a third kind of relationship

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `crates/slate-orm/src/relation.rs`, `crates/slate-derive/src/lib.rs`, `crates/slate-orm/tests/many_to_many.rs`, `docs/orm-comparison.md`
- **Kind:** feature

## What changed

`load_related_through` loads a many-to-many in two reads, and
`#[record(has_many(Tag, through = ArticleTag))]` gives the pair a name.
Six tests, including the oracle the plan asked for.

## Why

The plan described this as a third kind of relationship beside `has_many` and
`belongs_to`. It is not one. The function's bounds are `P: Related<J>` and
`J: Related<C>` — the has-many onto the join table and the join table's
belongs-to onto the far side — and nothing else. Both already come from the
ordinary declarations, so the *capability* needed no new attribute, no new
derive output and no change to `Related`.
`a_many_to_many_needs_no_new_declaration` is that claim as a test: it loads
articles' tags without `Through` appearing anywhere.

Two reads, not one and not N. One for every parent's join rows, one for every
join row's far row, both through `load_related`, so both deduplicate their `IN`
values. Not one read, because that is a join, and a join returns the product —
each article repeated once per tag — which is more bytes and still has to be
regrouped.

## Alternatives rejected

**A blanket `impl<P: Related<J>, J: Related<C>> Through<C> for P`,** so the
name costs nothing. This is what I wrote first, and `E0207` refuses it: `J` is
not constrained by the trait, the self type or the predicates. The compiler is
right — if two tables could stand in the middle, nothing picks one. That error
is the whole argument for the attribute existing, and it is why `Through` is
derive-emitted rather than free.

**No attribute at all, and let callers name the join table.**
`load_related_through::<_, Article, ArticleTag, Tag>` works today and is
arguably clearer, since the join table is visible at the call site. Rejected
because the plan's spelling is the one every other ORM uses, and because
`load_through::<_, Article, Tag>` is what a caller reaches for. Both are public;
the four-type form is not hidden.

**Deduplicate the far rows.** `has_many through` in ActiveRecord returns
duplicates without a `DISTINCT`, and removing them here needs `C: Ord` or
`C: Hash`, which `Record` does not require. Narrowing the bound for every
caller to fix a case a uniqueness constraint on the join table already
prevents is the wrong trade. Documented and tested instead.

**Clone the join rows to keep the per-parent grouping.** The obvious
implementation flattens `Vec<Vec<J>>` by cloning, and `Record` does not require
`Clone` either. Recording the group lengths before flattening by value costs
one `Vec<usize>` and no bound.

## Evidence

Six tests pass; `relations` (6) and `derive_relations` (7) still pass, so the
derive change did not disturb the existing attributes.

Six mutations:

| mutation | result |
| --- | --- |
| the regrouping loses a parent's boundary | killed |
| the counts are taken after flattening | killed |
| an empty far side returns no entries rather than empty ones | killed |
| no parents still issues a read | **survived** → removed as redundant |
| a `through` spec also emits a `Related` impl | not expressible |
| `Through::Join` names the far type, not the join table | **does not compile** |

The survivor was real redundancy: `load_related` returns early on no parents
itself, so the outer guard's early return was unreachable in effect and its
comment claimed a saving already made a line deeper. Removed, with the reason
where the guard was.

The last row is the best outcome available. Pointing `Through::Join` at the far
type is the mistake the attribute makes possible, and it fails to *build*:
`load_through` requires `Join: Related<C>`, and `Tag: Related<Tag>` does not
exist. A wrong join table cannot produce a wrong answer.

The oracle is `it_agrees_with_the_explicit_two_step`, and it is deliberately
not a second copy of the same loop: it walks the join rows and fetches each far
row one at a time — the N+1 this function exists to avoid — so an
implementation that regrouped by the wrong offset agrees with itself and
disagrees with it.

## What this does not do

No client has it. `load_related_through` is a record-layer function, and the
wire's `Related` RPC takes one relationship, not two — a many-to-many over the
wire is two `Related` calls and a regroup in the client, which is what a caller
would have to write by hand today. That is the same gap `Related` itself had
before P2, and closing it means a second RPC or a `through` field on the
existing one; neither is designed.

Nothing measures it. "Two reads rather than N" is a claim about round trips
that the code's shape makes obvious and no benchmark here confirms — the
`IN`-deduplication assertion in `parents_sharing_a_tag_share_one_read_of_it`
counts the values in the filter, which is the request's size and not its
latency.

Nesting is not addressed: `load_related_through` goes one hop through one join
table. Article → Tag → something is P5's problem and shares none of this code.
