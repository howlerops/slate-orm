# The third file that stated a job count, and a pin pattern that matched anything

- **Date:** 2026-10-01
- **Author:** Claude Code (session: work down the open caveat backlog)
- **Touches:** `scripts/test_check_sh.py`, `scripts/prebuilt.py`,
  `scripts/check_toolchain_pins.py`, `scripts/test_check_toolchain_pins.py`
- **Kind:** fix

## What changed

Two caveats, both about a guard reaching less far than it reads.

**`test_check_sh.py` sweeps every tracked file for a job count.** Any
`<spelled number> jobs` in `README.md`, `CLAUDE.md`, `docs/`, `site/`,
`scripts/` or `.github/` must be either held to `ci.yml` by an anchored
pattern or listed in `NOT_THE_JOB_COUNT` with the reason it is not a claim
about today's CI. **It found one on the first run:** `scripts/prebuilt.py`
said CI "shares it across **eight** jobs" and the number is **six**. That is
now a fourth anchored count, derived from the jobs that set `SLATE_SERVERD`
or `SLATE_TESTSERVER`.

**`check_toolchain_pins.py`'s `PINNED` is a Go module path**, not any
`@v<digit>` token. `actions/checkout@v5`, `node@v22` and `pg@v16` no longer
read as pinned Go installers.

## Why

`ledger/2026-09-29-the-file-named-after-the-guard-carried-the-stale-count.md`
ends with:

> **Only two files were swept.** The question that started this was "what else
> describes the guard", and the answer was found by grepping for
> `test_check_sh` and for `seventeen`. A third file describing it in words
> that match neither pattern is exactly as stale as these two were, and
> nothing looked.

A grep finds the files somebody thought of. The sweep finds the rest, and the
caveat was right: there was a third file, it had drifted, and the two greps
could not have found it — `prebuilt.py` does not mention `test_check_sh` and
has never said `seventeen`. It says *"CI builds one per run and shares it
across eight jobs, which is most of what makes the client suites
affordable"*, a sentence whose point is that the sharing is worth it, so the
number is load-bearing for the argument and nobody re-derived it.

**Why this sweep does not cry wolf, where the old loop did.** The loop that
`CLAUDE_MD_COUNTS` replaced read every word before "jobs" and reported any it
did not recognise, so "the jobs that find the interesting failures" produced
a complaint about `'the'`. This pattern is built from `COUNTED`'s own keys:
only a spelled *number* followed by "jobs" matches at all. The same sentence
is invisible to it, which is what makes an exhaustive sweep affordable — the
old loop could not have been pointed at the whole tree without producing
noise on every page.

The `ledger/` tree is **not** swept, and that is the one real exclusion. An
entry is dated and append-only: "twenty-three of twenty-four jobs green" in
one is the score of run 534 and is supposed to stay at that number forever.
Sweeping it would turn every historical record into a complaint, which is
the failure `check_cost_prose.py` already had to solve for measurements.

The `PINNED` caveat is smaller and from the same entry family
(`ledger/2026-09-29-the-skip-list-that-excused-nothing.md`):

> **`PINNED` still matches any `@v<digit>` token** […] A file containing a
> `go install` and an unrelated version-pinned string reads as a pinned Go
> installer; nothing in the tree does today.

Nothing does today, and the cost when something does is a refusal pointing at
`actions/checkout@v5` and demanding the file decide `GOTOOLCHAIN` — a message
that teaches a reader the guard is wrong about its own subject, which is how
a guard stops being read.

## Alternatives rejected

**Exempting `prebuilt.py` rather than anchoring it.** One line in
`NOT_THE_JOB_COUNT` and the sweep goes green. Rejected because it is a live
claim about `ci.yml` — the same kind as the three already anchored — and
writing it off would make the roster mean two different things. The
exemptions are for *narration* (a past run's score) and for *the guard
quoting itself*; a current claim nobody checks is the thing this exists to
find.

**Deriving the sentence instead of checking it.** The sibling caveat in the
same entry asks for exactly that, and it is still open and still rejected for
its own reason: generating prose means prose assembled at build time in a
repository whose docs are read in an editor. The guard telling an editor
afterwards is the trade `check_cited_docs.py` and `check_cost_prose.py` both
make.

**Sweeping `ledger/` with a date cutoff** — only entries newer than N days.
Rejected because the cutoff is the parameter that goes wrong: too short and
a stale claim written last week is missed, too long and old entries start
failing as CI grows. An append-only record is not stale, it is historical,
and the distinction is categorical rather than one of age.

**Widening the sweep to `crates/` and `clients/`.** Nothing there describes
the workflow, and the sweep costs one `git ls-files`. Left out because the
two trees it would add are 90% of the repository and 0% of the risk;
widening is one tuple if that changes.

**Making `PINNED` accept Go's full module-path grammar.** Unicode, `+incompatible`,
the major-version suffix `/v2`, the `!`-escaped uppercase form. Rejected
because every one of those is a way to match *more*, and the whole point is
to match less; the pattern is deliberately narrower than Go and the direction
that would matter — dropping a real target, so the guard silently stops
watching a file — now has a test.

## Evidence

**The finding, before and after:**

```
$ python3 scripts/test_check_sh.py
FAIL  CLAUDE.md's job count is ci.yml's
      prebuilt.py says 'eight' jobs share the prebuilt binary and 6 set
      SLATE_SERVERD or SLATE_TESTSERVER
FAIL  every job count stated in prose is anchored or excused
```

The six are `go`, `python`, `typescript`, `quickstarts`, `demo` and
`batchbench`, each downloading the `build` job's artifact. Derived, not
counted by eye.

**Four mutations of the sweep and the new anchor, all caught**
(`ledger/mutations/20261001T005905-scripts-prebuilt-py.json`,
`ledger/mutations/20261001T005923-readme-md.json`, `ledger/mutations/20261001T005924-scripts-check-workspace-py.json`):

| mutation | caught by |
|---|---|
| the prebuilt count drifts to `seven` | `CLAUDE.md's job count is ci.yml's` |
| the sentence is reworded so the anchor misses it | **both** rules — the anchor says the sentence moved, the sweep says it is unanchored |
| a fourth file (`README.md`) states a count in prose | the sweep |
| the excused phrase in `check_workspace.py` changes | the sweep's roster-rot half |

The second is the one worth noting: anchoring normally trades the
cries-wolf failure for a silent one — a reworded sentence checks nothing —
and here the sweep covers exactly that gap, because an unanchored claim is
reported whether or not an anchor used to match it. The two halves are not
redundant.

**Three mutations of `PINNED`, all caught**
(`ledger/mutations/20261001T005644-scripts-check-toolchain-pins-py.json`):
dropping the domain requirement, requiring a path element again, and breaking
the version suffix.

**And the second of those is a finding, not a test.** The first version of
the pattern required at least one path element below the domain, on the
reasoning that a `go install` target names a command *inside* a module.
Mutating `+` to `*` survived every case, which sent me to Go's rules:
`go install example.com@v1.2.3` is legal when the module root is itself a
main package. The `+` was over-narrow — a pattern that silently stops
matching a real target stops watching the file it exists for — so the code
was relaxed and the case added. This is the survivor-is-a-finding rule
working in the direction it is usually not: the mutation did not find a
missing test, it found wrong code.

**Three self-references, one of which the sweep caught.** `NOT_THE_JOB_COUNT`
has to quote the phrases it excuses, so the roster matches its own pattern;
the first run reported the exemption for `check_workspace.py`'s
`twenty-two jobs` as an unanchored claim in `test_check_sh.py`. Written into
the roster rather than special-cased, which is what
`check_toolchain_pins.py` does with `NOT_AN_INSTALLER`.

**Not measured.** The sweep reads 400-odd tracked files once and the whole
of `test_check_sh.py` still finishes well inside a second; the difference
was not timed because nothing here is close to a budget.

## What this does not do

**It reads a spelled number, not a digit.** `24 jobs` passes the sweep
silently. Every count in this repository's prose is spelled, which is why
`COUNTED` exists at all, and a digit pattern would match version numbers,
line counts and timings everywhere. The honest statement is that the sweep
enforces the convention rather than the fact, and a file that writes the
count in digits is invisible to it.

**It does not read `ledger/`, and one day a ledger entry will carry a live
claim.** The exclusion is categorical and the category is wrong at the edges:
an entry's *caveat* section is about the present, and one saying "nothing
checks the nineteen jobs" would go stale exactly like `prebuilt.py` did.
`caveats.py` is the thing that re-reads those, and it reads them as prose
rather than as numbers.

**The anchored table is a list maintained by hand.** Four patterns across
three files, and a fifth file is caught by the sweep but has to be *added*
here by somebody. That is the better failure — the sweep is what makes it
loud — but the list itself has the same shape as the three this repository
already records as hand-maintained, and now there are four.

**Nothing checks that an exemption's reason is true.** `NOT_THE_JOB_COUNT`
carries a sentence per entry and nothing reads it. A narration exemption
granted to a live claim would pass, which is the same ceiling `EXEMPT_BECAUSE`
in `check_closed_caveats.py` has and the same answer: the reason is for a
person, and writing one is the cost that makes the wrong exemption
uncomfortable to grant.

**`PINNED` still has one false-positive shape.** A container reference like
`registry.example.com/img@v1.2` has a dotted first element and a path, so it
matches. Nothing in the tree writes one — docker digests are
`@sha256:…`, which has no digit after the `@` — and narrowing further would
need to know what a Go module path is *not*, which is not a thing a regex
can be told.
