# The exit code a deployment now uses

## What changed

`examples/deployed/run.sh` runs `slate-serverd --plan` against the bucket it
has just written to, after the node is killed and its lease has expired and
before the replacement starts. A non-zero exit fails the harness; so does any
plan that is not empty.

Two claims in `ledger/2026-09-19-look-before-the-restart.md` updated: one
closed, one annotated with what checking it actually found.

## Why

`--plan` was built with the sentence *"Non-zero so a deployment can gate on
it"* in a comment beside `std::process::exit(1)`. No deployment gated on it.
That is this repository's most familiar defect — the workflow that never fired,
the decoders that compiled and never ran, the counter nobody scraped — and the
entry that shipped the flag named it: *"Nothing here proves the exit code is
usable as a gate beyond the test that asserts it."*

There is a second thing the harness buys that a test cannot. `plan.rs` covers
every shape of plan — first deploy, nothing to do, an index build, a blocked
layout change, the memory backend, and not fencing a live leaseholder — and
covers **all of them over `backend = "local"`**, for the good reason its own
header gives. The deployed harness is the only place in the repository where
`--plan` meets S3.

The position in the script matters and is the position an operator would use:
after the old node is gone and its writer lease has timed out, before the new
one starts. That is the moment a deploy would ask "what is about to happen to
this bucket".

## Alternatives rejected

**Assert only the exit code, not the output.** One line shorter, and it would
pass on a plan proposing an index build — which at that point in the harness
would mean the *first* node had not finished migrating what it claimed to
before acknowledging writes. That is a defect worth failing on, and an
exit-code-only check would let it through silently.

**Preview before the kill instead.** Simpler to write, and it tests the wrong
thing twice: `previewing_does_not_fence_the_node_that_holds_the_lease` already
covers `--plan` against a live leaseholder, and an operator previewing a restart
does it when the old process is down.

**Change the configuration between the two nodes so the plan has real steps.**
Much more convincing as a demonstration — the harness would preview an actual
index build over S3. Rejected because the restart phase's whole claim is that
*the bucket is the system of record and the process is not*, which requires the
second node to be the same node. Introducing a schema change would make the
restart prove something else, and the checks after it would be comparing
against a moved target. A preview that correctly says "nothing to do" still
exercises reading stored state out of S3, which is the untested part.

**Add a `--plan` step to CI as its own job instead.** It would need its own
MinIO and its own populated bucket — that is the deployed harness, rebuilt.

## Evidence

**The gate works in both directions, verified against real stored state** — a
`local` backend here rather than S3, because MinIO is not available in this
container:

- Deployed a node against an empty store, killed it, ran the harness's exact
  `if ! plan=$(...)` / `case` block: `GATE PASSES: Up to date: the stored schema
  already matches this configuration.`
- Re-ran the same block against a config that adds `by_kind` to the same store:
  `GATE CORRECTLY REJECTS`, printing the one-step plan. Without the output
  check, this case exits zero and passes.

**The string the `case` matches is the string the binary prints**, and is
already pinned: `plan.rs::a_keyspace_already_carrying_this_schema_has_nothing_
to_do` asserts `output.contains("Up to date")`, so the harness and the test
agree about the text by construction rather than by my having read it once.

**`sh -n examples/deployed/run.sh`** — syntax clean.

**The `DropIndex` naming item was checked rather than restated.**
`TableState::built` is a `Vec<IndexId>` encoded as four bytes per id, so the
stored side has never held an index's name. The original comment said the name
was unavailable "by definition" from the *current* catalog; the stronger and
more useful fact is that it is unavailable from the stored one too, and printing
it would mean migrating the migration state. Written into the older entry rather
than left as a to-do that looks cheap and is not.

## What this does not do

- **I did not run `examples/deployed` itself.** It needs MinIO, which this
  container does not have; CI's `deployed` job runs it. What I verified locally
  is the gate's logic and its two outcomes against a real populated store over
  the `local` backend. If the S3 path behaves differently, CI will say so, and
  that would itself be the finding — which is the argument for putting it there.
- **The backfill estimate is still missing.** The same entry's first item —
  *"It does not show how long a backfill would take... A table's statistics
  could answer it and this does not ask them"* — is untouched. It is the item
  with the most value left in it and it is a real change to the planner's
  output, not a harness change.
- **`backend = "memory"` still prints no plan**, which I looked at and left: it
  has no stored state a separate invocation can read, the flag says exactly that
  in a paragraph, and `a_memory_backend_says_it_has_no_stored_state_rather_than_
  inventing_one` pins it. Correct by design rather than a gap.
- **No measurement.** The preview adds one process start to a harness that
  already starts several; I did not time it.
