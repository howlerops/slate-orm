# The same wrong sentence, twice in one file

## What changed

Two things, both left open by
`ledger/2026-09-19-the-regex-hole-that-was-not-there.md`.

**The error message that caused that entry now names the spelling that
exists.** `kind matches '^a'` answered ``expected a comparison after the
column, found `matches` `` and now adds *"A regular expression is spelt `~`
here, as in Postgres — `~*` ignores case, and `!~` and `!~*` negate."* Six word
spellings earn that hint; any other word gets a different one naming the
word-spelled tests (`LIKE`, `ILIKE`, `IN`, `IS NULL`); punctuation gets none.

**The audit that entry asked for found one error**, in the same file as the
claim it corrected. `docs/validation.md`'s "What this note does not settle"
still opened by calling the regex question moot *because a check could not hold
one* — the withdrawn claim, standing ten days after its own correction, four
sections below it. Fixed, and the wording added to
`scripts/retired_claims.json` so a third copy fails a check.

## Why

The first half is the cheaper half and the entry already argued for it: the
message was "accurate and unhelpful", and a session spent a detour and
published a design note with a wrong headline finding because it read "found
`matches`" as "there is no regular expression here". The message named what it
found and not what it wanted. Naming the one spelling is not the `matches`
alias that entry rejected — it stays rejected — it is the difference between an
error a reader can act on and one they can only misread.

The second half is why the entry left the caveat: *"the three remaining
findings were each checked against the code when written — and so was the regex
one, and it was wrong, so 'checked when written' is a weaker guarantee here
than it sounded."* That is the right worry, and looking found something, though
not where the entry expected.

The three findings themselves hold up, and all three have since been built
(`V1`–`V3`); each is marked **Built** in the note. The two architectural
refusals that could be checked mechanically were:

- *A validation cannot depend on the caller.* True: `constant_predicate` in
  `crates/slate-serverd/src/schema.rs` parses every check against
  `TableScope::constant`, which refuses `:principal`, and then walks the tree
  again to assert the two agree.
- *A validation cannot query.* True structurally: `Expr` evaluates against one
  `Row` and has no node that reads.

What was wrong was not a finding but a **consequence**. A correction that
reaches the claim and not what the claim was used for is the two-copies failure
`retired_claims.json` exists for — and this is the first instance of it with
both copies inside one file, which is exactly the case a reader trusts most and
a `git grep` across files would never have found.

One nuance the fix records rather than smooths over: that paragraph's advice
was to "publish only the predicates whose client-side evaluation is provably
identical … and keep patterns server-side". The build did not do that and was
right not to — `--print-schema` publishes the source text of *every* check, a
regex one included. Publication and evaluation came apart. The paragraph now
says which line is the one to hold: nothing off the server decides whether a
row passes.

## Alternatives rejected

**Accept `matches` as an alias after all.** The obvious fix, and the one the
failing spelling asks for. Still rejected, for the reason the original entry
gave: two spellings of one operator is a grammar with a synonym in it, and
every future reader has to learn both. A hint costs a reader one line once; an
alias costs every reader forever. Nothing here revisits that argument — the
hint is what the entry itself proposed instead.

**Suggest `~` for every unrecognised word.** Simpler than a roster, and wrong
most of the time: `kind between 1 and 2` is not a regex mistake, and being told
about `~` would send the reader further from the answer. Hence two arms and a
test that asserts they differ — a test that only checked "some hint appeared"
could not tell a roster from a constant.

**Levenshtein distance against the operator names.** The general version, and
it does not fit the shape of the mistake. The guesses are not misspellings of
`~`; they are the *right word from another dialect*. No edit distance connects
`regexp` to `~`. A six-entry roster of what people actually reach for is
smaller, exact, and honest about being a list of guesses.

**Quietly fix the stale paragraph.** The convention here is the opposite, and
the entry being audited says so in its own Alternatives section. It is also
self-defeating: a correction that leaves no trace is how this one got made
twice.

**A guard for "a correction reached every copy *within* a file".**
`check_retired_claims.py` already reads every file and would have caught this
the moment the phrase was on its list. The gap was not the guard's reach but
that nobody added the phrase — the original correction predates the list. So
the fix is the entry, not a new rule.

## Evidence

Three tests in `crates/slate-serverd/src/lang/pred.rs`:

- `a_word_where_a_comparison_belongs_names_the_spelling_that_exists` — the
  exact failing input from the audited entry, plus the shouted spelling.
- `a_word_that_is_not_a_regex_guess_names_the_word_operators` — asserts the
  generic arm is a *different* answer, and that `~` is absent from it.
- `punctuation_where_a_comparison_belongs_gets_no_hint`.

`cargo test -p slate-serverd --bin slate-serverd lang::pred`: 28 → 31.

Five mutations,
`ledger/mutations/20260929T055707-crates-slate-serverd-src-lang-pred-rs.json`,
all caught on the first attempt:

| mutation | caught by |
| --- | --- |
| the roster is empty | `…names_the_spelling_that_exists` |
| the roster is matched case-sensitively | the same |
| punctuation gets a hint too | `punctuation_…gets_no_hint` |
| the generic arm suggests `~` as well | `…names_the_word_operators` |
| no hint is appended at all | both word cases |

For the audit half, the demonstration is the guard: adding the phrase to
`retired_claims.json` made `python3 scripts/check_retired_claims.py` print
`docs/validation.md:274 states a retired claim`, and it prints
`ok    7 retired claims, none restated in 656 files` after the fix. It also
fired on the first draft of the *replacement* paragraph, which quoted the
retired wording verbatim while explaining it — correct behaviour, and the
paragraph was reworded rather than exempted.

One thing the two guards found about each other: a closed caveat's witness
needle cannot *be* a retired phrase. The obvious witness for the audit half was
the phrase itself in `retired_claims.json`, and `check_retired_claims.py`
promptly refused `check_closed_caveats.py` for stating it — the guard that
forbids restating a claim against the guard that has to grep for one. The
needle is the *replacement* wording instead, which lives in the same file and
is not forbidden. Worth knowing before somebody writes the obvious line again.

`cargo test -p slate-serverd --no-fail-fast`: 312 tests over ten binaries, all
passing. `cargo clippy --workspace --all-targets` is clean here, which as ever
is necessary and not sufficient: CI's clippy is newer.

## What this does not do

**The roster is a list of guesses, not a measurement.** One of the six —
`matches` — is attested, by the entry this closes. The other five (`match`,
`regex`, `regexp`, `rlike`, `similar`) are the spellings other dialects use,
chosen by reading rather than by observing anyone type them. A seventh will
arrive and get the generic hint.

**No other error message in the parser was reviewed for the same weakness.**
This one was fixed because an entry recorded what it cost. `expected a column
name, found …` and the operand-type refusals may have the same shape and were
not examined.

**The audit covered `docs/validation.md` and nothing else.** The entry's caveat
named "the rest of the note", which is what was read. The note's claims about
*other* documents — that the fingerprint excludes checks, that the gap table
lists validations — were spot-checked against the code and not traced to their
own copies.

**It does not add a guard that a correction reaches the rest of its own file.**
The existing list-based guard does this once a phrase is on the list, and
nothing makes adding it to the list part of withdrawing a claim. That is the
next version of this failure and it is not closed.
