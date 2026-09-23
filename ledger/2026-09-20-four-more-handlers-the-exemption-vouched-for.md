# The exemption said its callers authorised first; four of six did not

- **Date:** 2026-09-20
- **Author:** Claude, checking a claim written into a guard an hour earlier
- **Touches:** `crates/slate-server/src/service.rs`, `crates/slate-server/tests/security_probe.rs`, `scripts/check_handlers.py`, `docs/security-review.md`, `CLAUDE.md`
- **Kind:** security

## What changed

`join`, `explain_join`, `aggregate` and `explain_aggregate` authorise every
table their request names before converting it. Two new helpers read the names
off the wire, because the converted read is the thing that cannot be produced
safely yet. Four tests. The exemption whose false claim started this now says
what is true and that it once did not.

## Why

`check_handlers.py` exempts `query_from_proto_at` from the authorise-first
rule, and the exemption carries a reason — deliberately, because the list is
meant to force re-argument. The reason said:

> its callers are `query`, `explain` and the join and chain handlers, each of
> which authorises before converting

I wrote that from reading. **Four of the six callers did not.**
`join_from_proto` and `aggregate_from_proto_query` take a `Catalog`, not a
`SecurityContext` — so they cannot authorise — and the handlers called them
before authorising anything. Measured on `join`, from a role granted nothing:

```
real   = "access denied: no role grants read on table `users`"
absent = "the projection names column 99 of table `users`, which has 4 columns"
```

The same oracle `query` had this afternoon, on four more RPCs. That is
**seven handlers, in three passes, for one finding** — and the third pass was
caused by the guard I wrote to stop the second from recurring, because the
guard's exemption was a claim I had not checked.

The uncomfortable part is not the defect, it is where it came from. This
session's repeated finding is "a fix inherits the scope of the finding it was
written for", and I wrote that sentence into a ledger entry, built a check for
it, and then vouched for six callers from reading in the same file. The list
being written down is what made it falsifiable an hour later; the list being
*correct* was never checked by anything.

## Alternatives rejected

**Authorise inside `join_from_proto`.** Where the tables are actually
resolved, and it would cover any future caller at once. It needs a
`SecurityContext` threaded into a pure conversion function, and the action
differs per handler — `Read` for `join`, `Explain` for `explain_join` — which
is the hazard `each_handler_authorizes_the_action_it_performs` exists for. The
same argument rejected the same idea for `query_from_proto` this afternoon.

**Authorise the tables `join_from_proto` returns, after converting.** Reads
better and is useless: the conversion is what discloses, so a check after it
protects nothing.

**Authorise only the first input.** Not seriously considered, but a mutation
that made it so **survived** until a test covered it — which means it was
untested rather than rejected. `reader_only` holds `Read` on `users` and
nothing on `docs`, and without the second check it could read `docs`'s width
through a join. A caller who can read *some* table is a much lower bar than
one who can read none.

**Skip the aggregate handlers, since `join` was the demonstrated one.** The
mutation that skipped the aggregate's `join` arm also survived. Both aggregate
RPCs reach `join_from_proto` through `aggregate_from_proto_query`, so fixing
`join` alone leaves the identical oracle one RPC away — which is this entry's
whole subject, committed again in miniature.

**Make the guard catch this shape rather than fixing the four handlers.** Not
either/or, and ~~the guard cannot: `join_from_proto` calls neither
`self.table(..)` nor `fingerprint::check` directly. Catching it would mean
following calls across functions, which is a parser and a call graph.~~ The
exemption's corrected reason is what carries this now, and the entry says
plainly that a reason is not a check.

> **Withdrawn, same day.** The guard can, and now does. No call graph was
> needed: a converter that resolves a request's tables is recognisable by its
> signature — a `&pb::` request and a `&Catalog` and no `SecurityContext` — so
> one pass derives the names and a second holds every call to them to an
> authorisation. What made this look like a parsing problem was stating it as
> "follows calls across functions" rather than as "which functions cannot
> check for themselves", and the second question is answerable locally. Four
> of the six callers this entry's exemption vouched for are now held by a
> check rather than by my reading. See
> `2026-09-20-taking-a-catalog-was-not-the-hazard.md`, which also records the
> two wrong criteria it took to get there.

## Evidence

**The disclosure, before the fix**, quoted above. The probe carries a control
assertion — that the *in-range* request is refused for the grant — because the
first version of it did not, and passed:

```
real="a join needs at least two inputs, and this one has 1; a single-table
      read is a Query"   absent=(the same)
```

A one-input join is refused by arity before anything converts. The probe could
not reach the code it was about and said "indistinguishable", correctly and
uselessly. It has two inputs now, and asserts it reached conversion.

**Three mutations, two of which survived the first run** and were real missing
tests rather than redundancy:

```
ok  join converts before authorising, as before  -> joining_..., every_input_...
!!  only the first input of a join is authorised: SURVIVED
!!  the aggregate's join arm is skipped:          SURVIVED
```

Both have tests now and all three are caught.

**Nothing over-refuses.** `cargo test -p slate-server -p slate-serverd
--no-fail-fast`: 32 suites, green. `clients/python` against a rebuilt
`slate-testserver`: **295 passed**. `clients/go` with `-count=1` against a
rebuilt `slate-serverd`: ok, 7.5s. Both client suites exercise joins and
aggregates.

**The Python suite refused to run first, which is a check working.** The
binary predated the `service.rs` edit and the harness said so by name rather
than testing a server this tree did not produce — the guard from
`Refuse a prebuilt server binary older than the source it was built from`,
earning its keep on me.

`scripts/check.sh` 25/25, `cargo fmt --all -- --check` and
`cargo clippy -p slate-server --all-targets` clean.

## What this does not do

**The corrected exemption is still a claim about six callers.** It is
accurate today, checked by grep and by four tests that exercise four of the
six. Nothing verifies it as the callers change; a seventh caller added
tomorrow is vouched for by a sentence.

**Only `join` and `aggregate` are probed; their `explain` twins are not.**
`explain_join` and `explain_aggregate` take the same helpers with
`Action::Explain`, and I asserted that by reading — which is the sentence this
entry exists to be embarrassed by. Two more tests would close it and I stopped
at the two that demonstrate the class.

**`Action::Explain` on the explain handlers is right for today's kernel**, as
the single-table `explain` note already says: it checks `Explain` then `Read`.
Nothing ties the two orderings together, and the difference is only which
action a refusal names.

**The chain handlers were assumed to be the join handlers.** A chain is a
`JoinQuery` with more than two inputs, so `join_from_proto` handles both and
the fix covers both — read from the converter, not demonstrated with a
three-input request.
