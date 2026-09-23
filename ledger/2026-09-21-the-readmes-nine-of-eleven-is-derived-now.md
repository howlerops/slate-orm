# The demo README's "9 of 11 books" is computed from the seed and the policy, not asserted — and writing the check found two survivors in the check itself.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #273 (F5s)
- **Touches:** `scripts/{check_demo_surface.py,test_check_demo_surface.py}`
- **Kind:** guard

## What changed

`scripts/check_demo_surface.py` grows a second half. It already checked that
every endpoint the demo's adapters serve is either called by the UI or rostered
in `NOT_IN_THE_UI` with a reason; it now also recomputes the two numbers in
`examples/explorer/README.md` — *"`reader` sees 9 of 11 books — a row policy
hides everything published before 1960"* — from the two files that decide them:
`backends/go/seed.go`, which holds the books and their years, and `head.toml`,
which holds the policy.

`readme_counts()` takes its three paths as arguments, the way `routes()` and
`called()` already did, and `main()` takes the check itself as one.
`test_check_demo_surface.py` goes from 10 cases to 20.

## Why

`ledger/2026-09-21-the-demo-ui-is-a-subset-on-purpose.md` closed one half of
"does the demo's description match the demo" and said in as many words that the
other half was still open: *"Nothing checks that **What the UI shows** still
describes `web/src/`."* This is that half, narrowed to the part that can be
checked mechanically — the counts.

The counts are the part that goes stale *quietly*. An endpoint nobody calls is
at least visible in a diff. `sees 9 of 11 books` stays grammatical, stays
plausible, and stays wrong the moment somebody seeds a twelfth book or moves
the policy's cutoff year — and it is the first concrete claim a visitor reads.

## Alternatives rejected

**A roster of expected counts.** `EXPECTED = {"books": 11, "visible": 9}` is
the idiom this repository uses everywhere else, and it is wrong here: it would
be a *third* place holding the same fact, and the failure it catches is
precisely "two places disagree". Recomputing from the two sources of truth
means adding a book updates the answer automatically and only the README can be
wrong. The cost is two regexes over somebody else's files, which is why all
four patterns carry a never-fires guard.

**Check the whole *What the UI shows* section, not just the counts.** The
section is prose about what panels exist, and matching prose against a UI is
either a keyword grep — which passes on a panel that was deleted and mentioned
— or a rewrite of the README into data. Neither is worth it. The counts are
the falsifiable sentence in that section, and this entry says plainly that the
rest of it is still unchecked.

**Monkeypatch `readme_counts` in the test to prove `main` calls it.** Written,
and `ty` refused it: a module-level `def` is *declared* as that one function,
so no other function is assignable to the attribute — `python-rest-ty` failed
where a bare `ty check` here had not. Passing the check into `main` as an
argument is what the file already does with its other two subjects, and it
types cleanly.

**Leave `readme_counts` reading three fixed paths.** This was the first version
and it is the reason this entry has an Evidence section worth reading. See
below: two mutations survived against it, and both are caught by a fixture.

## Evidence

`sh scripts/check.sh`: **38 passed, all of them.**
`python3 scripts/test_check_demo_surface.py`: **20 passed, 0 failed** (10
before).

The guard against the real tree, which is what it is for:

```
ok    12 of 24 adapter endpoints in the UI, 12 left out on purpose; the
      README's counts agree with the seed and the policy
```

`seed.go` holds 11 books; 9 are published in 1960 or later; the two hidden are
from 1951 and 1955. The README is right today, which is exactly why a guard
that only ever reads the real tree proves very little — see the two survivors.

**Mutations via `scripts/mutate.py`, nineteen, across four files.** Five in
the README, two in `head.toml`, two in `seed.go`, ten in the guard. All
caught by a named test, and the three runs exit 0.

The two that mattered are the ones that **survived the first version**:

| mutation | why it survived | what catches it now |
|---|---|---|
| `problems.extend(readme_counts())` → `problems.extend([])` | every test called `readme_counts` directly; nothing checked that `main` still asks. The real files agree, so deleting the call changed no verdict. | `a real run reports what the README check found` — `main` is handed a check returning a problem nothing else produces |
| `if year >= cutoff` → `if year > cutoff` | `>=` and `>` differ only on a book published *exactly* on the cutoff, and the real seed has none | the fixture has a book published in 1960 exactly, so `three files that agree report nothing` fails under `>` |

Both are the same finding in two shapes: **a check whose only subject is a
correct tree tests almost nothing.** That is the argument for the three path
arguments, and it was measured rather than reasoned — `mutate.py` exiting
non-zero is what interrupted me.

Three further mutations then reported `NOTHING RAN — []` rather than a verdict:
removing a never-fires guard leaves a `None` reaching the narrowing `assert`
below it, which raised and killed the suite before it printed its summary line.
That is lie number two in `mutate.py`'s own list — "no test failed" and "no
test ran" are the same empty output. The fixture loop now catches any exception
and reports it as that case's failure, so a crashing guard is a named `FAIL`.

Two honest notes about the patterns:

- The first version of `SEES`/`BEFORE` used a literal space. The README wraps
  that sentence across a line break, so both matched nothing and the check
  reported the sentences missing. That was the never-fires branch firing
  correctly on the check's own bug, and it is the reason both patterns use
  `\s+`.
- The summary line names the README half only when it ran. It said so
  unconditionally at first, which would have made a fixture run — which never
  reads a README — claim it. A summary that says more than it checked is the
  same lie as a skip that reports green.

## What this does not do

**It does not check the rest of *What the UI shows*.** The section names
panels in prose; only the counts are checked. The original entry's caveat is
narrowed, not closed, and nothing here would notice a paragraph describing a
panel that was deleted.

**It reads the Go seed only.** `seed.go` is the demo's single seeder today and
the Python and Node adapters read the same store, so there is one list of
books — but nothing enforces that, and a second seeder in another language
would be invisible to `BOOK`.

**It assumes the policy is a `year >= NNNN` comparison.** `POLICY` matches that
one spelling. A policy rewritten as `year > 1959`, or as a conjunction, is
reported as unreadable rather than evaluated — deliberately, since evaluating
an arbitrary predicate here would be reimplementing the kernel — but that is a
failure a person has to resolve, not a check that keeps working.

**It does not check the `9 of 11` claim against a running demo.** The numbers
are derived from two source files, not observed from a request as `reader`.
The conformance runner and the e2e both talk to a live stack and neither
asserts this count; making one of them do it would be the stronger check and
was not done.
