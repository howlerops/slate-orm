# Two "nothing checks this" caveats that a guard had already answered

## What changed

Three open caveats are now `closed`, with no code written for any of them. Each
said a property was a claim from reading; two had been made checkable by
`scripts/check_handlers.py` in the months since and neither entry was revisited
when that happened, and the third was an audit nobody had done.

- `2026-09-21-a-view-reported-as-a-typo.md` — *"Nothing asserts that
  `Head::table` stays the only producer of this refusal."* Rule 7 asserts it:
  every function building a `"no table named"` refusal must be in
  `NAMES_A_MISSING_TABLE`, and its never-fires half fires when the count reaches
  zero.
- `2026-09-20-the-caveat-was-already-true.md` — *"The by-caller list is a claim
  about callers that nothing checks … a caller that only converts, never
  fingerprinting, would satisfy nothing here."* Rules 1 and 3 between them cover
  that shape.
- `2026-09-20-the-caveat-was-already-true.md` — *"I did not re-audit the rest of
  `convert.rs`."* Now audited: two `&Catalog`-taking functions, both already
  rule 3's; two `fingerprint::` calls, one rule 2's and one a response builder.

`docs/caveat-status.json` carries all three verdicts with the demonstration in
the `by`, and `scripts/check_closed_caveats.py` gains the three witnesses
`sole-missing-table`, `converter-callers` and `convert-audited`.

## Why

A caveat is only worth keeping if it is still true, and the tracker's whole
argument is that a settled one costs nothing to leave open while an unsettled
one costs a reader's trust. These two were the opposite failure: work that
answered them landed under a different task, and the entries went on saying
nobody had checked. That is the stale-documentation rule applied to the ledger
itself.

Closing them needed a demonstration rather than a reading, because a reading is
what put them here. Both were closed against the *real* tree — the injected
fourth producer went into `crates/slate-server/src/convert.rs` and the removed
authorisation out of `crates/slate-server/src/service.rs` — rather than against
the guard's synthetic fixtures, which is a different claim. The fixtures say the
rule works; the real tree says the rule is pointed at the code the caveat is
about.

## Alternatives rejected

**Close them on a reading of the guard's source.** Cheapest, and exactly the
mistake being corrected. `FINGERPRINT_BY_CALLER`'s own reason records that the
first version of it "said so when four of the six did not" — the claim was made
from reading, was wrong, and a caller with no grant read a table's column count
off `join` until somebody checked. Reading the rule and concluding it covers the
case is the same move one level up.

**Add a Rust test asserting there is exactly one producer.** A
`#[test]` grepping `convert.rs` and `service.rs` for `"no table named` would be
a second implementation of rule 7, in a language with no access to the roster's
reasons, drifting from it the first time a file moved. It would also be scoped
to whatever crates its `include_str!`s named, which is the mistake
`check_guard_scope.py` exists to catch — the guard already walks the workspace
members and refuses a crate outside `SOURCES`.

**Mark them `narrowed`, with "a `&TableDef` passed along is still invisible" as
the residual.** Tempting and wrong: that residual is not these caveats. It is
already written down, in `check_handlers.py`'s own docstring — *"A handler can
hold a `&TableDef` from one of the accounted sites and pass it along, and this
will not see it."* Re-recording somebody else's caveat as the residual of yours
is how one gap comes to be tracked in three places and fixed in none.

**Leave them open until the pass-along hole is closed too.** That makes the
verdict a statement about a different property than the claim, which is the
`narrowed` misuse the tracker's README argues against. The claim as written is
answered; say so, and let the pass-along hole be tracked where it was recorded.

## Evidence

Rule 7, against the real tree. A fourth producer injected into `refuse_unused`
in `crates/slate-server/src/convert.rs`:

```
exit 1
convert.rs:1974: `refuse_unused` builds a "no table named" refusal.
  A declared view reaching this answers as though the operator mistyped a name
  they wrote themselves. Route it through `Head::no_such_table`, which consults
  the view registry, or add `refuse_unused` to NAMES_A_MISSING_TABLE with the
  reason a view's name cannot reach it.
1 problem(s)
```

Rule 1, against a seventh caller of the by-caller list that only converts:

```
service.rs:532: `preview` resolves a table with the bare `self.table(..)`,
  which does not authorise.
```

Rule 3, with `authorize_join_inputs` deleted from above `join_from_proto` —
the half the caveat says was read rather than checked:

```
service.rs:2654: `join` calls `join_from_proto` with no authorisation in the
  4 lines above it.
```

Each injection restored the file in a `finally` and the guard reported clean
afterwards: `30 files, 2 bare resolutions all accounted for, … 3 missing-table
refusals all rostered, 3 lookups by name all rostered`.

**Three mutations, over two runs, and one of the three was scored wrong.**

| mutation | caught by |
| --- | --- |
| rule 7's roster check made vacuous | `a function building a missing-table refusal itself is reported` |
| rule 7's pattern blinded | `a handler that authorises before fingerprinting passes`, and three more |
| a fourth producer in `refuse_unused` | **scored SURVIVED, and the score was false** |

`ledger/mutations/20260927T033116-scripts-check-handlers-py.json` holds the
first two, both caught.
`ledger/mutations/20260927T032711-crates-slate-server-src-convert-rs.json` holds
the third, and its verdict is wrong. `check_handlers.py` is a guard, not a test
suite: it prints `N problem(s)` and no dialect reads that, so the run wrapped it
as `sh -c "python3 scripts/check_handlers.py && echo '1 passed, 0 failed' ||
echo '0 passed, 1 failed'"`. The guard did fail, the `||` arm did fire, and the
`python` dialect read the resulting `0 passed, 1 failed` as a suite that
reported — but its *failure* pattern is `^FAIL\s+(.+?)$`, and the wrapper never
emitted one. One suite reporting, no named failure, is a survivor by definition,
so a working guard was scored as unprotected code.

That is the sixth lie in `mutate.py`'s docstring seen from the other side — a
failure pattern missing a real failure — and the docstring's prediction held
exactly: it "eventually shows up as a survivor, which exits non-zero and demands
an explanation". It did, and the explanation is that the harness was wrong. The
record is committed rather than deleted, because a run that happened is part of
the audit trail #287 exists to keep; this paragraph is what stops it being read
as a finding.

Tracker: 1015 caveats, 31 open before and 29 after; 239 closed, 222 of them
witnessed in the tree and 17 exempt.
`python3 scripts/check_caveat_citations.py` — *every path, test and function
name a caveat verdict cites is there*.

## What this does not do

**No new protection was written, so the caveats close at the strength the
guard already had.** Rule 7 is a roster over a string pattern: a fourth producer
that spells the refusal differently — `Status::not_found(format!("table {name}
is unknown"))` — is invisible to it, and the roster's never-fires half only
notices when *every* producer disappears. The alternative, matching on
`Status::not_found` and demanding each call justify itself, was rejected when the
rule was written for the reason it stays rejected: `not_found` has honest uses
this would flood.

**The pass-along hole is untouched and now tracked in one place rather than
three.** A handler holding a `&TableDef` obtained from an accounted site and
passing it to a new consumer satisfies every rule here. It is recorded in
`check_handlers.py`'s docstring, which is where it was found; closing it means
type-level provenance — an `Authorized<TableDef>` wrapper the resolver alone can
mint — which is a real design, a wide refactor, and not this.

**The demonstrations are hand injections, not `mutate.py` runs**, because no
dialect reads a guard's `N problem(s)` and the attempt to adapt one is the
false survivor above. Each closes the three hand-rolling failure modes
`CLAUDE.md` names — the anchor is asserted to occur exactly once, nothing is
passed through a shell, and the restore is in a `finally` — but not the fourth:
a killed run leaves no in-flight marker, so an interrupted injection would leave
the tree mutated with nothing to say so. Teaching `mutate.py` a `guard` dialect
reading `^\d+ problem\(s\)$` would fix that for every guard in `scripts/` at
once, and is the obvious next thing; it is not in this change because the
change is a triage pass, and a new dialect needs its own corpus entry in
`scripts/test_mutate.py` against all five existing ones.

**The third caveat closed here rests on one reading of one file.**
`2026-09-20-the-caveat-was-already-true.md`'s *"I did not re-audit the rest of
`convert.rs`"* is now closed because I read it: two `&Catalog`-taking functions,
both rule 3's; two `fingerprint::` calls, one rule 2's and one a response
builder holding an already-resolved `&TableDef`. That is a reading, and a
reading goes stale — which is precisely the objection this entry opens with. It
is defensible only because the *structural* answer stands behind it: rules 1, 6
and 8 roster every way a name becomes a `TableDef`, so a new schema-dependent
site in that file has to come by one of them. If those rules were removed the
reading would be worth nothing, and nothing ties the two together except this
paragraph.

**I did not extend the reading to the other 29 files the walk covers.** The
caveat's second sentence generalised the question to all of them; I answered it
with the structural argument above rather than 29 more readings, which is a
weaker thing than what it replaces. A file doing something schema-dependent with
a `&TableDef` handed to it — the pass-along hole again — would pass all three
rules and this reasoning.
