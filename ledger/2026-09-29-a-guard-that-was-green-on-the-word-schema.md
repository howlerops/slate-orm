# A guard that was green on the word "schema"

## What changed

`scripts/check_generated_is_used.py`, with `scripts/test_check_generated_is_used.py`
beside it, both wired into `scripts/check.sh` and the `guards` job in
`.github/workflows/ci.yml`.

The roster is read out of `ci.yml`: every path `codegen.py --check` is given is
a file the rule covers, so a sixth generated declaration comes under it by
being generated rather than by being remembered. Two rules per file:

1. Something in the file's own package imports it, in an import shape.
2. It reaches the wire, by the call named in `REACHES` — or the entry says
   why nothing does, which is the browser catalog's case.

`REACHES` also fails on an entry for a file `ci.yml` no longer generates, so
the roster cannot outlive what it describes.

## Why

`ledger/2026-09-18-the-catalog-writes-the-declaration-nobody-should-type.md`
left this open:

> `--check` proves the file matches the catalog, not that anybody imports it.
> What proves that is G1-G3, which are mutations rather than tests: no check in
> CI would notice if an adapter stopped calling `declaring`.

A generated declaration nobody reads stays perfectly in step with a catalog
nobody consults. `--check` is green the whole time, and every request from that
adapter goes out with no schema claim on it — which is the exact failure the
fingerprint exists to prevent, one level up.

Rule 2 is the half that matters and rule 1 alone would pass it: importing a
generated module for one constant while declaring nothing is a real shape, and
it is the one the caveat names.

**The interesting part is that the first draft of rule 1 was itself the bug it
was written to catch.** It grepped the bare module stem across the whole tree.
`\bschema\b` occurs in **483** of this repository's files — it is an ordinary
English word here — so rule 1 passed on the vocabulary rather than on any
import. A guard green for a reason unrelated to what it checks is precisely a
declaration in step with a catalog nobody consults.

Two qualifiers fix it, and both were bought by a failing test rather than
reasoned into place. Measured on the Go adapter:

| search | files |
| --- | --- |
| bare stem, whole repository | 483 |
| bare stem, scoped to the package | 7 |
| import-shaped, scoped to the package | 3 |

Those 3 are `handlers.go`, `main.go` and `roundtrips.go` — the real importers.
The scope does most of the work and the shape finishes it; neither alone is
enough, and there is a test for each.

The shape has two arms because Go's import lines carry no keyword of their own:
one for `from`/`import`/`require` followed directly by the module, one for a
quoted path. The first arm allows *nothing* between the keyword and the stem,
which is not fussiness — a draft allowing any non-quote text matched the
sentence "these labels come **from** the generated **catalog**", and a comment
mentioning a module is the precise thing rule 1 must not read as importing it.

## Alternatives rejected

**Roster the importer by hand, per generated file.** `REACHES` already names
the file that puts each declaration on the wire, so the importer could have
been a second field and the grep dropped entirely. Rejected because it inverts
the cost: the point of reading the roster out of `ci.yml` is that a sixth
generated file is covered the day it is generated, and a hand-rostered importer
means a sixth file is covered the day somebody remembers. It would also have
hidden the finding above — a rule that never greps cannot be caught grepping
the wrong thing — which is a bad trade for a repository that has now been
surprised twice by a check passing on the wrong evidence.

**Scope to the repository root when no manifest is found.** `examples/retention`
has no manifest, and falling back to the root would have been the tidier line.
It also puts the rule straight back to 483 matches. The fallback is the file's
own directory instead: narrow, and wrong in the loud direction — an importer
that sits one level up is not found, somebody reads the message and widens the
rule. A wrong-but-wide scope passes forever. The mutation returning `"."` from
that arm survived the first run, because every fixture package carried a
manifest and the arm was never taken; `a package with no manifest scopes to its
own directory` exists because of that survivor.

**Parse imports properly, per language.** A Go import block, a TypeScript
`import` statement and a Python `from … import` all have real grammars, and
three small parsers would be exact where two regex arms are approximate. Not
worth it: the approximation's failure mode is a false *negative* — a genuine
importer written unusually is not found, the check fails, and a person looks —
and the cost is three parsers to maintain against five files. Revisit if a
sixth client arrives with a syntax neither arm reads.

**Check that the fingerprint is actually on the wire, at run time.** The
strongest version of rule 2: run the adapter, capture a request, assert the
schema claim is present. The conformance runner is where that belongs, and the
caveat's own note says why it has not happened — the harness has no way to
express "this case should break". A static rule that fires on a manifest change
in a second, with no server and no toolchain, is worth having regardless, and
does not preclude the run-time version.

**Leave rule 1 as the bare stem and only ship rule 2.** Tempting once the 483
was known, since rule 2 is the half the caveat is about. Rejected: a vacuous
rule sitting in CI is worse than no rule, because it reads as coverage. That
argument is this repository's own, and applying it to a guard I had just
written is the only honest reading of it.

## Evidence

`python3 scripts/check_generated_is_used.py` on the real tree:

```
ok    5 generated declarations, all imported; 4 reach a request and the rest say why not
```

Its own suite: 11 cases, all passing. Four of them exist because a mutation
survived or a case failed against a draft:

- `a bare mention is not an import` — failed against the first draft, and is
  what exposed the 483.
- `an importer in another package does not count` — the other half of the same
  finding.
- `a package with no manifest scopes to its own directory` — written for the
  surviving `return "."` mutation.
- `a self-naming header is not an importer` — written for the surviving
  self-exclusion mutation. None of the five real generated files names itself
  today, so the exclusion looked like dead code; but
  `// Code generated from "…"; DO NOT EDIT.` is a codegen convention, and its
  quoted path matches the import shape exactly. The day `codegen.py` grows one,
  rule 1 goes vacuous for every file at once.

Three mutation runs, nineteen cases:
`ledger/mutations/20260929T054538-scripts-check-generated-is-used-py.json` (9),
`ledger/mutations/20260929T054622-scripts-check-generated-is-used-py.json` (5),
`ledger/mutations/20260929T054704-scripts-check-generated-is-used-py.json` (5).
Two survived, and both survivals were the finding: the first run's
`package_of falls back to the repository root` and the second's
`importers counts the file itself`. The third run re-ran both against the new
cases and caught them.

The 483/7/3 table was measured by running the three searches directly over the
checkout at the commit this entry lands in. The counts move as files are added
— `483` was `482` before this guard's own docstring mentioned a schema — so it
is the ratio that is the finding, not the integers.

`python3 scripts/test_check_sh.py` passes with the two new steps: 117 steps,
6 blocks and 2 env vars, all accounted for.

## What this does not do

**It does not prove the fingerprint reaches the server.** Rule 2 checks that a
named call appears in a named file. An adapter could call `Declaring` on an
empty map, or build a second client that skips it, and this would stay green.
The run-time version belongs in the conformance runner and is still not
written.

**The Python entry's rule 2 is weaker than the other two.** That client takes a
`Table` per call, so there is no single attachment to look for and the rule
checks the import statement itself — which makes rule 2 a restatement of rule 1
for that file. The comment in `REACHES` says so. Closing it properly means
asserting a query names a generated table, which is a run-time claim again.

**`examples/retention/purge.py` reaches its declaration dynamically**, by
importing the module its `--schema` argument names. That is the more
interesting path and no static rule can follow it; the check looks at `seed.py`
instead and the roster comment records the gap.

**The import shape is approximate.** Both arms are regexes over lines. A
re-export chain — `app/index.ts` importing the generated module and everything
else importing `index` — satisfies rule 1 for the right reason today and would
keep satisfying it after the middle link stopped re-exporting.

**Nothing checks the other four caveats in that entry.** `One catalog is
generated from` and `Nothing generates the catalog itself` are untouched and
still open.
