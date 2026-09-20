# The restore helper the last entry reverted, generated and actually called

- **Date:** 2026-09-20
- **Author:** Claude, closing the "what this does not do" of the entry beside this one
- **Touches:** `scripts/codegen.py`, the four generated declarations, `examples/retention/*`, the three row-test suites, `.github/workflows/ci.yml`, `scripts/test_check_sh.py`
- **Kind:** feature

## What changed

`restored()` / `Restored()` / `restoredX()` are generated for every
soft-deleting table, in Python, Go and TypeScript: the row with its stamp
cleared, ready to send back through `update`. Each is tested in its own
language, through the encoder rather than on the struct alone.

The retention example grows an **undo window**: a third role, a restore before
the sweep, and a row the sweep then has to leave alone. `examples/retention/
schema.py` is now regenerated and diffed in CI, which it was not.

## Why

`ledger/2026-09-20-the-write-that-names-a-key.md` made restoring possible and
closed with "**No client helper.** … regenerating it across Python, Go and
TypeScript is its own change." This is that change. The helper existed once and
was reverted, by
`ledger/2026-09-20-the-row-that-is-both-there-and-not.md`, on the grounds that
"a helper for an operation the database cannot perform is worse than no helper".
That is no longer true, so it is back.

Generating it rather than letting callers clear the column is the same argument
as `retired`: **the catalog knows which column carries the stamp and the client
should not have to.** A caller who writes `row.deleted_at = None` by hand has
hard-coded a column name the schema is free to change, and will find out by
writing to the wrong one.

**The retention harness is where it is called, and that placement is the point.**
`ledger/2026-09-20-...-the-accessor-three-adapters-now-call.md` recorded
`retired` as generated, compiled and never called; a unit test fixes that
narrowly, and a real server exercising it through CI fixes it properly. It also
happens to be the honest home: a retention window *is* an undo window, and until
this week the only thing that could happen to a retired row was being erased.

Two things fell out of putting it there, and both are worth more than the helper.

**Neither existing role can restore.** The application holds `update` and not
`read_deleted`; the retention job holds `read_deleted` and not `update`. Restore
needs both, so undoing a delete is a *third* party — which is the right answer
and was not an intended one. `head.toml` now says so in the place someone
copying it will read: an application that can un-delete anything it deleted has
no retention policy, only a convention.

**`examples/retention/schema.py` is generated and nothing regenerated it.** It
was written by `codegen.py` months ago, sits in the tree, and was outside the
`--check` step that diffs the explorer's three — so it silently did not have
`restored()` and the harness failed on `AttributeError` the first time it ran.
A generated file nobody regenerates is a hand-written file with a misleading
header. It is in CI now.

## Alternatives rejected

**A `restore()` on the client that reads, clears and writes in one call.**
Rejected: it hides a read the caller has to be authorised for and a write that
can fail on a unique slot, behind a name that sounds total. The generated helper
is a pure function on a row — it cannot fail, cannot do IO, and cannot surprise
anyone about what it sent.

**Return the row already encoded, so the call is `session.update(T, [row.restore_row()])`.**
Rejected for symmetry: `retired` is a property on the decoded row and
`to_row()` is the encoder; a helper that skipped a step would be the only one
in the file that did.

**Mutate in place in Go (a pointer receiver).** Rejected, and the test asserts
against it: a caller holding a row and asking for a restored one should still
have theirs. The value receiver makes the copy free to write.

**Demonstrate it in the explorer's three adapters instead.** That would have
meant a restore endpoint in Go, TypeScript and Python plus conformance cases
— a lot of surface for one helper, and the demo's `shipments` has no retention
story to hang it on. The retention example already stands up a node, already
has the grants split three ways, and already runs in CI.

**Leave `examples/retention/schema.py` out of the `--check`.** It is one file
and it had already drifted once this session. The cost is a second `mktemp`
in one CI step.

## Evidence

**The harness, run**:

```
head node on 127.0.0.1:35121
seeded: 1 and 3 retired, 2 live
undone: 3 is live again
refused: the application cannot undo 1 without read_deleted
notes: erased 1 row(s) retired before 1789922139
verified: 1 is gone, 2 and 3 are untouched
```

**And the harness can fail**, which is the part this repository has been caught
on before — the same file once *skipped* its whole check and reported success.
Two negatives, each run:

- Remove the undo step: `after the sweep the table holds [2], expected [2, 3] —
  … or 3 was never really restored and the sweep took it`. The restored row is
  load-bearing: a restore that silently did nothing leaves row 3 retired, the
  sweep erases it, and `--verify` fails.
- Replace `row.restored()` with `row` — the row written back exactly as
  `include_deleted` handed it over, which is the mistake a human makes:
  `InvalidRequest: column `deleted_at` … is written by `delete`, not by a
  caller; send null to restore the row`.

**Four mutations of the generator, four named failures**, each regenerated and
run in its own language:

| mutation | test that failed |
| --- | --- |
| Python `restored()` returns the column unchanged | `test_restored_clears_the_stamp_and_leaves_everything_else` |
| Go `Restored()` does not clear the field | `TestRestoredClearsTheStampAndLeavesEverythingElse` |
| TypeScript `restoredX` spreads without clearing | `restoredShipments clears the stamp and leaves everything else` |
| the emitter fires for every table, not only soft-deleting ones | `test_only_a_soft_deleting_table_gets_the_property` **and** `…_gets_restored` |

Each language's test asserts **through the encoder** — `to_row()`, `Row()`,
`encodeShipments` — rather than on the row object. Clearing a field in Python
buys nothing if the encoder then sends the old value, and that is a real shape:
the encoders were added in this repository precisely because a row type that
type-checks is not a row type that sends the right bytes.

**Suites**: `scripts/check.sh` 20/20, `scripts/test_codegen.py`,
`adapter/test_rows.py` 28, `go test ./schema/`, `npm test` 27,
`examples/retention/run.sh`, and `codegen.py --check` clean on all four
generated declarations.

## What this does not do

**Go and TypeScript never restore against a real server.** Their helpers are
unit-tested and their encoders are asserted, but the only end-to-end restore is
the Python one in the retention harness. The three-SDK conformance runner has no
case for it, so "all three agree about restoring" is untested and I am not
claiming it — what is tested is that all three produce a row with a null stamp.

**The demo does not show it.** `shipments` soft-deletes and the explorer's
`/api/typed` reports `retired`, but there is no restore in the UI and no
adapter endpoint for one. Adding it is three adapters and a conformance case.

**There is still no way to ask whether a row is restorable.** A retired row
whose unique slot was taken while it was gone cannot come back, and nothing
says so until the `update` is refused. The kernel test
`restoring_a_row_whose_unique_slot_was_reused_is_refused` pins the behaviour;
no API exposes it in advance, and the harness's single note has no unique index
to collide on, so that path is covered in Rust and nowhere else.

**The undo role is an example, not a recommendation with evidence behind it.**
`read`, `update`, `read_deleted` is the minimum that works — established by the
harness using exactly those and no more — but whether an undo window should be
a separate role, a time-bounded grant, or an operator action with an audit trail
is a question this repository has not looked at and the comment does not pretend
to answer.
