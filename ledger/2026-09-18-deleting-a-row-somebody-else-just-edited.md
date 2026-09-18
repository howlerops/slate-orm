# Deleting a row somebody else just edited, refused rather than done

- **Date:** 2026-09-18
- **Author:** Claude (Opus 5), working `docs/orm-comparison.md`'s W list
- **Touches:** `crates/slate-kernel` (`record.rs`), `crates/slate-orm`
  (`ext.rs`), `crates/slate-server` (proto, `service.rs`, `session.rs`), all
  three clients, `examples/explorer/`, `README.md`,
  `docs/orm-comparison.md`, `site/docs/clients.html`
- **Kind:** feature

## What changed

`RecordTransaction::delete_if_unchanged` in the kernel, `Records::remove_record`
over it, `DeleteRequest.expected` on the wire dispatching on all three write
paths, and `delete(..., expected=...)` / `DeleteIfUnchanged` /
`deleteIfUnchanged` in the three clients. Three conformance cases and a
`/api/conditional-delete` endpoint in all three adapters.

## Why

The README carried this as a one-sentence open item: "Deleting a row somebody
else just edited is the same class of mistake as overwriting it, and the same
argument applies." It is right, and the case is concrete. A moderator reads a
post, sees no views and no replies, decides it is spam and deletes it. Between
the read and the delete it acquires five thousand views. The delete lands, the
count comes back `1`, and nothing anywhere says the decision was made about a
row that no longer existed.

## Alternatives rejected

**Returning `bool` like `delete_record` does.** The obvious signature, and it
makes the conditional delete a drop-in. Rejected because the two answer
different questions. `delete_record` answers "make sure this is gone", which is
idempotent and where `false` correctly means "it already was". A caller that
*says what it expects to find* is asking something else, and `false` there
means somebody got there first — which is exactly the race this feature exists
to report, handed back as a value a caller will read as success. `Result<()>`
and `RowNotFound` make it impossible to miss.

**`Aborted` for the absent row**, matching the stale one. Both are "somebody
raced you", so one code is tempting. They differ in what a caller should do:
`Aborted` means re-read and decide again, and that is sound for a row that
moved; a row that is gone will not come back, so a client with a retry loop on
`Aborted` would spin. `NOT_FOUND` says the loop is pointless.
`a_row_already_gone_is_refused_rather_than_counted_as_absent` asserts the code
and the stable reason token rather than just "an error".

**Guarding the cascade.** `expected` covers the named row only, and a cascade
may still remove children the caller never saw. Making the cascade conditional
was considered and is not possible in any honest form: the caller does not know
what the deletion closure contains, and the closure is computed *without* the
row policy on purpose — so asking the caller to name it would mean disclosing
rows their policy hides. Documented in the proto, the kernel and the clients
rather than left for somebody to discover.

**Calling `delete` from `delete_if_unchanged`** after the comparison, instead
of repeating its three-line body. It would read a second time. Nothing can
change between two reads inside one transaction, so the second read is pure
cost, and the duplication is three lines with a comment saying why.

**A `RowDelete` pair in Go and TypeScript, matching `RowUpdate`.** Not really
an alternative — it is the same forced choice W1 recorded: both clients'
`delete` is variadic over its keys, so an optional argument is unavailable and
a method of its own is what is left. Pairing key with row keeps the wire's two
parallel repeated fields unrepresentable in the wrong length.

## Evidence

Kernel and ORM: five tests in `crates/slate-orm/tests/concurrency.rs`, each
showing both halves — the plain delete removing a row the caller never saw, and
`remove_record` refusing. Wire: twelve in
`crates/slate-server/tests/conditional_delete.rs`. Clients: 8 Python, 6 Go, 6
TypeScript. Corpus: 89 cases, three SDKs agreeing on all of them.

**Fifteen mutations. Thirteen killed outright; two survived and were then
killed.**

| mutation | outcome |
| --- | --- |
| the kernel skips the row comparison | KILLED `remove_record_refuses_rather_than_deleting_it` |
| the kernel returns `Ok` for an absent row | KILLED `a_row_already_gone_is_refused_rather_than_reported_as_removed` |
| `apply` ignores `expected` on a delete | KILLED (4 tests) |
| the session actor ignores `expected` | KILLED (2 tests) |
| the delete arity check removed | KILLED `expected_must_name_one_row_per_key` |
| Python: `expected` built and never sent | KILLED (3 tests) |
| Python: the arity check removed | KILLED |
| Go / TS: the session sends no expected rows | KILLED (3 and 3) |
| Go / TS: the transaction sends no expected rows | KILLED |
| Go / TS: each key guards itself | KILLED |
| the Go adapter sends a plain delete | KILLED (2 corpus cases) |
| the Python adapter loses the not-found distinction | KILLED (2 corpus cases) |
| **the kernel's key-mismatch check removed** | **SURVIVED** → killed |
| **the session actor's loop does not stop at a refusal** | **SURVIVED** → killed |

The first survivor is the more interesting. Removing the kernel's
`expected.primary_key_values(table) != primary_key` check left every existing
case answering `ROW_CHANGED` anyway — the read finds the row at the key, the
contents differ, and the outcome is identical. There is exactly one input where
it is not: a key that is *absent*. Without the comparison the caller is told
"no such row" and goes looking for a row that was never the problem, when their
actual bug is a mismatched pair.
`a_mismatched_pair_is_reported_as_such_and_not_as_a_missing_row` is that input,
and it is the same argument the update's twin of this check already carried in
prose.

The second is W1's finding a third time, and it is becoming a pattern worth
naming: **the transaction path's loop can only be observed with more than one
row.** The transaction test deleted one key, and with one key a loop that never
breaks is indistinguishable from one that does.
`a_stale_first_key_refuses_the_rest_inside_a_transaction` puts the stale key
first.

**An id collision, found on a full run.** `test_conditional_delete.py` took
`FIRST = 500_000`, which is `test_paging`'s range; its cleanup filters on its
own `kind`, so it could not remove paging's rows and the insert collided. It
passed alone and failed together — which is what a "flake" usually is, and the
second time this session has hit it. Moved to 900_000 with the reasoning in the
constant's comment.

Green: `cargo test --workspace` 1439 tests, `cargo fmt --all --check`,
`RUSTFLAGS=-Dwarnings cargo clippy --workspace --all-targets`, Python 280 with
`ruff` and `ty` clean, Go all, TypeScript 152, the docs check, the workspace
guard, the pre-commit hook's own 14.

Disk ran out completely during this — `cargo test --workspace` would not fit
beside 24 GB of accumulated `target/` variants, and the ledger's dedup snippet
was no longer enough. A full `cargo clean` freed 23.9 GB and the suite then ran
from scratch. Worth recording because the snippet is the documented remedy and
this is the first time it was insufficient.

## What this does not do

- **A conditional delete is one round trip per key**, like the plain one. The
  plain delete's comment already says why there is no batched form — two keys
  in one batch can reach the same doomed row by different foreign-key paths, so
  a batch would have to union the closures before writing anything. That
  argument is unchanged and this adds nothing to it.
- **It does not guard a cascade.** Above, and in three doc comments.
- **No SQL surface.** The front end has no `DELETE … WHERE … AS OF`, and
  nothing here proposes one.
- **`affected` is always the number of keys sent**, so it carries no
  information a caller did not have. That is a consequence of refusing the
  absent row rather than counting it, and it is stated where somebody would
  otherwise read the count as meaning something.
- **The demo's UI does not show it.** The endpoint exists and the corpus drives
  it; no panel does.
