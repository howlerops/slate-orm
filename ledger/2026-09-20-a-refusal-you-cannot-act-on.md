# The `RESTRICT` refusal now says the blockers are retired

- **Date:** 2026-09-20
- **Author:** Claude, on a defect introduced by this morning's own fix
- **Touches:** `crates/slate-schema/src/error.rs`, `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** fix

## What changed

`SchemaError::ForeignKeyRestricted` carries `retired`, and its message changes
when every blocking child is one a soft delete retired: it says "soft-deleted
rows", and it says what to do about them. `deletion_closure` gathers the
blockers before refusing rather than returning on the first, which is what makes
"every one of them" answerable.

## Why

**This is a defect I introduced this morning and reported as a caveat instead
of a bug.** `ledger/2026-09-20-a-child-that-is-still-a-child.md` made a
`RESTRICT` edge block on a retired child — correctly — and closed with:

> **It is a behaviour change, and a deployment can feel it.** A schema that
> soft-deletes its children and hard-deletes its parents now finds the parent
> undeletable until the children are purged…

What that framing missed is what the operator actually experiences. They delete
a parent and are told *"rows in `files` still reference it through foreign key
`files_folder`"*. They query `files`. **An ordinary read hides retired rows, so
they find nothing referencing that parent.** The message and the database now
contradict each other, and the reasonable conclusion is that the constraint is
broken.

The two cases also need opposite responses, which a single message cannot give:
a live child is deleted, a retired one is *purged* — the operation that destroys
history and that nobody reaches for by accident. Telling somebody to purge when
a plain delete would do is worse than saying nothing.

This is the same shape as `SoftDeleteColumnSupplied`, added hours earlier for
the same reason: an error that sends the reader somewhere the problem is not.
That one was found by attempting the operation; this one was in a "what this
does not do" section labelled as a deployment note, which is where it would have
stayed.

## Alternatives rejected

**Name the blocking rows' keys in the message.** The most useful thing the error
could say, and it is an existence oracle: the referencing search deliberately
ignores row-level security, so the blockers include rows the caller's policy
hides. `delete`'s own doc comment already bounds what that search may disclose —
"a caller who may delete a parent can learn from a `RESTRICT` refusal that
*something* references it" — and "something" is exactly as far as it goes. A
count would leak less and still leak; *which kind* leaks nothing new, because a
caller who may delete the parent already learns that something blocks.

**Decide per row and report the first blocker's kind.** Cheaper — no gathering —
and wrong on a mixture, where it would depend on scan order. The advice would be
"purge" or "delete" depending on which row the index happened to return first,
which is the least defensible kind of nondeterminism: correct-looking and
irreproducible.

**A separate error variant for the retired case.** It would let the daemon give
it its own reason token, which is a real benefit. Rejected because the *code* is
the same refusal for the same reason — a constraint held — and splitting it
would make every caller match two variants where they now match one, to
distinguish something only a human reading the message acts on.

**Leave it and document the behaviour in `docs/`.** The caveat was already
written down. Documentation is not where somebody looks when the database has
just told them something they can see is false.

## Evidence

Three tests, one per case, and each is a control for the others:

- `a_restrict_edge_blocks_on_a_retired_child` — asserts `soft-deleted` and
  `purge it` are in the message.
- `a_restrict_edge_blocks_on_a_live_child` — asserts `soft-deleted` is **not**,
  so the advice cannot be wrong in the commoner direction.
- `a_mixture_of_live_and_retired_blockers_reports_as_the_ordinary_case` — one
  live child and one retired child on the same parent, reporting as ordinary.

Three mutations, each caught by name:

| mutation | tests that failed |
| --- | --- |
| `retired` is always true | `a_mixture_...`, `a_restrict_edge_blocks_on_a_live_child` |
| `retired` is always false | `a_restrict_edge_blocks_on_a_retired_child` |
| `any` blocker retired rather than `all` | `a_mixture_...` |

The third is the one the mixture test exists for: `any` reads perfectly
sensibly and is wrong, because the live child is the one the caller can see and
the one they should deal with first.

**Suites**: `slate-kernel`, `slate-schema`, `slate-server`, `slate-orm` and
`slate-serverd` pass; `cargo clippy --workspace --all-targets` clean;
`scripts/check.sh` 20/20.

## What this does not do

**The three clients say nothing new.** The reason token is unchanged —
deliberately, per the rejected alternative — so a client that renders the token
rather than the message shows exactly what it did before. Only the prose
changed, which means only a human reading it benefits.

**There is still no way to inventory what is blocked.** An operator upgrading
into this behaviour cannot ask "which parents are now undeletable because of
retired children" without writing the join themselves against an
`include_deleted` read. That is the tool this change makes obviously missing,
and it is not here.

**`retired` is computed only for the edge that refused.** A parent blocked by
two different children through two different foreign keys reports the first
constraint that fires, as it always has, and says nothing about the other.

**No test asserts the message for a child that does not soft-delete at all.**
`retired` is `false` by construction there — `soft_delete().is_some_and(..)` —
and the live-child control covers a soft-deleting table with a live row, which
is the case that could go wrong. The non-soft-deleting table cannot.
