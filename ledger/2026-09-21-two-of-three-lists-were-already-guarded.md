# Two of the three lists I set out to guard already were; the third's own entry said what it needed.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #267 (F5m)
- **Touches:** `scripts/{check_demo_surface.py,test_check_demo_surface.py,check.sh,test_check_sh.py}`, `.github/workflows/ci.yml`, `examples/explorer/web/src/api.ts`, `docs/{full-text.md,orm-comparison.md}`, `ledger/2026-09-21-the-demo-ui-is-a-subset-on-purpose.md`
- **Kind:** guard

## What changed

`scripts/check_demo_surface.py`: every endpoint the adapters serve is either
called by the demo's UI or named in `NOT_IN_THE_UI` with a reason. Both
directions, two never-fires guards, ten cases in
`scripts/test_check_demo_surface.py`, in `check.sh` (37, from 34) and in CI.

The demo UI's coverage is **12 of 24**, not the 11 two documents said.

One comment in `examples/explorer/web/src/api.ts` corrected: it credited the
server's fingerprint check for catching drift in a list the server never sees.

## Why

The task was "three lists this repository maintains by hand". Two of the three
turned out not to be:

**The demo UI's `TABLES`.** `test/api.test.ts` already parses `head.toml` and
compares it table by table and in order, with a `NOT_IN_THE_UI` roster naming
the tables deliberately absent and why, checked both ways. I got as far as
generating it from the catalog with a new `codegen.py --web` before that test
failed and told me `posts` is left out on purpose — "nothing seeds it and the
UI has no way to render a list cell". Generation would have deleted that
reason and replaced a test that names the table with a diff that does not.
Reverted.

What *was* wrong is the comment above the list, which said "the server checks
it on every request and refuses a declaration that disagrees, so this cannot
drift silently". True of the adapters' declarations, which carry a
fingerprint. False of this one: it is column headers, it stays in the browser,
nothing hashes it, and `api.test.ts` is what catches drift. A stale entry here
is a wrong label over a right value.

**`crates/slate-sql/tests/sql.rs` naming its operator list by hand.** There is
no such file, and `front_end.rs` has no operator list. The claim was mine and
described nothing.

**The demo UI's endpoint coverage** was the real one, and its own entry had
already stated both the gap and the objection to closing it:

> A guard is writable (both lists are greppable, as above) and was not
> written, because a guard that fails whenever the adapters gain an endpoint
> would fail on every surface task and be switched off.

That is right about a guard that only compares two lists, and it names its own
answer. A roster with reasons — the `EXPECTED_REFUSALS` idiom already in five
places here — fails with a one-line remedy instead: not "you added an
endpoint" but "say why the UI does not show it". Adding an endpoint and a line
is a decision recorded; adding an endpoint and nothing is a decision nobody
made.

## Alternatives rejected

**Generate the UI's `TABLES` anyway, and keep the exclusion as a comment.** A
generated file cannot carry `NOT_IN_THE_UI`'s *checked* reasons — an entry
naming a table `head.toml` no longer has fails today, and a comment in a
generated file fails nothing. The existing test is strictly more informative.

**Compare the UI's endpoints to the adapters' with no roster.** The entry's
objection, and it stands: every surface task adds an endpoint before it adds a
panel, so a bare comparison is red on every one of them and gets deleted.

**Put the roster in the web app's test suite beside `NOT_IN_THE_UI` for
tables.** Tempting for symmetry. Rejected because the subject is the Python
adapter's route table, and a TypeScript test parsing Python to find it is a
worse coupling than a Python script reading both.

**Read all three adapters' route tables.** The conformance runner already
holds the three to each other — a route in one and not the others fails there,
with a better message than this could give. Here it is the *list* that
matters, and one authoritative copy of it is enough.

**Count endpoints from the UI's typed API surface rather than by grep.** More
precise, and it would miss the case worth catching: an endpoint mentioned in a
comment or a dead branch reads as coverage. Being generous means this stays
quiet rather than crying wolf, which is the property the entry's objection was
about.

## Evidence

`python3 scripts/check_demo_surface.py` against the real tree:

```
ok    12 of 24 adapter endpoints in the UI, 12 left out on purpose
```

**Twelve, where two documents said eleven.** `/api/restore-unchanged` reached
the UI one feature after the count was taken and the sentences did not follow.
That is the same drift the entry measured, one more time, and the reason the
guard is worth its lines. Both corrected.

`scripts/test_check_demo_surface.py`: **10 passed**. Ten mutations via
`scripts/mutate.py --dialect python`, all caught by named tests: the roster
check skipped, the two staleness directions skipped, both never-fires guards
removed, `reached` computed without intersecting the served set, both regexes
narrowed so a hyphenated endpoint stops matching, only the first UI file read,
and `.tsx` files ignored.

**Two of those survived the first run, and both were defects in my tests:**

- *The no-routes guard.* Both never-fires messages ended "so this checked
  nothing", which is what the case asserted — so removing the routes guard
  passed on the *calls* guard's message. The two messages now say different
  things and the case names the route table's own words.
- *Reading only the first UI file.* Every fixture put its endpoints in
  `api.ts`, which sorts first, so truncating the walk changed nothing. The
  fixtures now split calls across `api.ts` and `panels.tsx`, and a case exists
  whose finding depends on the second.

**A third thing the mutation run made me fix before it could run at all:** the
first fixtures served three endpoints, so the staleness check reported the
other eleven rostered ones and the passing case failed eleven times on entries
it was not testing. The fixture now serves the whole roster, which is what
`test_check_handlers.py` records under its own `PREAMBLE` and which I walked
into anyway.

`sh scripts/check.sh`: 37, from 34. `scripts/test_check_sh.py` refused the two
new CI steps until they were accounted for. `ruff` at the root refused
`RUF005` on a list concatenation, which is the second time this session the
root Python checks have caught something in a file nobody had linted before.
`npm test` in `examples/explorer/web`: 13 passed.

## What this does not do

**It reads one adapter's routes.** If the Python adapter dropped an endpoint
the Go and TypeScript ones still serve, this would call the roster entry stale
and say nothing about the other two. The conformance runner is what holds the
three together, and it would fail first.

**An endpoint named anywhere under `web/src` counts as called.** A path in a
comment, in a dead branch, or in a type that nothing constructs all read as
coverage. That is deliberate — see above — and it means the count is an upper
bound on what a visitor can actually reach.

**Nothing checks the reasons.** An entry whose justification has stopped being
true — a panel that would now render fine — keeps passing, because the check
is on the name. Same limitation as every roster here, stated for the same
reason.

**The README half of the original entry is still open.** Nothing checks that
*What the UI shows* still describes `web/src/`. It is prose against code and a
harder problem than two lists.

**The `api.ts` comment is now right and still unguarded.** Nothing stops the
next person writing a similarly confident and wrong sentence about why a list
is safe. That is what code review is for and this repository has no
mechanical answer to it.
