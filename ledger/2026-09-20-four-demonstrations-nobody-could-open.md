# The security review named four probes, and none of them existed

- **Date:** 2026-09-20
- **Author:** Claude, sweeping for stale citations after being told to close the untested
- **Touches:** `docs/security-review.md`, `scripts/check_cited_tests.py` (new), `scripts/test_check_cited_tests.py` (new), `scripts/check.sh`, `.github/workflows/ci.yml`
- **Kind:** docs

## What changed

`docs/security-review.md` said "Demonstrated by
`a_cascade_from_a_shared_parent_crosses_the_tenant_boundary`", and three more
like it, in the present tense. All four were renamed or merged by the commits
that fixed the findings they demonstrated, so a reader following any of them
found nothing. The document now names the tests that exist, with the originals
kept beside them as provenance. `scripts/check_cited_tests.py` refuses a `docs/`
file that cites a test nobody can open.

## Why

A security review is read by somebody deciding whether to trust a claim, and
the only thing that makes "impact: high — cross-tenant data destruction"
checkable is being able to run the test named under it. Four of those names
had gone dead:

| cited as | what it is now | killed by |
| --- | --- | --- |
| `a_cascade_from_a_shared_parent_crosses_the_tenant_boundary` | `a_shared_parent_with_a_tenant_scoped_child_is_refused` | `46ec173` |
| `a_restrict_refusal_discloses_another_tenants_row` | the `Restrict` arm of the same test | `46ec173` |
| `explain_recovers_a_value_from_a_tenant_the_caller_cannot_read` | `granting_explain_reopens_the_recovery_in_full` | `0d0fcf3` |
| `the_first_copy_of_a_duplicated_identity_header_wins` | `a_duplicated_identity_header_is_refused_rather_than_resolved` | `d6d6ca8` |

Every one was killed by its **own fix**, which is the part worth noticing. The
probes asserted the hole; fixing the hole meant inverting the probe; inverting
it meant renaming it. So the document was guaranteed to go stale at exactly the
moment its findings were closed, and the banner at the top saying they were
fixed was added by the same sessions that broke the names below it.

The document's framing sentence was stale for the same reason — "The probe
tests assert current behaviour, so they pass today. Each one names what to
change it to once the finding is fixed" describes a document whose findings are
all open. It has been rewritten to say what is true now.

Two of the four also lost information, not just a name. The `EXPLAIN` recovery
is still in the suite — the fix gated the action rather than blurring the
estimate, so a caller granted `Explain` recovers exactly as much as before —
and the document read as though the recovery had been closed. That is the
opposite of what an operator deciding whether to grant it needs to know.

## Alternatives rejected

**Delete the dead names rather than keep them as provenance.** Shorter, and it
loses the thing that makes the review readable in a year: a reader who searches
for the name in an old commit message, an issue, or their own notes has to be
able to land somewhere. Keeping both costs a clause.

**Guard every Markdown file, not just `docs/`.** Measured first, and it is the
reason this guard is as narrow as it is. The naive rule — every backticked
snake_case name in `docs/` + `ledger/` + `CLAUDE.md` must resolve — flags **38
names, of which 5 are real: an 87% false positive rate.** Four sources: the
ledger cites dead names *on purpose* (a dated record, and
`2026-09-15-pages-that-do-not-shift.md` literally says "There *was* a test
called X"); clippy lints and tonic builder methods are not tests; Python tests
are cited without their `test_` prefix; and a passage naming the dead beside
the live is correct. Narrowing to `docs/`, to names of five words or more, and
to paragraphs with no resolvable name in them takes it to **5 flagged, 5 real**.

**Ship the naive rule anyway and add a deny-list.** The deny-list would have
held two clippy lints — and `CLAUDE.md` says in its own text that CI's clippy
is newer than this container's and gains lints constantly, so the list would go
stale by design.

**Do not guard at all; this was a one-off sweep.** What I would have argued
yesterday. Against it: nothing compiles a document, the failure was invisible
for a week, and it is *structurally recurring* — every future finding fixed by
inverting its probe breaks its own citation the same way.

## Evidence

The guard is falsifiable in the only way that counts, against the document
before and after:

```
$ rule over docs/security-review.md @ HEAD (pre-fix)
  a_cascade_from_a_shared_parent_crosses_the_tenant_boundary
  a_restrict_refusal_discloses_another_tenants_row
  explain_recovers_a_value_from_a_tenant_the_caller_cannot_read
  a_restrict_refusal_discloses_another_tenants_row
  the_first_copy_of_a_duplicated_identity_header_wins
$ rule over docs/security-review.md (fixed)
  (nothing)
```

**The first two attempts at the rule flagged 5 both before and after**, because
the fix deliberately keeps the old names. That is the measurement that decided
the design: a rule with no discriminating power over the exact change that
fixes the defect is not a guard. Paragraph-scoping is what gave it power, and
it is why the document was then restructured to put each live name in the same
paragraph as its dead one — better prose and a clean check, from the same edit.

Eight tests, each building a tree and running the real `main()` over it —
`scripts/test_check_cited_tests.py`, 8 passed. Mutation-tested through
`scripts/mutate.py`, six mutations, every one caught by the test named for it:

```
ok  the threshold drops to four words          -> a four-word name is below the threshold and is ignored
ok  the paragraph rule demands every name ...  -> a dead name beside a live one is history, and passes
ok  the paragraph rule is dropped entirely     -> a dead name beside a live one is history, and passes
ok  a python test's prefix-stripped alias ...  -> a python test cited without its test_ prefix resolves
ok  the scope widens past docs/                -> the ledger is out of scope, because an entry is a dated record
ok  a finding no longer exits non-zero         -> a doc naming a test that does not exist fails, ...
```

That run is also how the bytecode defect in `mutate.py` was found; see
`2026-09-20-the-fourth-way-a-mutation-lies.md`. The first attempt at it
attributed a mutation to the wrong test, which is the only reason anybody
looked.

`scripts/check.sh` 21 → 23 checks, all passing; `ruff` clean;
`scripts/test_check_sh.py` accounts for both new CI steps.

## What this does not do

**It does not check that the test says what the prose says it says.** A
citation resolves if a function with that name exists anywhere in the tree. A
document claiming `a_shared_parent_with_a_tenant_scoped_child_is_refused`
demonstrates something it does not would pass. That is a much harder check and
this is not a step towards it.

**It does not cover `ledger/`, `CLAUDE.md`, `README.md` or `site/`.** The
ledger is exempt on principle. The other three are exempt on the weaker ground
that they cite lints and commands rather than tests *today* — an observation
about the current tree, not a property of those files, and if one of them
starts citing tests the guard will silently not cover it.

**Four words is a guess with one data point behind it.** No test in this
repository is named in four words, so the threshold costs nothing here. A
four-word test name would be missed, silently.

**It found nothing outside `docs/security-review.md`.** Eight other documents
in `docs/` were scanned and were clean, so the sample supporting "this
recurs" is one file with four instances, not four files.

**The two `RESTRICT`/`CASCADE` names were merged into one test, and I did not
verify the merged test covers everything the two separate ones did.** It loops
over both actions and asserts the same error, which is why the fix commit
called it a collapse rather than a deletion — but I read that commit's diff
rather than running the old probes, which no longer exist to run.
