# Explaining a plan is its own action, because a plan describes rows you cannot read

- **Date:** 2026-09-13
- **Author:** Claude (Opus 5)
- **Touches:** `slate-kernel` — `security.rs`, `read.rs`, `record.rs`; `slate-serverd` — `security.rs`; `docs/correctness.md`, `docs/security-review.md`
- **Kind:** security

## What changed

`Action::Explain` joins the four data actions, and `explain`, `explain_join`
and `explain_chain` authorise it on every table they touch. `Action::ALL` keeps
meaning the four data actions and now excludes it by name;
`Action::EVERYTHING` is the five. The daemon's TOML gains `explain` and
`everything` alongside `all`. Nineteen test grants and two config fixtures
were widened to keep explaining.

## Why

Security review finding 3. Statistics are gathered as superuser — correctly,
since per-policy histograms make the planner optimise for a table nobody is
querying — and every caller's plan is costed against them. A row count is a
number, but a histogram bound is a *value sampled out of the table*, and
`estimated_rows` is on the wire. Tenant A, able to read zero rows of `payroll`,
recovers tenant B's smallest salary exactly by binary search over
`EXPLAIN ... WHERE salary >= v`, and the shape of the whole distribution from
eight more probes.

Before this, `Action::Read` carried the ability to explain. The two are not
versions of the same permission: one returns rows the policy admits, the other
returns a summary of rows it does not.

## Alternatives rejected

**Quantise `estimated_rows` to a coarse ladder before it leaves the process.**
Non-breaking, and rejected because it raises the cost of the search rather than
removing it — the leak survives at lower resolution — while degrading the
number `EXPLAIN` exists to report. It also reads as a fix, which is worse than
a gate that visibly is one.

**Withhold `estimated_rows` from the wire entirely.** Closes it completely and
costs every caller, privileged or not, the cardinality figures that are most of
`EXPLAIN`'s value. The disclosure depends on the caller's policy hiding rows;
the remedy should too.

**Per-tenant statistics.** The only option that removes the channel rather than
gating it, keeps plans tenant-accurate, and needs no API change. Rejected for
now on cost: it is much the most work, adds per-tenant `analyze` time and
memory, and needs a policy for a tenant with too few rows to describe. It stays
the right long-term answer and this change does not block it.

**Put `Explain` in `Action::ALL`.** This was the tempting non-breaking version
and it is the one that fails: a blanket table grant plus a row policy is
*precisely* the arrangement that has the disclosure, so including it would have
left every affected deployment exactly as exposed while appearing to fix it.

**A global config flag rather than a grant.** A grant is per-role and
per-table, uses the machinery already there, and shows up where a reader looks
for permissions. A flag would be one switch for a whole node.

## Evidence

916 tests pass across the workspace, up from 908; `cargo fmt` and `cargo clippy`
clean. Two mutations:

- Neutering `authorize_explain` fails
  `explain_is_refused_to_a_caller_holding_only_read`.
- **Checking only the first table of a multi-table plan survived the entire
  suite.** That is a missing test, not a passing mutation, and
  `explain_on_one_table_does_not_carry_to_the_other_side_of_a_join` was written
  for it: Alice holds `Explain` on `directory` and only `Read` on `payroll`,
  and joining the two must not borrow the first table's grant. It asserts both
  orders, so the check cannot pass merely because the restricted table happens
  to be second. The mutation dies to it now.

`granting_explain_reopens_the_recovery_in_full` is the deliberately
uncomfortable one: it performs the original attack under a grant that permits
it and asserts the recovery still works, so the cost of the knob is a test
rather than a warning nobody reads.

## What this does not do

It gates the channel; it does not remove it. Anyone who grants `explain` on a
table with a row policy has the original finding back in full, which is why
that is a test rather than a footnote.

It does nothing about equality predicates, which never leaked —
`equality_selectivity` reads only the distinct count.

It does not distinguish a caller who can read every row of a table, for whom
`EXPLAIN` discloses nothing new, from one whose policy hides rows. Doing that
properly means knowing whether the caller's row filter is a tautology, which is
the per-tenant statistics work.

The `Action::EVERYTHING` spelling is a convenience and a hazard: it is the
right grant for a superuser-ish role and the wrong one reached for by anybody
who used to write `ALL`. The rename was chosen so that the wrong choice at
least has to be typed deliberately.
