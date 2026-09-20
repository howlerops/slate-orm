# A message that says nothing

## What changed

`TableBuilder::build` now refuses a `CHECK` whose message is present and empty,
or present and only whitespace, with a new `SchemaError::EmptyCheckMessage`.
A check with *no* message is unaffected and remains ordinary.

Three tests in `crates/slate-schema/tests/schema.rs`: the empty case, the
whitespace case, and the control.

## Why

`ledger/2026-09-19-a-check-that-names-its-field.md` named this and dismissed
it in the same sentence: *"A schema author can write an empty `message` and get
a status ending in `: `, which is ugly and not wrong enough to refuse a
deployment over."*

That reading was wrong, and what makes it wrong is work done *after* it was
written. When that entry shipped, the message was a fragment of a status string
— so an empty one really was cosmetic. Since then the message is:

- collected into `ErrorInfo` metadata as `message.N`, one per failing check;
- decoded by all three clients into a typed `violations` list;
- generated into three languages by `scripts/codegen.py`;
- and rendered by the demo's adapters beside the field it refers to.

An empty message is now a **blank error message shown to somebody trying to fix
their input**. The form highlights a field and says nothing about why. That is
not ugly, it is the validation surface failing at the one moment it exists for.

`Some("")` and `None` are also not the same thing, and that distinction is what
makes refusing safe: an author who called `with_message` meant to write a
sentence. Refusing the blank costs a schema author one clear error at start-up;
the alternative costs an end user a form they cannot get past.

## Alternatives rejected

**Normalise `Some("")` to `None` at build.** The output would be correct — the
refusal renders as ``check `year_is_positive` `` with no dangling colon — and it
would never fail a deployment. Rejected because it is silent: the author still
believes there is a message, the clients still generate a check with none, and
the form still shows nothing useful. It fixes the punctuation and leaves the
defect. `CLAUDE.md`'s "prefer a hard error to a skip whenever the thing being
skipped is the point" is exactly this case.

**Require every check to have a message.** Much stronger, and a different and
larger decision: most checks in this repository have none, deliberately, because
`year > 0` needs no gloss. Demanding one would be a schema-design opinion rather
than a correctness rule, and would break every existing catalog.

**Refuse only at the point of use**, i.e. when the violation is rendered.
Rejected because it moves a schema mistake into a runtime path — the error
surfaces when a user happens to trip the check, on a request, rather than when
the node starts. Catalog validation exists so the shape of the schema is
somebody's problem before it is an end user's.

**Also check the message does not interpolate row data**, which the original
entry named alongside emptiness. Not attempted: the message is a static string
in the catalog and cannot interpolate anything, so the concern was about an
author *writing* a value into it by hand. No mechanical check distinguishes
"Year must be positive" from "Year 1847 must be positive", and the second is a
judgement call about a schema nobody has written yet.

## Evidence

**Three mutations, each caught by a named test**, restored after each:

| mutation | test that failed |
| --- | --- |
| the check removed entirely | `a_check_may_not_carry_an_empty_message`, `a_check_message_of_only_whitespace_is_empty_too` |
| `m.trim().is_empty()` → `str::is_empty` (untrimmed) | `a_check_message_of_only_whitespace_is_empty_too` |
| `is_some_and` → `is_none_or` (refuses a missing message too) | `a_check_with_no_message_is_fine`, plus two unrelated tests that build a message-less check |

The third mutation is the one the control test exists for. Without
`a_check_with_no_message_is_fine`, that mutation would still have been caught —
by two tests that build a check for other reasons and would have failed
confusingly — and the failure would have named the wrong thing.

**Nothing in the repository trips the new refusal.**
`grep -rn 'message *= *""'` over every `.toml`, `.rs` and `.py` outside `target`
returns nothing, so no fixture, config or demo catalog is affected.

**Suites:** `cargo test -p slate-schema -p slate-kernel -p slate-orm
--no-fail-fast` — 74 test binaries, all ok. `cargo test -p slate-server -p
slate-serverd --no-fail-fast` — 32 binaries, all ok. `sh scripts/check.sh` —
20 of 20.

**A note on the run.** The server crates first failed with
`No space left on device` building `slatedb` — the ENOSPC condition `CLAUDE.md`
warns reads as a compiler error. Reclaimed to 12G free and both crates then
passed. Recorded because it is the second time today the first output of a suite
looked like a code failure and was not.

## What this does not do

- **This is a behaviour change to catalog validation.** A deployment whose
  config sets a check message to `""` will now refuse to start rather than
  serve with a blank message. That is intended and it is the cost: nothing in
  this repository does it, and a deployment elsewhere that does would see a
  clear error naming the table and the check.
- **"Is a sentence" is still unchecked**, and I do not think it is checkable.
  A message of `"x"` passes.
- **"Free of the row's data" is still unchecked**, for the reason above.
- **Nothing checks the message at the *wire* boundary.** If a catalog were
  constructed by some path that bypasses `TableBuilder::build`, an empty message
  could still reach a client. I did not look for such a path; the builder is the
  only constructor `TableDef` exposes, which is a strong hint there is none, and
  a hint is not a check.
