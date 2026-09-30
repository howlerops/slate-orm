# Three more guards rostered, and `.bar` satisfied by the word "toolbar"

- **Date:** 2026-09-29
- **Author:** an agent session, continuing from
  `ledger/2026-09-29-the-mutations-run-again-now.md`
- **Touches:** `scripts/check_site_css.py`, `scripts/test_check_site_css.py`,
  `scripts/mutations.json`, `docs/caveat-status.json`
- **Kind:** fix

## What changed

Three of the eleven guards with no real-tree mutation got one:
`check_table_provenance`, `check_generated_is_used` and `check_site_css`. The
first two caught theirs. The third did not, and the reason is the fourth
instance this week of the same class:

**`check_site_css.py` asked whether a class name appeared anywhere in the
concatenated sources, as a substring.** So `.bar` was satisfied by the word
"toolbar", and `.shell` stayed alive after its one `class="shell"` was gone
because `api.ts` explains an environment variable by saying "what a shell
produces". A use is now a whole word with comments stripped.

## Why

The roster from the previous entry turned "eleven guards have no mutation"
into a checked list with a sentence each, and seven of those sentences said
some version of "not this session". Three of the seven were a few minutes'
work, and one of the three found a defect — which is the argument for
finishing the other eight rather than leaving the sentences standing.

The defect matters in the direction that is easy to miss. `check_site_css.py`
exists to find dead CSS, and dead CSS is exactly what a substring match hides:
a class named for an English word — `shell`, `row`, `group`, `bar`, `panel`,
`note` — can be deleted from every template in the tree and stay "used" by a
comment, a variable name, or a longer word. Of the ninety-one classes across
the two stylesheets, a large fraction are ordinary words.

## Alternatives rejected

**Match only inside a `class=` attribute.** Tighter, and the existing test case
"a class only ever built dynamically still counts as used" is there precisely
to stop someone doing it: `site/workbench.js` builds
`` `<span class="badge" data-tone="${tone}">` `` and a stricter attribute rule
would have to understand template literals. The case predates this change and
says so; whole-word-outside-comments keeps it passing.

**Leave the substring match and accept the looseness.** It is a check whose
whole subject is a name appearing somewhere, so "somewhere" is arguably the
rule. Rejected on the measurement: with the tighter rule, **0 of 91 classes
change verdict**, so the looseness buys nothing today and hides an unknown
amount tomorrow.

**Write a real parser for the three source languages.** TypeScript, TSX and
HTML, to find attribute values properly. Out of proportion to a guard whose
job is to notice a stylesheet rotting, and the dynamic case above means a
parser would still need an escape hatch.

## Evidence

**The finding.** With the mutation `class="shell"` → `class="husk"` applied to
`examples/explorer/web/src/index.tsx`, the guard passed:
`ledger/mutations/20260929T085402-examples-explorer-web-src-index-tsx.json`
records the survivor. `grep -rn shell examples/explorer/web/src/` shows why —
three hits, one the definition, one the attribute, and one
`api.ts:31: * what a shell produces from an unset variable it expanded anyway`.
After the fix the same mutation is caught:
`ledger/mutations/20260929T085550-examples-explorer-web-src-index-tsx.json`.

**The measurement, before changing anything.** 61 classes in `site/style.css`
and 30 in `examples/explorer/web/src/styles.css`. Dead by substring: **0**.
Dead by whole word outside comments: **0**. So this is a strengthening with no
current instances, the same shape as `check_cited_files.py` this morning, and
the guard's summary is unchanged: `all of them reachable from 13 sources` /
`from 6 sources`.

**A trap met while fixing it, worth writing down.** `A_COMMENT` needs `DOTALL`
for `/* … */` and `<!-- … -->`. Under `DOTALL`, the line-comment alternative
`^[ \t]*(?://|\*).*$` is greedy across newlines and runs from the first `//` to
the end of the file. Forty-three live classes read as dead on the first run.
`[^\n]*` fixes it, and the case that pins it — a line comment *above* a use —
had to be written specially, because the fixture's existing comment case puts
its comment at the end of the file where eating to EOF costs nothing.

**Mutations.**
`ledger/mutations/20260929T085838-scripts-check-site-css-py.json`, 3 cases, no
survivors: the substring match, the comment stripping, and the greedy line
comment. Each was a survivor first and bought a case —
`a class whose only occurrence is inside a longer word is reported`,
`a class named only inside a source comment is reported`, and
`a line comment does not hide the uses below it`. Its tests went 12 → 15.

The other two suites, both clean first time:
`ledger/mutations/20260929T085401-docs-performance-md.json`
(`check_table_provenance`: a measurement table loses its build line) and
`ledger/mutations/20260929T085402-examples-explorer-backends-python-adapter-main-py.json`
(`check_generated_is_used`: an adapter stops importing its generated schema).

`python3 scripts/check_mutations_roster.py` reports `ok    29 guards, 20 with a
real-tree mutation, 8 with a written reason for having none`.
`sh scripts/check.sh` reports `85 passed, all of them`.

## What this does not do

**Eight guards still have no mutation.** `check_build_output` has a technical
reason — its subject is the absence of a tracked artifact, which `mutate.py`'s
replace-text-in-one-file patch cannot express. The other seven are still
"deferred", and this entry is the evidence that deferring them has a cost: one
of the three picked up today was broken.

**The whole-word rule still accepts a name in any string.** A class called
`error` is satisfied by `throw new Error("error")`. Narrower than a substring
and wider than an attribute, and the dynamic-class case is why it stops there.

**Comment stripping is three regexes, not a lexer.** A `//` inside a string
literal — a URL, a regex — is treated as a comment and the rest of that line
is dropped. Nothing in the two source sets has one before a class use, checked
by the run coming out unchanged; a future one would read as dead CSS, which
fails loudly rather than quietly.

**The measurement was taken once.** 0 of 91 either way is today's tree. Nothing
re-derives it, so the sentence "this strengthening finds nothing" will go stale
the first time it stops being true — and the guard will say so, which is the
point, but the entry will not.
