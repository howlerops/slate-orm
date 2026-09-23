# Where an undo window lives

`ledger/2026-09-20-the-write-that-names-a-key.md` made a soft-deleted row
restorable, and `examples/retention/head.toml` gives the capability to a role
called `undo` holding `read`, `update` and `read_deleted`. That example works
and it is one answer to a question nobody had asked: **who may take a delete
back, and for how long?**

This note is the question examined, not answered. The decision belongs to
whoever deploys this. What follows is what each shape costs *in this system*,
with the parts that are already possible separated from the parts that are not.

## The three shapes

### 1. A role

What `examples/retention` does. A grant of `read`, `update` and `read_deleted`
on the table, held by whoever operates the undo.

**What it costs:** nothing to build — it exists. The capability is as durable as
the grant, so "undo" is a thing a principal *is*, not a thing they may do for a
while.

**What it cannot say:** *when*. A row retired two years ago is as restorable as
one retired this morning, right up until a purge erases it. If the retention
window and the undo window are meant to be different lengths — and they usually
are; you keep records longer than you let people change their minds — a role
cannot express the difference.

**Who can already hold it:** notably, *neither of the other two roles in that
example*. The application has `update` and not `read_deleted`; the retention job
has `read_deleted` and not `update`. That is not an accident of the example: an
application that can un-delete anything it deleted has no retention policy, only
a convention.

### 2. A time-bounded policy

**This is expressible today, and it is demonstrated rather than asserted:**
`an_undo_window_can_be_a_policy_rather_than_a_role` in
`crates/slate-kernel/tests/soft_delete.rs` builds one and watches the window
close.

`PolicyPredicate::build` is called per request with the caller's context, so a
policy is free to read a clock:

```rust
Policy::new("recent_enough_to_undo", DOCS, [Action::Update], move |_ctx| {
    let floor = clock.now() - WINDOW;
    Expr::Or(vec![
        Expr::IsNull { column: DELETED_AT, negated: false },   // live rows
        Expr::compare(DELETED_AT, CmpOp::Gt, Value::I64(floor)),
    ])
})
```

**What it costs:** two things that are easy to get wrong and one that is not
available.

- **The live-row arm is mandatory.** A stamp of null compares *unknown* against
  any bound, so `deleted_at > floor` alone refuses every ordinary update on the
  table. The window has to admit live rows explicitly.
- **You must declare policies for the other actions too.** `row_filter_with`
  fails closed — *"RLS on with nothing admitting anything means no rows, not all
  rows"* — so a policy scoped to `Update` alone turns `Read`, `Insert` and
  `Delete` on that table into deny-all for every non-superuser. The first draft
  of the test above did exactly this, and what broke was not the window: the
  `delete` that was supposed to retire the row silently did nothing. Failing
  closed means this is safe, not that it is obvious.
- **It is Rust, not configuration.** `[[security.policies]]` in a TOML config
  takes a static `using = "..."` string with `:principal` substitution and has
  no clock. A time-bounded window is available to a deployment that builds its
  own catalog and not to one configuring `slate-serverd`. Closing that gap means
  either a time function in the config's predicate language or a declarative
  window on the table — both are real changes, neither is started.

### 3. An audited operator action

A restore that is recorded as a restore: who, which row, when, and ideally why.

**What it costs: this one is not available at all**, and the reason is the
design that makes restore simple. **Restoring is an ordinary `update`.** The
daemon names statements `insert`, `update` and `delete`; a restore is an
`update` and is indistinguishable in the per-request log and in `/metrics` from
any other write. Nothing distinguishes "this update cleared a soft-delete stamp"
from "this update changed a title".

Getting there needs one of:

- **A distinct statement on the wire.** `restore(table, key)`. It buys the audit
  trail and it is the alternative
  `ledger/2026-09-20-the-write-that-names-a-key.md` rejected: a second spelling
  of an update is a second write path to test against policies, checks, foreign
  keys, unique slots and partial indexes. That rejection was about ergonomics;
  auditability is a better argument for it than ergonomics ever was, and it is
  the one reason to revisit it.
- **Noticing the transition server-side.** `write_many` already reads the
  previous row, so "the stamp went from set to null" is known where the write
  happens — but **not where the counter is**. `WriteObserver` lives in
  `slate-server` and is fed by a `Tally` that names statements from the
  daemon's own arms: it knows `update`, and it cannot know that a particular
  update cleared a stamp. Surfacing it means the kernel returning the count —
  `write_many` is `Result<()>`, as are `insert_many`, `upsert_many` and
  `update_many` — which is a public API change in `slate-orm` and a change in
  `slate-server` to carry it. Not the one-line counter it looks like.

  (An earlier draft of this note called it "small". That was written without
  checking which crate the observer is in, and it is wrong: a restore counter
  is a two-crate change. A full audit trail with the principal and a reason is
  a further step again.)

## What this system does not have, in any of the three

- **No inventory.** Nothing answers "which retired rows are still restorable" or
  "which parents are undeletable because of retired children". Both are joins a
  caller can write against an `include_deleted` read; neither is a tool.
- **No expiry other than the purge.** A row is restorable until `purge_deleted`
  erases it. If the undo window is shorter than the retention window, only shape
  2 expresses the difference, and only in Rust.
- **No record that a restore happened**, per shape 3.

## The honest summary

Shape 1 works now and says nothing about time. Shape 2 says everything about
time, works now in Rust, is not reachable from a config file, and has two traps
that are both fail-closed and neither obvious. Shape 3 is the one a regulated
deployment would ask for first and is the one nothing here supports — because
restore was deliberately built as an ordinary update, and that decision is worth
revisiting only for auditability, not for the reasons it was first questioned.

Nothing here recommends one. The `undo` role in `examples/retention` is shape 1
because it is the shape that exists, and the comment in that file says so rather
than implying a judgement.
