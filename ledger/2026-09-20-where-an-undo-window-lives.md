# Examining the undo-window question instead of leaving it open

- **Date:** 2026-09-20
- **Author:** Claude, on the one item of three I had called "not mine to close"
- **Touches:** `docs/undo-window.md` (new), `examples/retention/head.toml`, `crates/slate-kernel/tests/soft_delete.rs`
- **Kind:** docs

## What changed

A design note, a test that makes half of it a measurement rather than a claim,
and a comment in the retention example pointing at both.

## Why

I closed three entries today saying the undo-window question — role, time-bounded
grant, or audited operator action — was a product decision I should not make
alone. That is true and it is not the same as *nothing left to do*. "Nobody has
examined this" is a gap; deciding is not the only way to close it, and the
repository already treats an examined question as a deliverable
(`docs/validation.md`, `docs/topology.md`).

Examining it turned up two things I could not have written from the armchair.

**A time-bounded window is already expressible, and I proved it by building
one.** `PolicyPredicate::build` is called per request with the context, so a
policy may read a clock and mean something different tomorrow.
`an_undo_window_can_be_a_policy_rather_than_a_role` builds exactly that and
watches the window close under a row.

**And writing it found a trap that is the real content of the note.**
`row_filter_with` fails closed — *"RLS on with nothing admitting anything means
no rows, not all rows"* — so a policy scoped to `Action::Update` alone turns
`Read`, `Insert` and `Delete` on that table into deny-all for every
non-superuser. My first draft did that, and the symptom was not the window
failing: **the `delete` that was supposed to retire the row silently did
nothing**, so the row under test was never retired and the "window has closed"
assertion found a live row it was happy to update. I spent the debugging on the
window and the fault was two statements earlier.

That is worth a note on its own. Anybody reaching for a policy to express a
window will write the `Update` one first, and the failure will not point at it.

**The third shape is the one nothing supports**, and the reason is a decision
made deliberately this morning: restoring is an ordinary `update`. The daemon
names statements `insert`, `update`, `delete` — a restore is invisible in the
per-request log and in `/metrics`. `ledger/2026-09-20-the-write-that-names-a-key.md`
rejected a `restore` verb on ergonomic grounds; **auditability is a better
argument for it than ergonomics ever was**, and the note says so rather than
leaving the rejection looking settled.

## Alternatives rejected

**Decide it.** The shapes differ in what a deployment's regulator, product and
operators want, none of which this repository knows. Picking one and writing it
into the kernel would make the choice by making the alternative expensive.

**Say nothing, since the example works.** The caveat was already written down
three times. A caveat repeated is not a caveat examined, and the two findings
above were sitting behind an afternoon's work.

**Build shape 2 into the config language** — a time function in
`[[security.policies]]`, or a declarative window on the table. It is the obvious
next step and it is a feature, chosen on behalf of a user who has not asked.
The note states the gap precisely enough that building it later is a small
decision rather than a rediscovery.

**Put the note's content in the example's TOML comments.** That file is already
the longest-commented config here, and a reader configuring a retention job does
not want three design options — they want the one that works, and a pointer.
The comment is four lines and a link.

## Evidence

`an_undo_window_can_be_a_policy_rather_than_a_role` passes: a row retired inside
the hour is restored, the clock moves past the window, the same update is
refused with `RowNotFound`, and an ordinary update of a *live* row still works
— the last being the half that a window written as `deleted_at > floor` alone
gets wrong, because a null stamp compares unknown against any bound.

The trap is evidenced by the draft that hit it, quoted in the test's own
comment: with no companion policy the delete returned without retiring
anything. The committed test asserts `delete` returned `true` precisely so that
a future edit removing the companion policy fails there rather than somewhere
confusing.

47 tests in `soft_delete`, all green. `python3 site/check/docs.py` passes — the
new page's relative links resolve.

**Not measured:** nothing here has a performance claim.

## What this does not do

**It does not implement any of the three.** Shape 1 already exists; shapes 2 and
3 are described with their costs and left.

**The policy demonstration is a kernel test, not a supported feature.** A
deployment reaching for it has to build its own `Catalog` and
`SecurityCatalog` in Rust. `slate-serverd` users cannot express it at all, which
the note says plainly and which nothing in this change improves.

**No audit trail, not even the cheap half.** Counting restores —
`slate_rows_written_total{statement="restore"}` — is described in the note
rather than built. The note first called that counter "small"; it is not, and
`ledger/2026-09-20-the-counter-that-is-not-one-line.md` records why.

**The note is one person's reading of three options.** It has no user research
behind it, no survey of what comparable systems do — `docs/orm-comparison.md`
does that kind of work elsewhere and this note does not — and its "honest
summary" is a summary of this codebase's capabilities, not advice.
