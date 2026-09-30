# The helpdesk on SlateDB, and the two findings that needed a running one

- **Date:** 2026-09-30
- **Author:** Claude Code (session: close the twelve open caveats)
- **Touches:** `examples/helpdesk/`, `.github/workflows/ci.yml`, `scripts/test_check_sh.py`, `CLAUDE.md`, `scripts/check.sh`
- **Kind:** feature

## What changed

`examples/helpdesk` finished. The slice committed earlier today had the
domain, the security catalog and seven operations; this adds the four that
were named as unwritten, the two that turned out to be findings, and the
runner that puts the whole thing on SlateDB over an object store.

New in the service: `inbox` (keyset-paged), `most_urgent` (sorted, bounded),
`workload` (one grouped read, per assignee, with the unassigned pile),
`search` (two text indexes and an `Or`), and `apply_schema` (the migration,
planned then applied, at boot).

New around it: `examples/over_slatedb.rs`, a runner that migrates, seeds,
serves, **closes the store and opens a fresh one over the same prefix**, and
re-asks every question; `run.sh`, which builds it `--release` and optionally
stands up a real signed S3 server; a `helpdesk` job in `ci.yml`; and a
`README.md` with the findings in one table.

Sixteen tests, up from eight. `run.sh --smoke` reports sixteen checks.

## Why

The crate's central claim was `Helpdesk<S>` being generic over `KvStore` —
that the same service runs over `MemoryStore` in the tests and over SlateDB
in production and answers the same. This morning's entry recorded honestly
that only `MemoryStore` had been run, and that a generic parameter is a claim
about a surface rather than a result. The user's ask was to test this "in a
real app using slatedb", and a real app that never touches an LSM is not one.

Running it found two things reading could not have.

**A paged read cannot be a sorted one.** An inbox wants most urgent first.
`Query::after` pins the access path to the table's own key range, and a query
sorted into an order the primary key does not give is refused when it runs:
*"the rows are re-ordered after they are read and a key does not say where
the page ended"*. So `inbox` pages in `(tenant_id, id)` order and
`most_urgent` is a separate unpaged read with a limit. This is a **good**
refusal — the alternative is a second page that silently skips and repeats
rows — and it is still a design constraint an application meets on its first
screen, which is why it is written down. It is documented on `Query::after`;
what was not obvious from reading is that the most ordinary read in a ticket
system hits it.

**A write a policy forbids says two different things.** An insert the row
policy rejects says *"row-level security forbids writing this row to table
`tickets`"*. An update it rejects says *"table `tickets` has no row with this
primary key"* — indistinguishable from a row that is genuinely gone. Found by
the runner printing its own refusal messages beside each other.

That second one is very likely deliberate: it is the same disclosure decision
`Helpdesk::by_reference` makes on purpose, because telling a caller that a row
they may not touch *exists* is a leak. The read path's version is documented
and reasoned about; the write path's is not, so an application meets it as a
puzzle. Recorded as a finding about the *documentation* rather than about the
behaviour, and the test is written so that changing either is a deliberate
test change.

The text index also got more expensive than this morning's entry said. That
entry called the workaround "writing the column list twice". It is more: a
`TableDef` is immutable once built and `Record::table()` is what every
`Records` method hands the store, so an index the derive did not declare is
an index **no write maintains** — the silent-empty-index failure
`ledger/2026-09-15-a-new-index-returns-nothing.md` is about. The table the
writes use has to be the hand-written one. What survives is the row codec:
`Indexed(Ticket)` implements `Record` with a hand-written `table()` and
forwards `to_row`/`from_row` to the derive, so only the schema is duplicated.
The correction is in the code and in the README; the earlier entry stands as
written, per `ledger/README.md`.

## Alternatives rejected

**Leaving `security()`'s two policies as `Expr::True`,** which is what the
morning entry recorded as a caveat. Rejected because a policy that admits
everything demonstrates nothing, and the user's ask named RLS. The write rule
is now "unassigned, or assigned to me", a `supervisor` role exists to move
work between agents, and three tests turn on it. Writing it found the
refusal-shape finding above, which `Expr::True` never would have.

**One policy with `Expr::Or` instead of two policies.** Both work — policies
are permissive and combine with `OR`, as in PostgreSQL. The pair was chosen
because each half is separately named, so a refusal can say which rule was
expected to admit the row, and because a fourth rule becomes an addition
rather than an edit to an expression somebody has to re-read. The cost is
that the disjunction is not visible in one place, which is why `security()`'s
docstring says it out loud.

**A `[[bin]]` rather than a cargo example.** A binary would have put `tokio`
and an LSM into the *library's* dependency table, and the library asks for
neither: the service is `async` and awaits whatever store it was handed.
`scripts/check_wasm_runtime.py` is the check that makes that visible — a
library taking a non-dev tokio has to be rostered with a reason — and the
honest reason would have been "because the runner lives in the same package",
which is a packaging accident, not a fact about the crate. A cargo example is
built with the dev-dependencies, so the runner gets its runtime and the
library keeps none.

**Building the runner in debug.** Measured elsewhere and recorded in
`CLAUDE.md`: the nine `slate-slatedb` examples are 131 MB at `--release`
against well over 1.4 GB in debug, which is `-C debuginfo=2` and nothing
else. A debug build of this one on the development container dies with a
linker `Bus error` or `LLVM ERROR: IO failure on output stream` — both
ENOSPC in disguise. `run.sh` builds `--release` unconditionally; CI has room
and does not notice.

**Making `--s3` the default.** Rejected because the default would then need a
server, and the differences an S3 server actually introduces — conditional
writes, error shapes, latency — are ones this runner asserts nothing about.
`LocalFileSystem` is an `ObjectStore` and the LSM above it is the same LSM,
with the same SSTs, WAL, manifest and compaction;
`crates/slate-slatedb/examples/bucket_layout.rs` makes the same choice and
gives the same reason. `--s3` runs the identical binary against `s3s` for
anybody who wants the other layer, and `minio` is the CI job that owns it.

**Running the whole thing in `check.sh`.** `check.sh` promises no built
binary, no browser, no container and no network, which is what makes it worth
running before every commit. A release build and a bucket is none of those.
It is in `ci.yml` with an `ELSEWHERE` entry saying why, which is the shape
`scripts/test_check_sh.py` enforces both ways.

**Writing the migration into `Helpdesk::open`.** Rejected because migrating
is a decision: a process that is one of six replicas should not quietly
create indexes because it started. `apply_schema` is a separate call, the
runner plans before it applies, and it checks that a second apply has nothing
to do — which is what makes it safe to run on every boot rather than behind a
flag somebody forgets.

## Evidence

**Sixteen tests over `MemoryStore`** (`cargo test -p slate-helpdesk`), all
passing. Five are findings rather than features; the README's table maps each
to its test. The new ones:

- `the_hand_written_tickets_table_matches_the_derived_one` — the cost of the
  text-index workaround made into a guard. Compares name, id, primary key,
  tenant column, soft-delete column, schema version and every column's
  (name, type, nullability, managed) against `Ticket::table()`, then asserts
  the *only* difference is the two text indexes. A field added to `Ticket`
  and not to `TICKETS_TABLE` turns it red; nothing else would say so.
- `a_search_finds_a_ticket_by_a_word_in_its_body` — the subject index, the
  body index, two terms as a conjunction, a blank box matching nothing, and
  the tenant wall holding over an index the query never names.
- `the_inbox_pages_without_repeating_or_skipping_a_ticket` — seven tickets,
  three a page, three pages, seven distinct references; then a close and six.
- `most_urgent_is_sorted_and_the_paged_read_is_not` — inserted worst-last so
  an unsorted read cannot pass by accident, and `limit(1)` returns the worst
  rather than the first found.
- `a_sorted_inbox_page_is_refused_rather_than_silently_misordered` — asserts
  the refusal *and its sentence*. It first passed against `is_err()` alone,
  which the same call also satisfies with no `limit`; the limit is set two
  lines above precisely so the sort is the only thing left to refuse, and the
  assertion now names the sentence so a pass cannot come from the other one.
- `the_workload_rolls_up_per_agent_and_keeps_the_unassigned_pile` — two
  groups, the null key present, 150 + 25 = 175 hundredths with no lost cent.
- `an_agent_cannot_push_work_onto_a_colleague` — the refusal, the supervisor
  succeeding, and then the agent locked out of a ticket that is no longer
  theirs. The assertion names `row-level security`; it first passed against
  a substring list that did not include the real message, which is how the
  real message got read.
- `a_write_a_policy_forbids_says_two_different_things` — the second finding,
  both halves, plus the supervisor succeeding so that neither refusal can be
  about a missing row or an odd table.

**Sixteen checks over SlateDB** (`sh examples/helpdesk/run.sh --smoke`), all
passing, over a `LocalFileSystem` object store. The migration reports eleven
steps — four `Register`s and seven `BuildIndex`es, including
`tickets_by_subject_term` and `tickets_by_body_term` — and a second plan is
empty. After the store is closed and reopened: the schema is still there, the
tickets are, the comment is, the roll-up still sums to 150, the closed ticket
is still closed, and **the text index still answers**, which is the one worth
naming because an index that exists and is empty is a failure this repository
has met.

**The same sixteen over a real signed S3 server**
(`sh examples/helpdesk/run.sh --smoke --s3`), which stands up `s3s` with
`s3s-fs` behind it and points the identical binary at it through
`S3Config::from_env`. Same result: `store: s3 bucket slate-helpdesk`, eleven
migration steps, sixteen checks. Run by hand, once; nothing re-runs it, which
is the first caveat below.

**The two refusals, as the runner printed them side by side:**

```
ok    an agent cannot hand a ticket to a colleague
        (row-level security forbids writing this row to table `tickets`)
ok    and then the agent cannot touch it
        (table `tickets` has no row with this primary key)
```

That is finding 5, and it is in this entry because the two lines were next to
each other in one transcript.

**No mutation run.** Nothing in this crate is a guard or a check, and its
tests are the assertions rather than a thing to assert about. What is
available instead — break the surface and watch a *named* test fail — is
exactly what the five finding-tests are built to do, and each carries a
failure message saying so.

**Not measured.** No number here is about speed and the runner takes no
timings. The `--smoke` size was chosen as the smallest that still pages more
than once and rolls up more than one group, not to be fast.

## What this does not do

**It does not run `--s3` anywhere automatic.** The CI job is `--smoke` over a
filesystem object store. The S3 path works — it is the same binary and
`S3Config::from_env` — and nothing re-runs it, so it can rot the way any
untriggered path can. `minio` is the job that owns S3's real differences and
it does not run this application.

**The reopen is a clean close, not a crash.** `SlateStore::close` flushes,
so the second half reads what was durably written rather than what recovery
could reconstruct. A crash test is `crates/slate-slatedb`'s and this does not
duplicate it — which means "the WAL replays correctly for this schema" is
untested here.

**Five findings are still not an audit.** They are what a handful of ordinary
requirements happened to hit, and two of the five only appeared once the code
ran. What else the derive cannot say was not enumerated.

**The roll-up decodes positionally.** `read_workload` maps
`group.values[0]` to the count and `[1]` to the sum, and the only thing tying
those positions to those meanings is the aggregate list passed a few lines
earlier. Every unreadable shape is an error rather than a zero, which is the
best available here, but the coupling is real and a grouped read on this
surface has no typed result.

**Nothing asserts which plan runs.** `most_urgent` is sorted the way
`tickets_by_priority` already stores, so the planner *should* serve it without
a sort. That is a claim, not a result: checking it needs
`Records::explain_records` and an assertion about the plan shape, which is a
different exercise.
