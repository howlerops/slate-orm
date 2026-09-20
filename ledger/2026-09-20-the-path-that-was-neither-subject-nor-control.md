# Finding 2 was open on the single-row upsert, which was neither the finding nor its control

- **Date:** 2026-09-20
- **Author:** Claude, working the "re-examine findings 2 through 8" caveat
- **Touches:** `crates/slate-kernel/src/record.rs`, `crates/slate-kernel/tests/security_probe_cascade.rs`, `scripts/check_write_paths.py`, `scripts/test_check_write_paths.py`, `scripts/check.sh`, `docs/security-review.md`
- **Kind:** security

## What changed

`RecordTransaction::upsert` decides the row policy before it reads the key,
which `write_many` was fixed to do and this path was not. The disclosure probe
now asks the question of **all twelve** write paths rather than the one a
finding happened to name, and `scripts/check_write_paths.py` holds its roster
to `record.rs`.

## Why

An earlier entry today left this open:

> **It does not re-examine findings 2 through 8 for the same class of bypass.**
> Finding 1's fix was guarded in one constructor of two. Whether any other
> finding's fix sits behind an opt-in call I have not checked.

Finding 2 was that `write_many` read storage *before* deciding the row policy,
so a caller could send a row carrying another tenant's key and read the tenant
boundary off which error came back. The review fixed it there and quoted
single-row `insert` as the control — the path that already did it right.

`upsert` is neither the subject nor the control, and it read the key first.
Measured, before the fix:

```
assertion `left == right` failed: the single-row upsert answers differently:
  RowNotFound { table: "users" } vs RowCheckFailed { table: "users" }
```

Key 7 is taken in tenant B; key 8 is not. Tenant A is told which is which, in
one call, with nothing written, repeatably. That is finding 2 verbatim — rated
*high, cross-tenant disclosure of primary keys and unique-index values* — on a
path the finding did not name.

**The blast radius, precisely.** Over the wire it is unreachable:
`Write::apply` routes even a one-row upsert through `upsert_many`, which is
fixed. The exposure is `RecordTransaction::upsert` and the typed ORM method
over it, `crates/slate-orm/src/ext.rs:384` — a published surface, the same
shape as `Catalog::insert` this morning.

**This is the fourth time today a fix covered one path of several**, and the
first three were found by asking "where else?" *after* fixing. So this time the
probe asks about every path at once, and a check makes the list stay complete.

## Alternatives rejected

**Fix `upsert` and move on.** Two lines and it closes the demonstrated hole.
Rejected because it is precisely the move that produced the previous three:
the catalog refusal in one constructor of two, the identity header in one
`Authenticator` of two, finding 8 on three handlers of seven. The cost of
asking the wider question once is a morning; the cost of not asking it has
been four mornings.

**Pre-check `Action::Update` instead, or the action the branch turns out to
be.** More precise, and it cannot be done: which branch this is depends on the
read, and the read is what must come after. `write_many` faced the same
question and answered `Action::Insert` on the grounds that the tenant
restriction is the same expression either way. Matching it is also the point —
two spellings of one operation that disagree about who may do it is its own
defect.

**Make the probe a `#[test]` per path.** Clearer failures, and it loses the
thing being built: the roster exists so the *set* is checked, and twelve
independent tests have no set to compare against. It is also how `upsert` was
missed — there were three tests for finding 2 and none of them was a set.

**Derive the roster from "takes a `&SecurityContext`" or "authorises a write
action".** Both are proxies and both are wrong at the edges. Every read takes a
context. `insert_many` authorises nothing itself — it hands off to
`write_many`, so an authorise-based derivation misses all three batch paths.
What makes a method a write path is that it reaches a primitive that writes,
which is one grep. That lesson came straight from the converter rule an hour
earlier, where two wrong criteria cost 47 and 59 false positives before the
right one fit exactly.

**Leave the predicate writes out of the probe.** They take no key, so the
"another tenant's key" framing does not apply. Included anyway: a predicate
names column *values* as freely as a key does, `tenant_id = B AND id = 7` is
the same probe in a different spelling, and "it reads through the policed path"
was a claim until a mutation proved it.

## Evidence

**The disclosure, before the fix**, quoted above. It is gone after it, and
eleven other paths were asked the same question at the same time and answered
identically both ways.

**Three new probes plus a control.** `no_key_naming_write_path_answers_
differently_for_another_tenants_key` drives the nine paths that take a
caller-named key or row through both cases and collects every path whose two
answers differ. `no_predicate_write_path_answers_differently_either` does the
same for `delete_where` and `update_where`.
`purging_a_table_that_does_not_soft_delete_refuses_rather_than_counting`
covers the twelfth, whose observable is a count rather than an error.

`the_same_paths_succeed_inside_the_callers_own_tenant` is the control, and it
is not decoration: if tenant A's attempts were being refused by the *grant*,
every path would answer identically for a reason unrelated to finding 2 and the
table would pass while proving nothing. Two probes were vacuous exactly that
way earlier today.

Five mutations against `record.rs`, all caught:

```
ok  upsert goes back to reading before it checks the policy   -> no_key_naming_write_path_...
ok  write_many goes back to checking the policy after its reads -> four tests
ok  the predicate writes read without the caller's policy     -> no_predicate_write_path_...
ok  purge_deleted accepts a table that does not soft delete   -> purging_a_table_that_...
```

**A sixth mutation was invalid and I wrote it, not the harness.** Adding
`#[allow(dead_code)]` to `matching_rows` changes no behaviour, so its survival
said nothing about the tests. Replaced with one that reads through a superuser
context, which is a behaviour change and is caught.

`scripts/check_write_paths.py` derives the twelve public write paths from what
each method calls and compares them to `WRITE_PATHS` in the probe, in both
directions. Seven cases over trees the test writes; five mutations, all caught
after the sixth exposed a hole in my own fixture:

```
ok  a write path missing from the roster is not reported  -> a write path missing from the roster fails
ok  a stale roster name is not reported                   -> a roster naming a path that no longer writes fails
ok  an absent roster is read as an empty one              -> write paths with no roster anywhere fails
ok  a source reaching no primitive is a pass              -> a source that reaches no primitive fails
ok  private helpers count as write paths too              -> three cases
```

**The fifth survived first time round, and the reason is worth keeping.** My
case named "a private helper that writes is machinery, not a path" had no such
helper in it: every private function in the fixture was one of the primitives,
so the `name not in PRIMITIVES` clause already excluded them and the `public`
clause was doing nothing. The case asserted its own title and tested something
else. A `stamp_and_write` helper — private, writing, not a primitive — makes it
real, and the mutation is caught.

`cargo test -p slate-kernel`: 55 binaries, none failing.
`cargo test -p slate-orm -p slate-schema`: clean. `cargo clippy --workspace
--all-targets`: zero diagnostics, after `&[row.clone()]` drew
`clone_on_copy`-adjacent advice that `-D warnings` would have turned red in CI.
`scripts/check.sh`: 27/27.

`docs/security-review.md` §2 now says FIXED *twice*, names which path was
missed and for how long, states that the wire was never exposed and why, and
records that the other eleven paths were already correct — this was one path,
not a class.

## What this does not do

**It checks eleven paths and fixes one.** The other eleven were already
correct, which the table now demonstrates rather than asserts — but "correct"
here means only "the two answers are indistinguishable for a key in another
tenant". Finding 4's question, whether a write discloses a row hidden by RLS
*within* the caller's tenant, is a different question and the table does not
ask it.

**One hop, not a call graph.** `check_write_paths.py` treats `write_many` and
`remove_row` as primitives so that `insert_many` is seen to write. A public
method reaching a primitive through *two* private helpers would be invisible to
it. There is no such method today; there is also nothing that would tell me
when there is, beyond this sentence.

**The criterion is the calls, not the effect.** A future write path that went
straight to the `KvTransaction` rather than through `write_row` would write and
not be rostered. That would be a strange thing to do — index maintenance lives
in `write_row_with` — but it is the gap, and the never-fires branch only
catches the case where *all* of them stop matching.

~~**Findings 3, 5, 6, 7 and 8 were not re-examined here.** The caveat asked
about 2 through 8 and this closes 2. 1 and 6 were done earlier today, and 8 has
a guard as of an hour ago. 3, 5 and 7 remain judged rather than probed for this
particular class, and I am recording that rather than implying a sweep I did
not run.~~

> **Done in the next commit.** `2026-09-20-where-else-does-this-live.md` asks
> the question of every finding and records the answer per row in
> `docs/security-review.md`. 3 and 5 turned up no second path; 3 and 4 are
> marked there as enumerated by reading rather than probed, which is the part
> of that sweep still worth distrusting.

**The performance cost of the extra check was not measured.** `upsert` now runs
`check_row` on every call, including ones that would have passed anyway. It is
an expression evaluation against one row with no I/O, against a path that does
a storage read, so I judged it irrelevant. That is a judgement, not a
measurement.
