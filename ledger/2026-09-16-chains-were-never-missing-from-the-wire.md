# Chains were never missing from the wire, and the audit row that said so is withdrawn

- **Date:** 2026-09-16
- **Author:** Claude Code, on `claude/orm-essentials`
- **Touches:** `examples/explorer/` — all three adapters, `conformance/conformance.py`, `CONTRACT.md`; `docs/orm-comparison.md`
- **Kind:** docs

## What changed

`docs/orm-comparison.md` loses its "Chains (3+ table joins) on the wire" row and
gains a withdrawal saying why it was wrong. `/api/chain` exists in all three
demo adapters, and five conformance cases compare the three on a three-table
chain — `authors` → `books` → `sales`, all four join types, plus one as
`reader`. The corpus is 64 cases and the three agree on every one.

## Why

The audit listed chains as absent on this evidence: *"proto has `Join`, no chain
RPC; `chain` appears only in comments"*. Both halves are literally true. The
conclusion is wrong, because `JoinQuery.inputs` is `repeated JoinInput` and a
chain is a join with more inputs — `service.rs` says so directly, above the
handler that routes them: *"a join or a chain: one request shape, two kernel
paths"*. All three clients had chain tests when the audit was written:
`TestGroupingAChain`, `test_grouping_a_chain`, `"grouping a chain"`.

That is worth naming precisely, because the method that produced it is used
throughout that document and mostly works: grep for the name of a thing, and
report its absence as evidence. It fails exactly when the capability is not
named after itself. A chain has no chain RPC for the same reason a list has no
`ListOfTwo` type.

What *was* missing, underneath the wrong row: nothing compared the three SDKs on
a chain. The demo's `/api/join` built two inputs and no more, so the one place
that checks the clients against each other had never seen a third. That is the
part that is built here.

## Alternatives rejected

**A third input on `/api/join`, behind a flag.** One handler instead of two, and
the response shape would then depend on the request — `sales` present or absent
— which every adapter would have to branch on and the corpus would have to
compare across. Two handlers with fixed shapes are simpler to read and to diff.

**Leave the audit row and just add the cases.** The row would then be a false
statement sitting above a feature the same document says is built. `CLAUDE.md`
is explicit that stale documentation is worse than none, and this is the kind
that is worse: a reader deciding whether to use this would read "no 3+ table
joins" and stop.

**Silently delete the row.** Cheaper and dishonest. The withdrawal is more
useful than the row ever was, because the mistake is reusable: it is the shape
of every other `grep`-based claim in that table, several of which may be wrong
the same way.

**Compare only an inner chain.** Half the cases. The outer ones are where a
chain is hardest: `Author Unknown` has a book and no author, `Ann Leckie` has
no books, every book has a sale — so `left`, `right` and `full` each produce a
different set of nulls in a different position.

## Evidence

`./run.sh --conformance`: **64 cases, the three SDKs agree on all of them** (59
before).

Mutation run against the Go adapter, three mutations:

| mutation | caught by |
| --- | --- |
| the third input attaches to the first, not the second | all five chain cases |
| the third input is always an inner join | *a left chain*, *a full chain* |
| the third side is dropped from the answer | *a left chain*, *a full chain* |

The first mutation is the one the cases exist for — attaching `sales` to
`authors` on `authors.id = sales.book_id` is a well-typed join that returns the
wrong rows, and it is the mistake a client writing a chain by hand actually
makes. Its first form **did not compile** (`books` declared and not used), and
the harness reported neither a kill nor a survival rather than guessing; adding
`_ = books` made it compile and all five cases caught it. That is the fourth
defective mutation in two days, and the second time the "neither" branch has
been what stopped a wrong conclusion — a mutation harness needs that branch.

The two `inner`-only kills are honest: an inner chain drops every unmatched row
anyway, so a mutation that changes how unmatched rows are handled is invisible
there. The outer cases are not decoration.

## What this does not do

Nothing here changes the server, the kernel or any client. The capability was
already there; only the demo, the corpus and the document moved.

The chain cases do not cover a chain's *computed* value or a grouped chain,
both of which each client tests on its own and neither of which the corpus
compares. A chain with a `compute` reading all three inputs is the obvious next
case and is not written.

The other `grep`-based rows in that table have not been re-checked. "Batch",
"validations" and "window functions" each cite a grep returning nothing, and at
least the batch one is the same shape of argument as the row withdrawn here. I
have not verified them, and say so rather than implying the table is now sound.
