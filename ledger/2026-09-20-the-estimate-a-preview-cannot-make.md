# The estimate a preview cannot make

## What changed

Nothing in `--plan`. The claim that it *should* estimate a backfill is
withdrawn, with the reason.

What did change is the other end: when a start builds more than one index,
`slate-serverd` now prints the entries written **per index** as well as the
total, and `crates/slate-serverd/tests/migrations.rs` gained a case pinning it
over two indexes whose counts differ.

## Why

`ledger/2026-09-19-look-before-the-restart.md` left this: *"It does not show how
long a backfill would take, only that one is coming: the plan holds no row
count and the step does not estimate. **A table's statistics could answer it**
and this does not ask them."*

The bolded half is wrong, and I found that out by trying to do it:

- `RecordStore::analyze` is a **scan**. It walks the table incrementing
  `row_count += 1` per row, sampling columns and building histograms as it
  goes.
- Nothing persists the result. `slate-serverd` runs it at startup into an
  in-memory `Statistics` and that is where it stays; the keyspace holds a
  `TableState` per table with a schema version, a fingerprint and a list of
  built index ids, and no counts.
- `--plan` is a *separate process* that opens the keyspace read-only and reads
  those few state keys.

So the row count is not somewhere `--plan` declines to look. It is somewhere no
second process can look, and producing it costs a full pass over the table —
which is what `BuildIndex` itself costs. **A preview that pays the price of the
thing it is previewing is not a preview.** That is not a limitation to work
around; it is the answer.

What *is* available is the number after the fact. The node already printed
`built N index entries in T`, summed. Over two indexes that answers "was there
a backfill, and how long" and not "which of them was slow" — which is the
question an operator has once the answer to the first is "yes, four minutes".
Since the run that pays the scan is the only place the number can come from,
it should give up all of it.

## Alternatives rejected

**Scan in `--plan` anyway, behind a flag.** `--plan --count` is honest about
its cost and a deploy gate could skip it. Rejected because the flag's whole
claim, in its own doc comment, is that it is the cheap safe thing you run
before a deploy; a mode that quietly reads every row on object storage is a
different tool with the same name, and the first person to put it in a
pre-deploy script would find out during an incident.

**Estimate from the keyspace size.** `open_for_plan` already lists the object
store, so bytes-under-prefix is nearly free. Rejected as not per-table: SSTs
span tables, and `BuildIndex` is per-table. A total that cannot be attributed
would answer "this database is large", which the operator knows.

**Persist the row count in `TableState`.** Then a later `--plan` could read the
last-known size for nothing. This is the real answer if somebody wants the
estimate, and it is a change to a persisted format — the same objection that
stopped index *names* being printed in a plan two entries ago, and the same
reason to record it rather than do it in passing. Recorded here as the
direction, not started.

**Print the per-index breakdown always.** It repeats the total when there is
one index, which is noise on the common path.

## Evidence

**The breakdown carries information, demonstrated with counts that differ.** A
node started over a keyspace holding two `notes` rows, with an index on that
table and another on an empty second table:

```
slate-serverd: building 2 indexes before serving: by_body, by_tag
slate-serverd: built 2 index entries in 200.7ms
slate-serverd:   by_body: 2
slate-serverd:   by_tag: 0
```

Two *full* indexes over one table would have written the same count as each
other, so a wrong breakdown would have looked right; two tables of different
sizes is what makes the numbers separable. The test uses that shape for that
reason.

**Three mutations, each caught by a named test:**

| mutation | test that failed |
| --- | --- |
| report `0` instead of the summed count | `a_node_builds_an_index_added_since_the_rows_were_written` |
| drop the per-index lines | `a_backfill_over_two_indexes_reports_each_one` |
| print the *total* against each index name | `a_backfill_over_two_indexes_reports_each_one` |

**The second one is why the new test exists.** Dropping the breakdown left all
four existing tests green: the one that asserts on backfill output builds a
single index, and the breakdown deliberately prints only when there is more
than one. A mutation that causes no failure is a missing test.

**The ordering is not assumed.** `migrate::apply` pushes one
`entries_written` entry per `BuildIndex` step as it walks `plan.steps`, and
`building` is collected from the same `plan.steps` with the same filter, so the
zip is aligned by construction. Read rather than inferred from the output,
because output that happens to line up on two elements proves little.

**Suites:** `cargo test -p slate-serverd --no-fail-fast` — 8 test binaries, all
ok. `cargo fmt --all -- --check` clean. `sh scripts/check.sh` — 20 of 20.

**A note on the run.** The serverd suite first died with a linker failure, which
is the ENOSPC signature `CLAUDE.md` describes; 5.6M free. Reclaimed to 11G and
it passed. Third time today, and the first where I recognised it from the
symptom rather than from `df`.

## What this does not do

- **`--plan` still says nothing about size.** That is the decision, not an
  omission, and the argument above is what to attack if you disagree.
- **No estimate from a persisted count.** Named as the direction and not taken;
  it needs a format change to `TableState`.
- **The per-index line has no timing.** Entries, not seconds. A per-index
  duration would need timing inside `migrate::apply`, which currently returns
  counts and lets the caller time the whole thing — a bigger change for a
  number an operator can mostly infer from the counts.
- **Nothing was measured about the scan's cost**, so "a preview that pays the
  price of the thing it previews" is an argument from what the code does, not
  from a timing. On a large table over object storage I would expect it to
  dominate; I did not build the table to prove it.
