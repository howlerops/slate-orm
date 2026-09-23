# A view over a view was refused by accident. It is refused on purpose now, and the message says which.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #263 (F5i)
- **Touches:** `crates/slate-serverd/src/views.rs`, `crates/slate-serverd/tests/refusals.rs`, `docs/views.md`, one ledger entry
- **Kind:** decision, with the refusal that enforces it

## What changed

`docs/views.md` gains §5: a view over a view is **Refused**, with the argument.
`views::views` enforces it before the parser is asked, by name, in both
declaration orders and for a view that reads itself.

## Why

The step-2 entry recorded the state honestly and that is what makes it worth
fixing: *"Not argued for, just not built."* A view naming another view failed
because the parser resolves names against the catalog and the view is not in
it — so the operator read **"no table named `big`"** about a name they had
declared twelve lines above.

That is the exact shape task F4 refused to leave CTEs in. A limit nobody
decided is a limit reported by an error that describes it wrongly, and a reader
cannot tell a decision from an omission — which means they cannot tell whether
to work around it or wait for it.

**The decision is to refuse**, and the argument is asymmetry. What nesting
would buy is `WHERE a AND b`, which one view already says: a view here is a
name for a `WHERE` over one table (§1), so there is no projection to inherit,
no join to flatten, no ordering to compose. What it would cost is that the base
table stops being a field and becomes a traversal — with a cycle check and a
depth bound whose failure mode is a server that will not finish starting — and
that the property every client relies on, *a view's declaration is its base
table's under the view's name*, is stated about a base table that would no
longer be one hop away. `--print-schema`'s `views` key becomes a graph;
`codegen.py` resolves transitively; so does every client.

## Alternatives rejected

**Build it.** Twenty lines for the composition and a real design change for
everything downstream, in exchange for an `AND`. The step-2 entry's own summary
— "the composition would be an `and` of two predicates and the loop would need
a cycle check" — describes the cheap half and stops before the expensive one.

**Leave it, and only fix the message.** Tempting, and it is most of the value.
Refused because a message that says "a view over a view is not supported"
without a decision behind it is the same omission with better wording: the next
reader still cannot tell whether to wait.

**Detect it after the parse fails, by reading the parser's error.** The failure
is "no table named `big` — this database has …", and recovering `big` from that
means parsing an error message. `slate_sql::Schema` is a concrete type whose
lookup is private, so there is nothing to observe the resolution through, and
a wrapper is not available.

**Parse against a catalog padded with placeholder tables for the views.** Gets
past name resolution and then fails on the *columns* instead, which is a worse
message than the one being replaced.

**Refuse only views declared earlier in the list.** The obvious loop, and it
accepts `bigger` reading `big` when `bigger` is declared first. The check reads
the whole list; the test asserts both orders, and a mutation that narrowed it
to the already-resolved map is what the second half of that test exists for.

## Evidence

`from_table` is a ten-line scan for the word after the first `FROM`, and it is
**deliberately allowed to be wrong**: a `from` inside a string literal in the
select list would scan to the wrong word, and the only consequence is falling
through to the parser's own message, because the scanned word is used for
nothing but an equality test against a declared view's name. It can make a
refusal more specific; it cannot make one wrong. That is the property that made
a ten-line scanner acceptable where a ten-line parser would not have been.

`cargo test -p slate-serverd --test refusals`: 51 passed, three new assertions
across two tests — a view over a view in each declaration order, and a view
reading itself.

Mutations via `scripts/mutate.py`, four, each caught by a named test: the
self-reference check disabled, the `FROM` scanner never matching (caught by
both tests), the sibling lookup matching nothing, and the lookup comparing
against emptiness rather than the scanned name.

An earlier mutation run reported `NOTHING RAN` four times with `linking with
\`cc\` failed` — ENOSPC, which `CLAUDE.md` names as the usual cause and which
the script refuses to score rather than reading as four survivors. It also
reported "the tree did not come back clean", which was its own final
verification failing on the same full disk rather than a mutation left behind;
`git status` and a re-read of the file confirmed the restore. Worth recording
because "the restore failed" is the one message from that tool that should
never be shrugged at, and this time it was the disk.

## What this does not do

**It does not detect a cycle longer than one.** There is nothing to cycle
through: a view may not name a view, so the graph has depth one by
construction. If §5 is ever reversed, the cycle check arrives with it and this
scan goes away.

**It does not refuse a view whose `FROM` is a view in a statement the scanner
mis-reads.** The fall-through message is then the parser's — "no table named
`big`" — which is the message this change exists to remove. Constructing such a
statement requires a string literal containing `from` *before* the real `FROM`,
in a select list that a projection refusal would reject anyway, so the case is
believed unreachable and is not tested.

**Nothing checks that §5 and the code still agree.** The refusal cites
`docs/views.md` §5 in its message, and `scripts/check_cited_docs.py` checks
that cited documents exist — not that a section number still points at the
argument it did. That guard is one this repository has and this use is outside
it.
