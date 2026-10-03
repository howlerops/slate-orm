# Column grants, designed before built, because the projection is not how columns leave

- **Date:** 2026-10-03
- **Author:** Claude Code (session: picking up from the handoff prompt, task T4)
- **Touches:** `docs/column-grants.md` (new), `docs/views.md`, `docs/agent-handoff.md`
- **Kind:** docs

## What changed

A design note for column-level grants, in the style of `views.md`. It builds
nothing. It decides seven things:

1. What an existing table-level grant means once columns exist: every column,
   including columns added later.
2. A caller's reference to a column it cannot read is refused, wherever in the
   query the reference appears.
3. System-added predicates are decoded, used, and then dropped from the row.
4. Every path a row leaves by, mapped, with what each will do.
5. `Explain` and `analyze` are refused for narrowed callers.
6. Column grants are read-only, and narrowed readers may not replace whole
   rows.
7. A hidden column draws the same error as a column that does not exist.

It leaves two questions open. `views.md` and the handoff prompt now point at
it.

## Why

The handoff named T4 as the real multi-tenant blocker and said to write the
design first. The reason that instruction was right showed up as soon as the
current behaviour was measured. The obvious design, a column list on `Grant`
checked against the projection, fails before it is built. A caller projecting
only `name` today also receives the RLS policy's column, receives `salary`
whenever it filters on `salary`, and receives the whole row from `get`. Table
grants make all of that harmless. Column grants would make each one a
disclosure. A survey of the code found thirteen such paths. The note's §4 is
the map of them, and it is the part the build has to satisfy.

## Alternatives rejected

- **Build first, with a projection check, and patch leaks as found.** This
  would ship a boundary with known holes. A privilege feature that fails open
  in the cases nobody wrote a test for is worse than none, because it is
  trusted.
- **Make a table-level grant an exclusion list** ("read, except `salary`").
  This adds deny precedence to a model that is additive by construction. It
  also fails open on schema change, because a new column would be visible to
  everyone until the exclusion was extended. §1 of the note argues this.
- **Null hidden columns instead of refusing references to them.** This leaves
  the filter as a probing channel: binary search on `salary > x`. It also
  creates a null that no caller can tell from data. §2.
- **Narrow `Explain` and `analyze` output rather than refuse them.** Each would
  need a printer written to be partial, and partial printers drift. §5.
- **Answer `AccessDenied: column salary`** for a hidden column. That is a
  schema oracle of the same class as security-review finding 8. §7.
- **Design column-level writes now.** None of their open questions is needed
  for the use case that motivates the feature, so they are deferred, and the
  config refuses non-`read` column grants at load. §6.

## Evidence

The measurement in the note's opening section came from a throwaway kernel
test, deleted afterwards. A role with table `Read`, projecting `[name]` over
`staff (id, name, salary, owner)` with policy `owner = :principal`, got these
results:

```
project [name]                         -> [1, "ann", Null,   7]  [2, "bob", Null, 7]
project [name] where salary > 100000   -> [2, "bob", 120000, 7]
get(1)                                 -> [1, "ann", 90000,  7]
```

The thirteen-path map in §4 comes from a read-only survey of the code, with
file and line for each path. Three of its claims were checked against the code
directly before the note relied on them:
- `plan_hinted` builds `output_columns` from the secured predicate;
- `SecurityCatalog::grants` has no column dimension;
- `get` returns the unchecked row filtered only by the row filter.

The rest of the map has not been independently verified. Its own last section
says the build is expected to falsify it in places.

## What this does not do

- **It builds nothing.** No step of the build order has started. The paths in
  §4 still return what they returned.
- **The map is a survey, not a proof.** The defence against a missed path is
  the sentinel-bytes oracle the note specifies as the primary test, and that
  oracle does not exist yet.
- **The Postgres comparisons are recalled, not tested**, as in `views.md`.
- **No owner reviewed the semantics.** §1 (what a table grant means) and §7
  (hidden columns indistinguishable from missing ones) are decisions a
  repository owner may want to overrule before step 1 is built. The note gives
  the argument for each so that overruling it is a decision rather than a
  rediscovery.
