# Finding 8 was fixed on the write paths, and the read paths gave up more

- **Date:** 2026-09-20
- **Author:** Claude, probing finding 8 after deferring it in two consecutive entries
- **Touches:** `crates/slate-server/src/service.rs`, `crates/slate-server/tests/security_probe.rs`, `docs/security-review.md`
- **Kind:** security

## What changed

`query`, `explain` and `related` authorise the caller before converting the
request, rather than after. All three resolved their table with the bare
resolver and then ran a conversion that reads the table's shape — so a caller
holding no grant at all could read a table's column count out of a refusal, in
one request.

## Why

Two entries in a row closed with "finding 8 is read, not probed". Probing it
found the finding open on three handlers the fix never touched, and disclosing
more than the channel it did close.

Finding 8 was about `fingerprint::check` running before anything authorised the
caller, letting someone confirm a guessed `(name, type, key)` layout **one
64-bit fingerprint at a time**. The fix reordered the handlers that
fingerprint-check. `query`, `explain` and `related` do not fingerprint-check
first; they convert. And conversion is schema-dependent by design — a
`ColumnRef` is "which input, and which index inside it", and the proto says
plainly that the server resolves it "because only the server knows how wide
each table is".

So, from a role granted nothing whatsoever on `users`:

```
the projection names column 99 of table `users`, which has 4 columns
```

That is not a bit per probe. It is the exact width, stated, in one request.
`related` gave up a different part of the schema by the same route:
`resolve_relation` refuses an unknown foreign key by **listing the ones that
exist**, and it ran before the handler's per-step authorisation.

**Three in a row is the pattern worth recording.** This is the third
consecutive commit where a fix covered one path of several: finding 1's
refusal covered one catalog constructor of two, finding 6's covered one
`Authenticator` of two, and finding 8's covered the write handlers and not the
read ones. Each time the fix was correct for the path the finding was written
about, and each time "the other paths look the same" was a reading rather than
a test. The lesson is not about these three fixes; it is that a finding names
the path where it was *found*, and the fix inherits that scope silently.

## Alternatives rejected

**Make the conversion errors vague instead.** Stop the message naming the
column count. Cheaper, and wrong twice: the count is still recoverable by
whether the request is refused at all, so it would close the message and leave
the oracle; and it would make the error useless to the overwhelmingly common
caller, who has a grant and a bug. The disclosure is the *ordering*, not the
wording.

**Authorise inside `query_from_proto`.** It would cover every caller of the
converter at once, which is the shape that would have prevented all three of
this week's misses. Rejected because the converter has no `SecurityContext` and
threading one in makes a pure function into a policy decision point — and the
action to check differs per handler, which is exactly the hazard
`each_handler_authorizes_the_action_it_performs` was written for.

**For `related`, authorise the step's `rows_from` rather than
`relation.table`.** More precise: `rows_from` is what is actually read, and for
a `PARENTS` step that is the parent while `relation.table` is the child. It
cannot be done — `rows_from` is only known *after* resolving, and resolving is
what discloses. So this asks for `Read` on the child even when the child's rows
are not read. Accepted deliberately: naming a relation is asking what foreign
keys a table has, and a caller with no grant on it has no business being told.
The cost is a caller who can read parents but not children now needs a grant on
the children to traverse to them, which the Python (295) and Go suites say
nobody does.

**Leave `explain` authorising `Read`, like `query`.** Simpler and subtly wrong.
The kernel checks `Explain` first and `Read` second, so a caller holding
neither would be told "read" where the kernel says "explain" — a difference
only in the message, which is why the mutation swapping them **survived** until
the test asserted the action by name.

## Evidence

**The disclosure, before the fix.** Two requests differing only in a projection
ordinal, from `stranger`, a role granted nothing on `users`:

```
assertion failed: a real and an absent column must be indistinguishable
  "access denied: no role grants read on table `users`"
    vs
  "the projection names column 99 of table `users`, which has 4 columns"
```

After: both `PermissionDenied`, same message.

**Four mutations, all caught**, each by the test written for it:

```
ok  query resolves without authorising, as before   -> a_caller_with_no_grant_cannot_probe_a_tables_width
ok  explain resolves without authorising, as before -> explaining_does_not_leak_a_tables_width_either
ok  the relation resolves before anything authorises -> loading_does_not_leak_a_tables_foreign_keys
ok  explain authorises Read instead of Explain       -> explaining_does_not_leak_a_tables_width_either
```

**The fourth survived at first**, which is the result worth keeping. Swapping
`Action::Explain` for `Action::Read` changed nothing any test could see,
because both refuse the same callers — the only difference is which action the
refusal names, and nothing asserted that. The test now does. A mutation that
survives because two behaviours are *nearly* equivalent is the case
`CLAUDE.md`'s "a mutation that causes no failure is a missing test" is hardest
to apply to, and the missing test here was one line.

**Nothing over-refuses**, which was the risk in the `related` change:

- `cargo test -p slate-server -p slate-serverd --no-fail-fast` — green, 24
  suites in slate-server alone.
- `clients/python`, the whole suite against a built `slate-testserver`:
  **295 passed** in 102s. It exercises relations in both directions.
- `clients/go`, `go test ./...` against a built `slate-serverd`: **ok**, 10.4s.
- `cargo clippy -p slate-server -p slate-serverd --all-targets` clean,
  `cargo fmt --all -- --check` clean, `scripts/check.sh` 23/23.

**Every `fingerprint::check` is still guarded.** Fourteen call sites now,
against the four the original fix was written for, and each is immediately
preceded by `authorized_table`. Checked by reading the fourteen, not by a test
— see below.

## What this does not do

**Nothing stops the next handler getting this wrong.** Fourteen
`fingerprint::check` sites and three converters are guarded because somebody
looked; a fifteenth handler calling `self.table(..)` and converting would be as
wrong as these three were, with nothing to say so. A guard — a lint, or a
resolver that cannot be called without a context — is the real fix and is not
here. The `query_from_proto` rejection above says why the obvious version does
not work; it does not say no version does.

**The TypeScript suite, the conformance runner and the e2e were not run
locally.** Python and Go were, and both exercise relations. The other three
need a browser or more setup than the disk had left; CI runs all of them, so a
break surfaces there rather than here. That is a reason to watch CI, not a
check I ran.

**`explain`'s early action is right for today's kernel.** It checks `Explain`
then `Read`; if that order ever changes, this handler names the wrong one again
and only the message differs, which is precisely the thing that survived a
mutation. Nothing ties the two orderings together.

**Table existence is still disclosed, unchanged and deliberate.** An unknown
name answers `NOT_FOUND`, a known one with no grant `PERMISSION_DENIED`. The
reasoning is in `authorized_table` and I did not revisit it.

**I did not audit the remaining `self.table(..)` callers beyond these three.**
There are four in the file; one is inside `authorized_table` itself and three
are these. That is the whole list *today*, from a grep, and the grep is the
evidence.
