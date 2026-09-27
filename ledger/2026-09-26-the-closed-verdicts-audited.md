# The last unread class: 195 `closed` verdicts read, one wrong, four supporting details wrong — and every verdict in the tracker has now been looked at

- **Date:** 2026-09-26
- **Author:** Claude, working from the standing instruction to address every caveat
- **Kind:** process
- **Touches:** `docs/caveat-status.json`, `ledger/`

## What changed

`ledger/2026-09-26-the-reverse-sweep-found-six.md` ended by naming the 195
`closed` verdicts as "the largest unread surface in the tracker by a wide
margin" and "the only class where a wrong verdict is invisible in every
listing". This reads them.

One verdict moved. **`2026-09-24-the-caveat-tracker-counted-emphasis-as-caveats.md`'s
"The 762 is still not audited" is `narrowed`, not `closed.`** Reading all 762
and giving each a verdict audits the extraction pattern's **precision** — none
of the 762 turned out to be something other than a caveat, and the single
non-caveat that was found is why `WITHDRAWN` exists. It says nothing about its
**recall**: a caveat written under a different heading, inside a list, or with
an italic rather than a bold lead is still invisible, and reading what the
pattern found cannot find what it missed. The verdict I wrote answered the
easier of the two questions the caveat asks.

Four supporting details were wrong under correct verdicts, and are corrected:

* A `by` cited `scripts/check_cost_prose.py:401` as where the `docs/` widening
  is. Line 401 is a whitespace-collapsing helper. The widening is
  `DOCS = ROOT / "docs"` and the module docstring; the closure holds, the line
  number never did.
* A `by` said "site/ holds only the workbench now". `site/` holds
  `index.html` **and** `workbench.html`. What went is the two-dropdown panel,
  which is what the caveat was about.
* A `by` said `ledger/mutations/` "holds 80 records". It holds 88 today and
  will hold more tomorrow. A count in a status file is not a dated record the
  way a ledger entry is, so it now says "one record per run".
* `2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md`'s caveat
  **"`sql.rs:34` may be stale, and I still have not confirmed it"** carried a
  live risk in its own text: *"I marked the caveats that say 'one group key per
  join' as closed on the strength of the kernel signature… If the comment is
  right and the parser still refuses, those verdicts are wrong."* Confirmed by
  reading rather than inferring: the grammar block now reads
  `[ GROUP BY <item> (, <item>)* ]` with no join-specific limit, and
  `crates/slate-sql/tests/front_end.rs:185`
  `a_join_groups_by_more_than_one_key` fails with *"two group keys on a join
  were refused"* if the parser ever does refuse. The `by` now names that test
  instead of the entry that fixed the comment.

## Why

Because `closed` is the verdict that *removes* a caveat from every listing, so
a wrong one is not pessimism, it is a disappearance. The entry that wrote 167
of them said so:

> **167 `closed` verdicts rest on evidence of very uneven strength.** Where a
> feature either exists in the tree or does not, I checked it… Where the caveat
> was about a test existing, or a guard covering a case, I matched it against
> the completed task whose title names the same gap, which is good evidence and
> not proof.

The reading confirms that split and puts numbers on it. **188 of 195 cite an
entry or a completed task by title. Seven make a claim a grep can settle.** The
188 are the weaker kind and they are weak in a specific, bounded way: each names
work that the task list records as completed, and the failure mode is not that
the work never happened but that it landed narrower than the caveat's sentence.
That is the failure mode the one `narrowed` above is.

The seven greppable ones were all checked against the tree, and that is where
three of the four wrong details were. **The more specific a citation, the more
likely it is to be wrong** — which is the opposite of reassuring and is exactly
what the invented-citation entry earlier today found: a precise-looking
reference is one nobody re-derives.

## Alternatives rejected

**Verify all 188 by reading the cited entry.** It is what the caveat's residual
asks for and it is a day's work at the reading pass's measured rate. The reason
not to do it in this commit is that it is a *different* job: checking a
`closed` means going to the tree and confirming the thing exists, not reading
prose, and mixing it with the prose pass would produce one commit where two
kinds of evidence could not be told apart — the same argument the reverse sweep
made for not folding this in. What is done instead is the mechanical layer in
full and the specific layer in full, which is the part that could be finished
honestly today.

**Trust the task list.** Every one of the 188 names a task the list marks
completed, so a script could join them and report green. It would be a check
that reads one of my own records against another of my own records; a completed
task means the work was done, not that it closed the caveat's sentence. The one
verdict that moved today cites a completed task and is still wrong about scope.

**Leave the four details alone because the verdicts are right.** A wrong line
number under a right verdict is the cheapest kind of wrong and the most
corrosive: the next reader who follows `check_cost_prose.py:401`, finds a
whitespace helper, and concludes the closure is fabricated has learned to
distrust a file that is 99% correct. `CLAUDE.md`'s "stale documentation is
worse than none" applies to a `by` field exactly as it applies to a README.

## Evidence

**Mechanical layer, whole population.** `scripts/check_caveat_citations.py`
(built earlier today) reports 123 path citations across 883 verdicts, all
resolving. A second, one-off pass extracted every identifier-shaped token in a
`closed` verdict's `by` — `Type::member`, `snake_case_name`, `fn_name()` — and
grepped the tree for each: **49 tokens, 0 not found.**

**Specific layer, the seven greppable claims, each checked:**

| claim | verified |
|---|---|
| Go and TS tests `TestOrderingAChainsGroupsByItsComputedValue` / "ordering a chain's groups by its computed value" | both present, one each |
| `crates/slate-serverd/src/main.rs:370` calls the migration verifier | `verify_of` is called there |
| the wasm `JoinSpec` names its side with `ComputeSpec::input` / `AggregateSpec::input` | three occurrences |
| `Chain::compute` exists | `pub compute: Vec<Scalar>` at `chain.rs:340`, builder at `:425` |
| `panels.tsx:331` offers author, country, decade | exactly those three `<option>`s |
| `site/check/workbench.py` has "the aliased chain plans as three steps" | present — but "site/ holds only the workbench" is wrong |
| `check_cost_prose.py` was widened to `docs/` | `DOCS = ROOT / "docs"`, docstring records it — but `:401` is wrong |

**Spot checks on the strongest falsifiable "closed by task N" claims**, because
"the task is completed" is not evidence that the tree changed:
`examples/explorer/head.toml` has one `[[views]]` section and an array column
(`tags`, `sizes`, with a comment saying it exists so the generated array
decoder has something to execute); `scripts/check_handlers.py`'s `SOURCES` is
two directories rather than the one file the caveat complained of, and its own
comment records that reading `service.rs` alone missed a `fingerprint::check`
in `convert.rs`; `scripts/check.sh` runs all four of `ruff`/`ty` × client/root.

**Result: 1 wrong verdict in 195, 0.5%.** Against 6 in 322 (1.9%) for the
`deliberate` class this morning. I expected `closed` to be the *worse* of the
two, on the entry's own "evidence of very uneven strength" and on the argument
that a wrong `closed` hides itself. It is three times better. The honest
reading is not that `closed` verdicts are more careful — it is that closing a
caveat requires naming something that exists, and naming a thing is a step that
fails loudly, where deciding a caveat is settled requires naming nothing.

**Every verdict class has now been read.** `open` and `narrowed` on 2026-09-26
(`the-backlog-is-read.md`), `deliberate` this morning
(`the-reverse-sweep-found-six.md`), `closed` here, and the 65 `moment` verdicts
read in the same pass as this one — all 65 are statements about one run, one
commit or one moment, and none is misfiled. Two were worth a second look and
both held: "Nothing checks the CI job actually runs" is explicitly bounded by
"the first push will say", and "`ValueType::ALL` is new public API" answers its
own question in its second clause.

**No mutation run.** Nothing executable changed; the diff is six `by` strings
and one verdict.

`python3 scripts/caveats.py`: no problem, no orphan. `--unread 30`: 0.

## What this does not do

**188 `closed` verdicts still rest on a task title.** The reading confirmed
each names work the task list records as done and did not go to the tree for
each one. That is the caveat's residual, it is unchanged, and this entry has
now said so twice in two sections, which is the honest shape of a job that was
bounded rather than finished.

**Nothing re-reads a `closed` verdict when the thing that closed it is
reverted.** `--unread` filters `closed` out before it looks, by design: a
settled verdict is not a worklist item. It is also how a regressed feature
stays `closed` for ever. The caveat that says this
(`what-is-left-to-do-needs-a-list-not-a-count.md`'s "Nothing re-triages") is
itself `closed`, against the `checked`-date mechanism, which covers only the
open list — which makes it the same shape as the one this entry narrowed, and I
am not confident enough about that to move it on a second reading of my own
first reading.

**The four corrected details were found because they were checkable.** Six
`by` strings in this file make a claim about *prose* — what an entry argues,
what a message says — and no pass has followed one. A wrong detail there looks
exactly like the four that were found and nothing distinguishes it.

**One verdict moved and it was mine, found by me, on the same day I wrote the
others.** Three passes over three classes, all by the same reader, all finding
a low single-digit error rate. That number is a floor and it measures my
consistency at least as much as the verdicts' correctness, which
`the-reverse-sweep-found-six.md` already recorded and this repeats because it
applies to every figure in all three entries.
