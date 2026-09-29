# The proto guard's roster named one copy of three, and the other two had been there all along

- **Date:** 2026-09-29
- **Author:** an agent session
- **Touches:** `scripts/check_proto_copies.py`,
  `scripts/test_check_proto_copies.py`, `scripts/mutations.json`
- **Kind:** guard

## What changed

`check_proto_copies.py` held a hand-written `COPIES` dict naming one pair,
`slate/v1/records.proto`. It now walks `crates/slate-server/proto/` and
`clients/typescript/proto/` and compares every `.proto` beneath them, in both
directions: a canonical file with no copy and a copy with no canonical file are
both findings, because with the pairs derived the *set* of files is a claim
too. Three pairs are compared where one was. The fixture suite went from 5
cases to 8.

`scripts/mutations.json`'s real-tree mutation for this guard moved from
`records.proto` to `clients/typescript/proto/google/rpc/status.proto` — one of
the two files the roster never named — so the mutation that proves the guard
is pointed at something lands where the list version would have missed.

## Why

The entry that added the guard left this open, and it was right to:

> `COPIES` is still one pair and still has no completeness check.
> — `ledger/2026-09-21-a-window-crosses-the-wire-in-its-own-list.md`

It is the second time that file has been wrong about the same thing in the same
direction. Its first version claimed the schema had two derivatives; CI found
four the same afternoon, and the entry carries the withdrawal:

> I looked for others and found none, which is not the same as there being
> none.

That sentence is the whole finding here, one layer down. There were two more
literal copies — `google/rpc/status.proto` and `google/rpc/error_details.proto`,
vendored into `clients/typescript/proto/` so `src/details.ts` can decode the
`ErrorInfo` the head node packs into `grpc-status-details-bin` — sitting in the
same directory as the one pair the roster named, duplicated from the same
source tree, and unguarded. **The guard covered one copy of three and reported
a pass.**

They were found sideways rather than by looking again. Widening
`check_cited_files.py` to treat an ambiguous path suffix as a finding
(`ledger/2026-09-29-the-path-in-a-command-was-the-one-nobody-checked.md`)
printed the two `google/rpc/*.proto` names as matching two tracked files each,
which is what a duplicated file looks like from the outside. A guard about
citations found a gap in a guard about copies because both are the same
question — *is this second statement of a fact still the same fact?*

Deriving rather than extending the list is the point. Adding two lines to
`COPIES` would have fixed today and left the next copy exactly as unguarded as
these two were, which is the standing finding this repository keeps meeting:
`ledger/2026-09-21-two-of-three-lists-were-already-guarded.md` and
`ledger/2026-09-29-the-fifth-table-list-is-derived-now.md` are two instalments
of it.

## Alternatives rejected

**Add the two pairs to `COPIES` and keep the dict.** One line each, no new
failure modes, and it leaves the caveat exactly where it is: "still no
completeness check". A roster that has been wrong twice about its own
completeness is a roster, not an oversight — the second miss is the evidence
that the first was structural. The walk costs one `rglob` and cannot fall
behind.

**Add a completeness check *over* the dict** — every `.proto` in either tree
must appear in `COPIES`. This was the shape the caveat itself proposed, and it
is strictly worse: it keeps the list and adds a second thing to maintain, so
the list exists only to be checked against the trees it was derived from. If a
check can compute the roster it is checking, the roster is the bug.

**Generate the copies instead of comparing them** — a script that copies the
canonical tree into `clients/typescript/proto/` and a test that it is a no-op.
That is what the Python and Go clients do with their generated stubs, and it
would be more robust. Rejected as out of scope for closing this caveat, and
because the copies are *literal* and a copy step is one more thing that can be
skipped: the comparison fails whether or not anybody remembered to run
anything, which is the property that matters for a file nothing builds.

**Compare semantically, parsing the protos.** Rejected here for the reason the
guard already gave for the one pair it had: a comment drift is a real failure,
because the comments in `records.proto` carry most of the reasoning about what
each field means, and a client author reading a stale one is what this is
about.

## Evidence

- `python3 scripts/check_proto_copies.py`: **3 pairs**, up from 1. The two that
  were unguarded were byte-identical when this was written — measured, and a
  **null result**: nothing was broken, and nothing had been checking.
- `python3 scripts/test_check_proto_copies.py`: **8 passed, 0 failed**, from 5.
  Three of the new cases exist only because the pairs are derived — a copy with
  no canonical file, a second pair with one of them drifted, and both trees
  empty (the never-fires case, which a list of pairs could not have had).
- **Five mutations against the fixture suite, all caught**, recorded in
  `ledger/mutations/20260929T163225-scripts-check-proto-copies-py.json`: each
  direction of the set comparison dropped separately, the byte comparison
  disabled, the never-fires branch disabled, and `rglob` narrowed to `glob` so
  only a top-level `.proto` is seen.
- **One mutation against the real tree**, in
  `ledger/mutations/20260929T163239-clients-typescript-proto-google-rpc-status-proto.json`:
  the vendored `google/rpc/status.proto` reuses a field number. Caught. That
  file is the one the old roster did not name, so this is the measurement that
  says the widening reaches and not just that it runs.
- `python3 scripts/run_mutations.py`: **27 suites clean, 0 with findings**,
  after repointing this guard's suite.
- `sh scripts/check.sh`: **85 passed, all of them**, exit 0.

## What this does not do

**It compares two trees, and there could be a third.** The walk is over
`crates/slate-server/proto/` and `clients/typescript/proto/` by name. A copy
vendored into a fourth place — another client, an example, a fixture — is as
unguarded as these two were, and this entry is the second in a row to observe
that looking and finding none is not the same as none. What would settle it is
a rule over *every* `.proto` in the repository, grouped by path within its
tree; that is a different guard and is not written.

**The generated stubs are still checked by their own tests, not by this.**
`clients/python` and `clients/go` regenerate and compare bytes, which is
stronger than what happens here. Nothing joins the two: a change to
`records.proto` that somebody propagated to the copy and not to the generators
fails in the client jobs and not in this guard, which is fine and worth knowing
when reading a red build.

**Nothing checks that a vendored `google/rpc` file still matches upstream.**
Both copies are compared to `crates/slate-server/proto/`, and that tree's own
fidelity to googleapis is unchecked and always was. The repository's copy of
`error_details.proto` deliberately declares three messages where upstream
declares ten — written up in `docs/orm-comparison.md` — so a literal upstream
comparison would be wrong, and what a correct check would compare is not
obvious.

**The mutation moved rather than multiplied.** `scripts/mutations.json` allows
one suite per guard, so pointing the real-tree mutation at `status.proto` means
`records.proto` no longer has one there. It is covered by the fixture suite and
by the byte comparison it shares with the other two, so nothing is untested —
but the strongest single check now lands on a vendored file rather than on the
schema itself, which is a trade made for reach and is worth reversing if the
roster ever takes two suites for one guard.
