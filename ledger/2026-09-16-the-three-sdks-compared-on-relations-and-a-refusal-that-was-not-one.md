# Relations in the three-SDK conformance corpus, and a refusal case that quietly returned rows

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/explorer/` — `head.toml`, `CONTRACT.md`, all three adapters, `conformance/conformance.py`; `docs/orm-comparison.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`POST /api/related` in all three demo adapters, and seven cases in the
conformance corpus that compare the three answers. The demo's schema gains its
first foreign key, `sales.book_id -> books`, because a relationship here *is* a
foreign key and the schema had none. The corpus is 59 cases and all three SDKs
agree on every one.

P2 is marked built in `docs/orm-comparison.md` and on the site's roadmap page,
with what was actually measured rather than what was planned.

## Why

This is the piece the plan singled out. Each client's own suite runs against
the same server, which catches *a* client being wrong and cannot catch two
disagreeing about something the server accepts from both. Relations are
unusually exposed to that: the server replies with one group per **distinct**
key that matched something, and says nothing about the caller's order. Putting
the answer back into the caller's order — including the empty groups, including
a key sent twice — is entirely the client's, three times over, with nothing
comparing them until now.

`sales.book_id` rather than the obvious `books.author_id`: the book
`Author Unknown` names author 99 on purpose, so that a right or full join has
an unmatched side to show. A foreign key there would refuse that row at write
time and take the join demonstration with it. `sales` has no such orphan, and
reading it backwards goes through `books`, which carries the row policy — so
the `reader` case asserts that a relationship load loses a hidden row exactly
as a direct read would.

## Alternatives rejected

**Compare only the forward direction.** Half the cases and half the code. The
backwards read is the one that goes through the policed table, so it is the
case with a security claim attached rather than a shape claim.

**A new pair of tables for the relationship**, leaving the demo's three alone.
It avoids adding a foreign key to a schema several other checks read. It also
means the relationship is exercised over data no other case touches, so a
disagreement about relations could never be cross-read against a disagreement
about anything else — and the seeded row policy, which is the interesting part,
would have to be duplicated onto the new table to get it back.

**Let the adapters take the whole relation from the request body** — table,
key, direction — rather than hard-coding `sale_book`. More flexible, and it
turns the corpus into a second little language the three adapters each parse,
which is the thing `CONTRACT.md` already argues against for filters. `through`
*is* settable, for one reason stated in all three: naming a key that does not
exist is how the three refusals get compared, and that is the only part of this
call they could plausibly word differently.

**Show relations in the demo UI too.** Worth doing and not done here: the UI is
where a reader sees the feature, the corpus is where it is checked, and the
corpus was the plan's item. Recorded below rather than quietly skipped.

## Evidence

`./run.sh --conformance`: **59 cases, the three SDKs agree on all of them.**

Two bugs found by running it, both in this change:

1. The Python adapter used `decode` without importing it at module scope — it
   was imported inside `build_filter`, which is not where the new handler is.
   Every relation case came back `{"error": {"kind": "adapter", "message":
   "name 'decode' is not defined"}}` while Go and Node agreed perfectly. The
   corpus reported it as a disagreement, which is exactly its job.

2. **The refusal case was not a refusal.** `a stranger may not load a
   relationship` was written as a `children` read, and `stranger` *is* granted
   read on `sales` — only `books` and `authors` are denied. So all three
   returned rows, agreed, and the case passed while asserting nothing about
   permission. Now a `parents` read, which goes through `books`. This is the
   failure mode `EXPECTED_REFUSALS` exists for and it still slipped past,
   because the list checks that a refusal case *is* refused and this one was
   listed as expected-to-refuse while returning data — the list had been
   written from the case's name rather than its behaviour.

Mutation run against the corpus itself, three mutations of the Go adapter:

| mutation | caught by |
| --- | --- |
| drops the empty groups | *a book with no sales…*, *a reader's books…* |
| deduplicates the caller's keys | *a repeated key*, *the books behind several sales* |
| always reads children | *the books behind several sales*, *a stranger may not…* |

The dedup mutation reported as a **survivor on its first run and was not one**:
it compared `fmt.Sprint(keys[len(keys)-1])`, which is `[7]`, against
`fmt.Sprint(value)`, which is `7`, so the branch never fired and the adapter
was unmodified. Rewritten to compare `had[0]`, it dies to two cases. That is
the third time in two days a mutation run has produced a confident wrong
answer through a defective mutation rather than a defective test — twice from a
first-occurrence replace hitting the wrong site, once from this. **A mutation
that survives is a claim about the tests and should be read as a claim about
the mutation first.**

## What this does not do

The demo's UI has no relationship view. The endpoint exists and the corpus
exercises it; a visitor to the demo sees nothing about relations.

The corpus compares the three clients to each other and not to a third
implementation. Three clients agreeing is not three clients being right — it is
the property that a caller can switch SDKs, which is what it claims.

`through` being settable is a hole in the argument that the adapters should not
parse a language. It is one string with one non-default value used by one case,
and it is there because the alternative was not comparing the refusals at all.

Nothing measures the read count here. Go and Python assert it in their own
suites, at the transport; the corpus compares answers and an answer cannot tell
one read from fifty.
