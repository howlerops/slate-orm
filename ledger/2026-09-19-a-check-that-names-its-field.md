# A check that names its field

## What changed

A `CHECK` can now say which column it is about and what sentence to show:

```toml
[[tables.checks]]
name = "title_length"
predicate = "title ~ '^.{1,80}$'"
column = "title"
message = "Title must be 1 to 80 characters."
```

Both are optional. `CheckDef` gained `with_column` and `with_message`,
`SchemaError::CheckViolation` gained `column` and `message`, and the daemon
puts `column` in `ErrorInfo.metadata` beside a new reason token,
`CHECK_VIOLATION`.

## Why

The first of the three recommendations the validation note ends on. The
refusal was `row violates check `title_length` on table `docs``: it names the
rule that fired and not the field to put it beside, so a form either shows a
generic error or parses a name the schema author chose — a contract that works
until somebody renames a check.

`column` is optional because `discount <= price` is about two columns and
naming one would be a lie a form would render next to the wrong field. That is
also why a `column` naming nothing is refused **at startup**: the whole point
is that a form can trust it, and one pointing at a column the table does not
have fails silently, at the first refused write, in production.

## Alternatives rejected

**Infer the column from the predicate.** `size >= 0` mentions exactly one
column, so a parser could fill this in. Rejected: it is right until the
predicate mentions two, and then it is either wrong or absent, and the author
cannot tell which without reading the inference rule. A field that is sometimes
inferred and sometimes declared is worse than one always declared. The
predicate is already parsed at startup, so this would have been cheap — it is
not rejected for cost.

**A translation key instead of a string.** The note flagged this as unsettled
and it is settled here, for this: a string. The catalog is the only place the
rule exists, and a key would put the rule in the catalog and its meaning
somewhere this system cannot see or validate. A deployment that needs two
languages can put a key *in* the string; nothing stops it, and the default case
stays readable. This is a reversible decision and the note now says which way
it went rather than leaving it open.

**Leave the reason token as `SCHEMA`.** It is a wire contract, and the file
says renaming one breaks a client as surely as changing a code would. Split
anyway, because a check violation is the caller's *data* being wrong — a form
retries it after editing a field — and every other schema error is the caller's
*schema* being wrong, which retrying cannot fix. Collapsing those is exactly
the loss the token system exists to undo. Checked rather than assumed safe:
`grep '"SCHEMA"'` across every client finds only the line that produces it.

**Put `column` in the message text.** Then every client parses prose. The
unique-violation case three arms up in the same function already made this
decision the other way, for the same reason, and copying it keeps one idiom.

## Evidence

Seven mutations across four layers, all caught:

| mutation | caught by |
| --- | --- |
| the error never carries the column | `a_check_violation_carries_the_column_and_message_it_was_given` |
| the error never carries the message | the same |
| `with_column` silently drops the value | `a_check_carries_its_column_and_message` |
| `with_message` silently drops the value | the same |
| a `column` naming nothing is accepted | `a_check_naming_a_column_that_does_not_exist_is_refused` |
| the wire sends `column: ""` instead of omitting it | `a_check_violation_with_no_column_omits_the_key` |
| a check violation reports the generic `SCHEMA` token | `a_check_violation_names_its_column_in_the_details` |

The empty-string mutation is the one worth keeping. `put("column",
column.unwrap_or_default())` compiles, passes any test that only checks the
happy path, and tells a form to render the error beside a field named `""` —
which is worse than telling it nothing, because nothing has a sensible
fallback.

`cargo test -p slate-schema -p slate-kernel -p slate-serverd -p slate-server`:
45 suites, no failures. `cargo clippy --workspace --all-targets`: clean.

A full `cargo test --workspace` was attempted twice and died at the linker both
times with the container out of disk — the `ENOSPC` signature `CLAUDE.md`
describes, not a test failure. Reclaiming and narrowing to the four changed
crates is what produced the run above; the whole workspace is CI's.

## What this does not do

The write path still stops at the first failing check, so a form with four bad
fields still needs four round trips. That is recommendation (2) and the next
task; this change deliberately did not restructure the error into a set,
because the two decisions are separable and doing both at once would have put
one ledger entry's worth of reasoning behind two contracts.

No client reads the new metadata. Python, Go and TypeScript all surface the
status message, so the `message` already reaches a caller; the `column` is on
the wire and nothing picks it up yet. A client-side `error.column` is a small
addition to three SDKs and is not here.

~~Nothing validates that `message` is a sentence, non-empty, or free of the
row's data. A schema author can write an empty `message` and get a status
ending in `: `, which is ugly and not wrong enough to refuse a deployment
over.~~ **Partly closed** — an empty or whitespace-only message is now refused
at catalog build; see `2026-09-20-a-message-that-says-nothing.md`, which also
explains why "ugly, not wrong enough" was the wrong reading. "Is a sentence"
and "free of the row's data" remain unchecked and are not checkable.

`#[derive(Record)]` cannot declare either field. A check written in Rust goes
through `CheckDef` directly and can call the builders; one written through the
derive macro's attribute cannot yet say `column = "..."`.
