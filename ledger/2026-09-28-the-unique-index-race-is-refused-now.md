# The rolling-deploy unique-index race is refused, not just written down

- **Date:** 2026-09-28
- **Author:** Claude, continuing the pass over what the caveat tracker still listed
- **Touches:** `crates/slate-kernel/src/migrate.rs`, `crates/slate-kernel/tests/migrations.rs`, `crates/slate-serverd/src/main.rs`
- **Kind:** fix

## What changed

Adding a **unique** index to a table the keyspace already knows now raises
`Refusal::SoleWriterNotEstablished`, so the plan is blocked and `apply` does
nothing. A caller that can establish nothing else is writing clears it with
`MigrationPlan::with_sole_writer()`, which drops that refusal and no other.

`slate-serverd` clears it, at `reconcile` and at both `--plan` preview sites.

## Why

`ledger/2026-09-27-the-last-twenty-six-and-a-hazard-nobody-had-written-down.md`
found the hazard and wrote it into the module docs, ending: *"Nothing enforces
that, which is why it is written here rather than assumed."* Its recorded
residual said so too. A hazard whose only defence is a paragraph is a hazard
that reaches whoever did not read the paragraph.

The failure is silent and permanent. The backfill's uniqueness check and the
write path's are separate reads, so two rows colliding on a new unique key can
each pass their own and both be written — leaving an index that names one row
and hides the other. Nothing errors, and a later reader gets one row where
there are two.

## Alternatives rejected

**A lease inside the kernel.** The obvious fix and the one the original note
weighed: have the backfill hold something the other writer respects. Rejected
for the reason that note gave — a lease lives in the storage layer, so giving
`migrate` one inverts the layering the rest of the crate keeps, and would make
the kernel depend on whichever backend supplies it. What is new here is the
third option that note did not reach: the kernel does not need a lease to
refuse to *guess*. It asks, and the caller that already holds one answers.

**Build-then-validate: a second pass over the finished index.** Still the more
complete answer, and still not taken. It is the only shape that lets the build
proceed *concurrently* rather than requiring a single writer, which is what a
large table actually wants. It is also a second full pass, a new step kind, and
a decision about what to do when validation fails — a change several times this
one's size. What is bought here is that the unsafe case now stops, which the
larger change would also have had to do first.

**Refuse only when the table is non-empty.** Narrower, and wrong: "is it empty"
is a read that races the very writer the refusal is about, so the check's
answer can change underneath it. The rule is about a shape, not a row count.

**Refuse unique index builds everywhere, including a table the keyspace has
never seen.** Simpler to state. Rejected because it fires on every fresh
install of a schema with a unique index, where there is nothing to backfill and
no older binary has ever written. A guard that trips on the case it was never
about is a guard somebody turns off.

**A boolean parameter on `plan`.** Would have touched 48 call sites, 44 of them
in one test file, for a mechanical edit of the kind `CLAUDE.md` warns re-reading
is the only defence against. Putting it on the plan instead reuses `is_blocked`,
`why_blocked` and `apply`'s existing refusal, and changed one existing test —
the one that adds a unique index to a live table, which is exactly the test that
ought to have to say it is the only writer.

## Evidence

Five tests, written before the implementation and red against it:
`adding_a_unique_index_to_a_live_table_refuses_until_sole_writership_is_claimed`,
`claiming_sole_writership_clears_that_refusal_and_nothing_else`,
`claiming_sole_writership_does_not_clear_a_layout_refusal`,
`a_non_unique_index_needs_no_such_claim`, and
`a_unique_index_on_a_table_the_keyspace_has_never_seen_needs_no_claim`.

**Five mutations, all caught** —
`ledger/mutations/20260928T183008-crates-slate-kernel-src-migrate-rs.json` and
`ledger/mutations/20260928T183040-crates-slate-kernel-src-migrate-rs.json`:

| mutation | caught by |
|---|---|
| the refusal fires for a non-unique index too | `a_non_unique_index_needs_no_such_claim` + 2 |
| the refusal never fires | `adding_a_unique_index_to_a_live_table_refuses_until_sole_writership_is_claimed` |
| `with_sole_writer` clears every refusal | `claiming_sole_writership_does_not_clear_a_layout_refusal` |
| `with_sole_writer` clears nothing | `claiming_sole_writership_clears_that_refusal_and_nothing_else` + 1 |
| a never-seen table's unique index is refused too | `a_unique_index_on_a_table_the_keyspace_has_never_seen_needs_no_claim` |

The third is the one worth having: an escape hatch that grew to cover every
refusal is how the next one gets waved through, and it is a one-character edit
away.

`cargo test -p slate-kernel --test migrations` — 35 pass (30 before).
`slate-serverd` builds and its own tests pass.

**One existing test changed**, and the change is the finding restated:
`building_a_unique_index_over_duplicates_refuses_instead_of_hiding_a_row`
called `migrate::migrate` and now goes through
`plan().with_sole_writer()` + `apply()`. It is one thread and one
`MemoryStore`, so the claim is true; without it the plan stops at the new
refusal and the duplicate-collision guard it exists for never runs.

## What this does not do

**It does not make a concurrent unique-index build safe.** It makes an unsafe
one stop. A deployment that genuinely needs the index built while another node
writes still has no way to do it, and build-then-validate is still the change
that would give it one.

**`slate-serverd` clears the refusal, so its deployment path is unchanged.**
That is deliberate — `reconcile` runs only on the node that won the leadership
campaign and opened the writer, and opening a SlateDB writer fences the
previous one. But it means the guard defends the *library* caller and not the
daemon, and the daemon's safety still rests where it did: on SlateDB's fencing,
which nothing in this change tests. A fencing window, if one exists, is
untouched by this and would not be visible to any test here.

**Nothing checks that a future caller's `with_sole_writer()` is honest.** It is
an assertion, and a caller that makes it wrongly gets exactly the race this
refuses. What changed is that making it is now a deliberate line of code with a
name, rather than the default.

**The `--plan` preview clears it too**, so an operator previewing a deploy is
not shown the hazard. That follows from the preview having to describe what the
run will do, and it means `--plan` is not where someone learns about this. The
module docs are.
