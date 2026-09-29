# Seven more guards mutated, and a rule that only checked one direction

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-a-guard-whose-roster-was-itself.md`
- **Touches:** `scripts/check_guard_scope.py`,
  `scripts/test_check_guard_scope.py`, `scripts/check_cost_prose.py`,
  `scripts/check_site_claims.py`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

Seven more guards mutated against the real tree: `check_write_paths`,
`check_secret_types`, `check_transport_door`, `check_wasm_runtime`,
`check_handlers`, `check_none_last` and `check_guard_scope`. Six caught theirs.
The seventh found a gap in its own rule.

`check_guard_scope.py` checked that every tree a guard's docstring *claims* to
read is one it has a path into. It did not check the other direction, and both
guards making a scope claim understated it by exactly one tree —
`check_cost_prose.py` omitting `site/`, `check_site_claims.py` omitting
`docs/`. Both sentences are corrected, and the rule now runs both ways.

## Why

A scope claim that understates is read as exhaustive. `check_cost_prose.py`'s
paragraph opened "**It reads `crates/` and `docs/`.**" and the guard has read
`site/` since #288 and `README.md` since the same change. Anyone deciding
whether the site's cost prose was covered would have read that sentence and
concluded it was not — and stopped. That is `CLAUDE.md`'s "stale documentation
is worse than none", in the docstring of the guard written to enforce it, and
`check_guard_scope.py` was pointed at it and could not see it.

The direction it did check is the rarer one. A claim naming a tree the code
does not read is an over-promise, which a reader discovers by looking. A claim
omitting a tree the code does read is an under-promise, which a reader never
discovers at all, because nothing contradicts it.

## Alternatives rejected

**Require every guard to declare its scope.** Then the rule needs no
"if the docstring claims anything" exemption and twenty-five more guards get a
declaration. Rejected: it turns a check on a claim that exists into a mandate
for a claim that does not, which is a different rule with a different cost —
twenty-five docstrings to write and to keep true, for guards whose scope is
usually one obvious tree. Saying nothing is not saying the wrong thing.

**Widen `roots()` instead, so it only reports trees a guard really walks.**
The survivor's first reading was that `roots()` over-reports. It does not:
`check_cost_prose.py` genuinely reads `site/`, through `SITE = ROOT / "site"`
threaded into three functions. The claim was wrong, not the parser.

**Leave the two sentences and add the rule later.** The rule is four lines and
the sentences were the finding. Splitting them would leave the entry claiming a
rule it had not run against the thing that motivated it.

## Evidence

**The survey, batch three.** One real-tree mutation each, through
`scripts/mutate.py` with `scripts/mutate_guard.py` as the command:

| guard | the mutation | caught |
| --- | --- | --- |
| `check_write_paths` | `delete_where` renamed to `delete_matching` | yes |
| `check_secret_types` | `Credentials` in `s3.rs` derives `Debug` | yes |
| `check_transport_door` | the Go door stops taking `grpc.DialOption` | yes |
| `check_wasm_runtime` | `slate-kernel` asks tokio for `rt-multi-thread` | yes |
| `check_handlers` | an authorisation removed above `join_from_proto` | yes |
| `check_cost_prose` | `POINT_READ_COST = 1.0` in `docs/` becomes `3.0` | yes, once the marker was moved |
| `check_guard_scope` | a claim gains a tree the guard does not read | yes |
| `check_guard_scope` | a claim loses a tree the guard does read | **no** |

Each caught one names the finding precisely — for instance
`check_handlers: service.rs:2654: 'join' calls 'join_from_proto' with no
authorisation in the 4 lines above it`, and
`check_secret_types: 'Credentials' at crates/slate-slatedb/src/s3.rs:57 derives
'Debug' and holds a secret.`

**Two mutations were rejected before they counted, and both are worth naming.**
Renaming `fn authorize_join_inputs` to `fn check_join_inputs` survived — but
that renames only the declaration, leaving both call sites saying
`authorize_join_inputs`, so the guard's answer is correctly unchanged. Not a
finding; not a change. And dropping `go-version: "1.24"` to `"1.23"` survived
`check_toolchain_pins`, which is right: that guard's stated invariant is about
a pin *existing*, not its value, and its docstring says so. The third cause
`mutate.py`'s survivor message names — the mutation may not be a change — was
the answer twice in one batch.

**The measurement behind the new rule.** Of twenty-eight guards, three make a
scope claim `CLAIMS_READ` can parse. Two of the three understated:

```
check_cost_prose.py    claims [crates, docs]           reads [crates, docs, site]
check_site_claims.py   claims [clients, crates, site]  reads [clients, crates, docs, site]
```

Two of two, which is why the rule went in rather than the sentences alone.

**The rule.** `ledger/mutations/20260929T083652-scripts-check-guard-scope-py.json`,
3 cases, no survivors — the new direction, the exemption that keeps it off
guards with no claim, and the old direction, each mutated separately.
`python3 scripts/test_check_guard_scope.py` reports `15 passed, 0 failed`, up
from 13.

The real-tree mutation:
`ledger/mutations/20260929T083705-scripts-check-cost-prose-py.json` — putting
`check_cost_prose.py`'s sentence back to `crates/` and `docs/` is now refused:
`scripts/check_cost_prose.py's docstring lists the trees it reads and 'site/'
is not among them, while the file builds a path into it.`

`sh scripts/check.sh` reports `83 passed, all of them`.

## What this does not do

**Eleven guards still have no real-tree mutation.** Seventeen of twenty-eight
now. Findings so far: one gap between guards, one guard checking itself, one
mention-counts-as-a-use pattern, one exemption marker over-reaching, and one
rule checking only half its property. Five in seventeen.

**The scope rule only sees `<root> / "…"` paths.** `roots()` is an AST walk for
that one shape. A guard reading a tree through `subprocess.run(["git", "grep",
… , "docs"])` or a glob string builds no such path, so its scope is invisible
to both directions of the rule. `check_cited_files.py`, added this morning,
shells out to `git ls-files` and would be one — it makes no scope claim, so the
rule leaves it alone, which is the exemption doing its job and also hiding the
limitation.

**Nothing checks the two sentences beyond their tree lists.**
`check_cost_prose.py`'s now says "plus the Markdown named in `README_GLOBS`
below", which is prose about a constant, checked by nobody. If `README_GLOBS`
is renamed the sentence rots and only a reader notices — the same class this
entry is about, one level down.

**`check_toolchain_pins` and `check_handlers` were mutated at one site each.**
`check_handlers` guards fourteen findings' worth of call sites and this
exercised one of them; the others are covered by its fixture tests and not by
the real tree.
