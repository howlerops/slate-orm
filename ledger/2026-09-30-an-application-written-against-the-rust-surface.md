# An application written against the Rust surface, and the three things it could not say

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `examples/helpdesk/`, `Cargo.toml`
- **Kind:** feature

## What changed

`examples/helpdesk` — a small multi-tenant helpdesk written against
`slate-orm` the way somebody building on it would write one. Four tables
(tenants, agents, tickets, comments), a service layer that takes a
`SecurityContext` and returns domain types, and eight tests that drive it.
A workspace member, so `cargo test --workspace` builds it.

This is the first slice. It has the domain, the security catalog, and the
operations an application actually performs: raise a ticket, read it by the
reference a customer quotes, assign it without losing a colleague's
reassignment, log billable time, comment, read a thread, close. The inbox
page, the workload roll-up, the search box and the SlateDB binary are not
written yet, and are named below.

## Why

Everything else in this repository exercises the record layer as a *test*
does. The demo goes through the head node and three SDKs; the deployed
harness loads a hundred thousand taxi trips and folds them; the oracles
compare a plan against a full scan. None of them is somebody sitting down to
write an application against the Rust surface and finding out what that is
like — and a surface is only as good as the thing you cannot say in it.

Three things could not be said, and all three have the same shape: **a
feature the database has, reachable from the TOML schema or the wire, and not
from `#[derive(Record)]`.** That asymmetry is invisible from inside the
repository because everything here that wants those features declares its
tables in TOML.

1. **No `CHECK`.** A ticket's status must be one of four words. The derive's
   field options are `pk`, `rename`, `added_in`, `scale`, `created_at`,
   `updated_at`, `soft_delete` and `index`. So the constraint lives in
   `Helpdesk::open_ticket`, and anything reaching the store directly writes
   whatever it likes — demonstrated, not argued, by
   `the_same_status_written_past_the_service_is_not_refused`, which stores
   `"opne"` and reads it back.

2. **No text index.** Index options are `name`, `id`, `unique`, `desc`,
   `columns`, `only_where`. The kernel walks an inverted index and the TOML
   schema takes `text = true`; an application wanting a search box over a
   column has to declare that table with `slate_schema::TableBuilder`
   instead, writing the column list a second time and keeping the two in
   step.

3. **Soft delete has no read.** `#[record(soft_delete)]` works and the close
   stamps rather than erases — which is the whole point of the column. But
   `Deleted` appears nowhere in `crates/slate-orm/src`: the kernel's
   `visible_row_with(..., Deleted::Visible)` is private, the wire has
   `include_deleted` as a privileged read, and the Rust surface has neither
   that nor a `restore`. **An application can close a ticket and cannot
   reopen it.** That is the sharpest of the three, because the derive offers
   the column and the half that makes it worth having is missing.

## Alternatives rejected

**Declaring the tables with `TableBuilder` instead of the derive**, which
would have given the `CHECK` and the text index immediately. Rejected because
it hides the finding: the point of writing an application against the derive
is to find out what the derive cannot say, and routing around it on the first
obstacle produces a demo rather than a report. The workaround is named in the
code and in this entry, which is where a reader who needs it will look.

**Building the app over the head node and an SDK.** That path is already
proved three ways — the demo, the conformance runner, the deployed harness.
The Rust ORM on SlateDB directly is the layer with no application over it,
and "slate-orm, using slatedb" is what was asked for.

**Faking the missing features so the app looks complete** — a status enum
validated only in the service and presented as if the schema held it, a
search implemented as a scan and called full-text. Every one would have made
the app read better and the findings disappear. The service *does* validate
the status, because an application has to; what it does not do is pretend
that is the same thing.

**Waiting until the app was finished to write this down.** Three findings in
the first hour is the useful output, and an entry written at the end would
have folded them into a feature announcement.

## Evidence

Eight tests, all passing (`cargo test -p slate-helpdesk`), and three of them
are the findings rather than the features:

- `a_status_the_application_does_not_know_is_refused` — the service refuses
  `"opne"`.
- `the_same_status_written_past_the_service_is_not_refused` — the store
  accepts it and reads it back verbatim. The pair is the finding: the rule
  exists in exactly one place and it is not the database.
- `a_closed_ticket_is_hidden_and_cannot_be_reopened_from_here` — the close
  hides the row, and a **superuser** read cannot see it either, because
  hiding a retired row is not a permission and the bypass is about
  permissions. The assertion is written so that growing an include-deleted
  read turns it red, with a message saying the finding is wrong.

The other five are ordinary application behaviour: read-back by reference,
the tenant boundary refusing a cross-tenant read with the same error a
missing reference gives, `replace_record` refusing a write decided from a row
that has since moved, a comment thread, and integer hundredths accumulating
without a lost cent.

Two smaller observations, recorded because they cost time rather than because
they are defects:

- `Records` is implemented on `RecordTransaction`, not on `RecordStore`, so
  every operation opens and commits a transaction. That is defensible and it
  is not what the `RecordStore` name suggests.
- `begin` and `commit` raise `KernelError` while every `Records` method
  raises `OrmError`, so an application's error enum carries both. There is no
  one error type spanning a transaction from open to close.

No mutation run: nothing in this crate is a guard or a check, and its tests
are the assertions. The three findings are demonstrated by tests that fail if
the surface changes, which is the mutation-shaped protection available here.

## What this does not do

**It is one slice.** The keyset-paged inbox, the per-assignee workload
roll-up, the search box, the reopen (which cannot be written), a migration
applied at boot, and the `run.sh` that runs the same service over SlateDB on
an object store are all unwritten. **Only `MemoryStore` has been run.**
`Helpdesk<S>` is generic over `KvStore` and the docstrings say *should* rather
than *does*, because a generic parameter is a claim about the surface and not
a result; nothing settles it until a binary drives this against a bucket. The
crate therefore takes no `slate-slatedb` and no non-dev `tokio` — a library
that asks for a runtime it does not use would have to be rostered in
`scripts/check_wasm_runtime.py`, and it has nothing to roster yet.

**The security catalog's two policies are `Expr::True`.** The tenant boundary
is the key prefix and is enforced below this layer; the policies are where a
real deployment would put "only tickets on your team" and there is no such
rule here. The cross-tenant test therefore proves the *prefix*, not a policy.

**Three findings are not an audit of the derive.** They are what three
ordinary requirements happened to hit. What else the derive cannot say was
not enumerated, and enumerating it means reading the macro rather than using
it — which is a different exercise and a weaker one.

**Nothing here is measured.** No number in this entry is about speed, and the
crate takes no timings.
