# Four say absent, one says present

## What changed

No code. The sweep the previous entry named as not done, done — and the finding
it recorded, sharpened.

`2026-09-20-the-row-that-is-both-there-and-not.md` closed with: *"It does not
check the other write paths. `delete_if_unchanged`, `update_where` and
`delete_where` all reach rows through filters of their own… `update_where` is
the one I would look at first."*

They are checked now. `update_where` and `delete_where` are **fine**, and the
defect is narrower and stateable in one line.

## Why

A gap named in a "what this does not do" is a gap until somebody looks. I
guessed `update_where` was the likeliest second instance; it is not one.

## Evidence

Against a soft-deleting table holding row 1 retired and row 2 live, each write
run from a clean state:

```
state: [(1, True), (2, False)]   live: [2]

update_where(all)      -> affected=1      after: [(1, True), (2, False)]
delete_where(all)      -> affected=1      after: [(1, True), (2, True)]
delete_if_unchanged(1) -> NotFound        after: [(1, True), (2, False)]
```

- **`update_where` skips the retired row silently.** Its predicate matched both
  and it touched one. That is the documented meaning of a soft delete — a
  retired row is invisible to a path that did not ask for it — and it is what
  the entry guessed might instead be an error. It is not.
- **`delete_where` skips it too**, retiring only the live row. Correct twice
  over: the other one is already retired.
- **`delete_if_unchanged` errors**, like every other write that names a primary
  key.

**So the split is not "predicate writes versus the rest". It is: a write that
names a primary key sees the retired row or not, and the five of them do not
agree.**

| path | names a key | a retired row is |
| --- | --- | --- |
| `insert` | yes | **present** — `AlreadyExists` |
| `insert(upsert)` | yes | absent — `NotFound` |
| `update` | yes | absent — `NotFound` |
| `delete_if_unchanged` | yes | absent — `NotFound` |
| `update_where` | no | invisible, skipped |
| `delete_where` | no | invisible, skipped |

Four of the five key-named paths deny the row exists. One discloses it. The
predicate paths are consistent with each other and with the convention, and
were never the problem.

That is a better statement of the defect than the previous entry's, which
framed it as upsert-versus-insert. The shape is: **`insert`'s duplicate-key
check reads the row unfiltered, and everything else asks `permits_row`, which
defaults to `Deleted::Hidden`.** One read is soft-delete-aware and one is not,
and which of the two a path uses was never decided — it fell out of whether the
path needed the previous row's *contents* or only its existence.

**The defect bit this probe's own setup**, which is worth recording as the
operational cost. `reset()` upserted the two fixture rows between experiments
and failed on the second run: row 1 was retired and could not be written. The
probe now purges before seeding. A test fixture cannot reuse a key after
retiring it without a privileged purge, and neither can an application.

## Alternatives rejected

**Fold this into the previous entry.** It is one commit old and nothing has
read it. Rejected for the reason that entry gave about a different edit: the
sweep is a separate piece of work with its own result, and an entry that grows
new evidence after the fact stops being a record of what was known when.

**Write the table into `docs/` as well.** It describes a defect, and a document
that describes current behaviour would have to be rewritten the moment the
behaviour is fixed. The ledger is where a finding with an open question lives.

**Probe the remaining paths too** — the cascade walk, `purge_deleted` itself,
and writes inside an atomic batch. Not done; named below.

## What this does not do

- **Still no fix**, and still for the reason the previous entry gives: whether
  restoring requires `ReadDeleted` is a policy decision.
- **Three paths remain unprobed**: a cascade delete into a soft-deleting child
  whose row is already retired, a write to a retired row inside an atomic
  batch, and `purge_deleted` on a table whose rows a policy hides. The first
  two go through the same `write_many`, so I expect them to behave as the
  key-named paths above; that is an expectation, not a measurement.
- **No test pins any of this.** Same reason as before: a test of the current
  behaviour would pin the defect.
- **The predicate paths were probed at one shape.** A predicate matching *only*
  the retired row, rather than both, would report `affected=0` — which I did not
  run, and which is the case where "silently skipped" is most likely to
  surprise somebody.
