# Two of nineteen verdicts cited a question that had already been answered

## What changed

Nineteen more `open` caveats decided: 18 `deliberate`, 1 `closed`, and of
those 18 one was rewritten as `narrowed` before the commit. Open falls 56 → 37.

The closure: **the write paths are timed.** The caveat is from the entry that
made the workbench's read timing honest, and it said `INSERT`, `UPDATE` and
`DELETE` report no milliseconds at all. They do. `Playground::wrote` sets
`kernel_ms` on each, `patch` adds the read half of an `UPDATE ... SET` to the
write half deliberately, `site/workbench.js:773` renders it per statement and
`summarise` folds it into the status bar. Four tests hold the number to being a
measurement rather than a constant.

The two corrections are the reason this entry exists:

- The reason first written for **"it does not reach a million rows"** said the
  loader cliff was what stopped it and was still unattributed — quoting the
  caveat. The day after that caveat was written,
  `2026-09-23-the-cliff-does-not-reproduce-and-the-knob-does-place-it.md`
  loaded a million rows in 22.0 / 23.2 / 22.5 s with identical PUT counts. There
  is no cliff. The verdict is `narrowed`, and the residual is only that the
  scan-cost-per-row measurement has not been re-run since.
- The reason first written for **"it does not settle `SCAN_ROW_COST`"** said
  settling it meant resolving the 3.5× cold-full-scan disagreement.
  `2026-09-21-a-planner-constant-that-said-the-question-was-open.md` resolved
  that: neither example was wrong, they scan stores in different cache states,
  and a partially populated block cache fragments a scan into many small ranged
  reads. What is actually left is three explained values spanning 7×, and
  choosing one is a decision about which cache state a server should assume.
  That is a better reason and a genuinely undecidable-by-measurement one.

## Why

Both wrong reasons were caught by `check_caveat_citations.py`, and not for the
reason it exists. It refused the commit because both cited a ledger file that
does not exist — I had written plausible names from memory
(`2026-09-22-the-loader-cliff-tracks-bytes-not-rows.md`,
`2026-09-21-two-examples-disagree-about-a-cold-full-scan.md`), the eighth and
ninth invented citations this repository has caught. Looking up the real names
is what made me read the real entries, and the real entries said the premises
were gone.

That is worth recording because it is the *second* order effect of a guard that
checks existence rather than aptness. The caveat
`2026-09-26-the-citation-nobody-could-follow.md` carries — "it checks
existence, not aptness" — is true and was decided `deliberate` on the argument
that aptness needs prose parsing. What it misses is that forcing a writer to
produce a name that resolves makes them open the file, and opening the file is
what catches the stale premise. A guard can be worth more than its rule.

## Alternatives rejected

**Trust the caveat's own account of why it is blocked.** Both wrong reasons
were faithful paraphrases of the caveat text. It is the cheapest way to write a
`deliberate` verdict and it is wrong in exactly the direction that matters: a
caveat is a snapshot of what was known when it was written, and the ones worth
deciding are disproportionately the ones where something has moved since. Two
of nineteen here, three of a hundred and three in the previous sweep, two of
twenty-nine in the one before: the rate is stable at a few per cent and does not
fall as the easy rows are cleared.

**Decide `open` for anything whose premise needs checking.** This is the
conservative reading and it is what produced a 186-row backlog nobody opened. A
verdict whose reason cites the entry that moved the premise is checkable by the
next reader in one `git show`; a row sitting under `open` with no reason is not
checkable at all, and costs a re-reading every sweep.

**Close `SCAN_ROW_COST` rather than decide it.** Tempting, because the question
the caveat names *is* answered. Rejected because the caveat is about the
constant, not the disagreement: three explained values spanning 7× is still
three values, and a `closed` verdict would tell a future reader the constant had
been settled. `deliberate` with the workload argument is the true statement.

**Argue "nobody got round to it" as a deliberate reason.** Rejected for all
nineteen. Where the only honest reason was that the work has not been done, the
row stays `open` — which is why 37 remain rather than 0. The schema requires a
`deliberate` verdict to name the alternative and its cost, and a verdict that
cannot is the tracker lying in the expensive direction.

## Evidence

- `cargo test -p slate-wasm --test timing --no-fail-fast`, 2026-09-27:
  `6 passed; 0 failed`, including
  `a_write_reports_what_the_index_maintenance_cost`,
  `an_update_charges_for_the_read_it_has_to_do` and
  `a_buffer_reports_a_time_for_every_statement`.
- `python3 scripts/check_caveat_citations.py` refused the first draft, naming
  both invented paths and nothing else. After the corrections: `every cited
  path resolves`, 259 paths and 5 names across 979 verdicts.
- `python3 scripts/caveats.py` — `37 open, 28 narrowed, 227 closed, 611
  deliberate, 0 untriaged`. Before: `56 open, 27 narrowed, 226 closed, 592
  deliberate`.
- `python3 scripts/check_closed_caveats.py` — 9 of 9, 210 witnesses in the tree.
- `sh scripts/check.sh` — 71 of 71.

## What this does not do

**It does not re-read the seventeen reasons that were not corrected.** Two of
nineteen premises had moved; the other seventeen were checked only as far as
writing the reason required, which for most meant reading the caveat's
paragraph and one file it names. A premise that moved in a file the reason does
not cite would not have been noticed, and at the observed rate there is
probably not one here — but "probably not one" is an estimate from a sample of
nineteen, not a check.

**The `narrowed` residual is the measurement, and nothing schedules it.** The
scan-cost-per-row run stopping short of a million rows is now known to be
unblocked rather than blocked, which makes it ordinary undone work. It goes on
the worklist with a `checked` date that will expire, which is the only
mechanism there is.

**Thirty-seven rows remain open and every one of them is work.** What is left
after four sweeps has no argument to make: the committed mutation specs nothing
re-runs, `ErrorInfo.metadata` unsurfaced, `purge_deleted`'s two-grant path,
the unbounded array element check, the relationship fan-out with no ceiling, a
browser check over the docs pages, and the client-facing docs. Deciding them
would mean writing reasons of the kind this entry rejects.
