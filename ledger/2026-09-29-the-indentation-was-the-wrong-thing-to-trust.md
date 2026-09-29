# The column-zero test failed open, and it was written down as a deliberate choice

- **Date:** 2026-09-29
- **Author:** an agent working through the record-layer task list
- **Touches:** `scripts/check_handlers.py`, `scripts/test_check_handlers.py`
- **Kind:** security

## What changed

Rule 10's `IMPL_CATALOG` matches `impl Catalog` at any indentation and
captures it, and the block now ends at the `}` sitting at that same
indentation rather than at the first `}` in column zero. Two test cases: an
accessor inside `mod extra { impl Catalog { … } }` is reported, and a
function *after* that block closes is not read as one of its methods.

## Why

The entry that widened rule 10 to the crate recorded this as a limit and gave
a reason:

> **Column zero is still the test for an `impl` block.** An `impl Catalog`
> indented inside a `mod` — legal, and how a `#[cfg(test)]` helper would be
> written — is skipped. That is deliberate for tests and wrong for a real
> submodule that indents its contents, which no file in this crate does and
> nothing checks.

The second half of that sentence is the finding. A guard's default has to fail
*closed*: the column-zero test skips an indented block whether it is a test
helper or a real submodule, and it cannot tell them apart, so the case it was
chosen for and the case it was wrong about are the same match. Matching any
indentation inverts that. A `#[cfg(test)]` module that really does add a
public accessor to `Catalog` now needs a `HANDS_OUT_A_TABLE` entry saying it
is a test helper — which is the roster idiom the other nine rules are built
on, and better than a silence.

That it was written down as "deliberate for tests" is the part worth keeping.
It reads as a decision somebody weighed. It was a default nobody had turned
over, wearing a decision's clothes, and the thing that exposed it was writing
the limit out in full: the sentence had to end with "and nothing checks",
which is not how a deliberate choice ends.

## Alternatives rejected

**Match any indentation but skip a block under `#[cfg(test)]`.** Keeps the
original intent and needs the guard to track attributes and module nesting to
know whether the `impl` it just matched is inside a test module. That is a
Rust parser's job, and a half-built one gets the answer wrong in the
direction that stays quiet. A roster entry costs one line and is auditable.

**Brace counting instead of matching indentation.** Correct in general and
wrong here: string literals, char literals and comments containing braces all
have to be handled or the count drifts, and a drifted count either ends the
block early — silently dropping accessors — or never ends it, sweeping the
rest of the file in. Indentation is a weaker rule that fails in a way a
`cargo fmt`ed tree cannot reach. The residual is below.

**Leave it and keep the caveat.** The same call as last time and the same
answer: three lines against an open row that reads as a hazard somebody
accepted. It is also a *security* guard's hole, which is a different weight
from a measurement's.

## Evidence

`python3 scripts/check_handlers.py` on the real tree: unchanged, 4 public
`Catalog` methods handing out a table, all rostered — `catalog.rs` has one
`impl Catalog` at column zero, so the widening changes nothing about today's
answer and everything about tomorrow's.

`python3 scripts/test_check_handlers.py`: **58 passed, 0 failed**, up from 56.

Three mutations, record
`ledger/mutations/20260929T224957-scripts-check-handlers-py.json`:

| mutation | caught by |
| --- | --- |
| the `impl` must start at column zero again | `a lookup in an indented impl Catalog is reported` |
| the block ends at the first line ending in `}`, any depth | most of the file's cases |
| the block ends only at column zero, whatever its own indent | `a method after an indented block closes is not inside it` |

The first and third are one case each, and they are the two halves of the
change: matching the opening brace and matching the right closing one. The
middle one over-catches because ending every block at the first `}` breaks
the top-level scan for every fixture; it is reported here rather than
presented as three clean single-case catches.

## What this does not do

**Indentation is not nesting.** A block whose closing brace is not at the
`impl`'s own indentation — hand-written, or a macro expansion — runs to the
end of the file, and every `pub fn` after it is read as a `Catalog` method.
That fails toward reporting, so it costs a spurious roster demand rather than
a miss, and `cargo fmt` does not produce it.

**The `#[cfg(test)]` case is now a roster entry, not an exclusion.** If a test
module in `slate-schema` ever adds a public accessor, this fails and a person
adds a line saying it is a test helper. That is the intended behaviour and it
is also a small tax on a refactor that moves tests around; nobody has paid it
yet, so nothing says how it feels.

**Nothing in this change touches the `tables()` hole**, which remains the way
a name becomes a table without rule 10 seeing it, in any crate outside the
two rule 8 reads.
