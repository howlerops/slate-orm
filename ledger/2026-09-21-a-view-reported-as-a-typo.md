# A declared view was reported as a misspelled table by every path but one. One function, six paths.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #257 (F5c)
- **Touches:** `crates/slate-server/src/service.rs`, `crates/slate-serverd/tests/views.rs`, `scripts/{check_handlers.py,test_check_handlers.py}`, `docs/views.md`
- **Kind:** fix

## What changed

`Head::table`'s refusal moved into `Head::no_such_table`, which consults the
view registry and answers *"`notes` is a view over `docs`, and only a plain
query can read through one"* for a declared view, and the old *"no table named
`notes`"* for everything else. Every handler but `query` resolves through
`Head::table`, so writes, joins, chains, aggregates, point gets, explains and
relation steps all get the honest message from one edit.

`RESOLVES_VIEWS` in `scripts/check_handlers.py` gains `no_such_table`, and the
roster's own comment stops claiming it holds only view *resolvers* — it holds
three functions that read `self.views` and one of them resolves.

## Why

`docs/views.md` §3a predicted this message and called it the price of the
declare-first build order: "a message to improve, not a hole". The step-2
commit narrowed it to every path except `query` and left it. It is worth
paying now rather than later because the failure is a *lie to the operator*,
not a missing feature: they declared `notes` in their own `head.toml`, joined
it, and were told the name does not exist. The next thing they do is check the
spelling, which is correct, and then they are stuck with no signal at all.

It is one function because `Head::table` is the only place a name-to-table
refusal is produced for a handler. That was worth checking rather than
assuming — `convert.rs` produces the same sentence twice — and the reason
those two cannot fire for a view is rule 3: every converter call has an
authorisation above it, so by the time a converter resolves a name, that name
is a real table.

## Alternatives rejected

**Teach the join path about views.** What the step-2 ledger entry assumed the
fix would be, and it is the expensive shape: every path that wanted the better
message would read `self.views`, each read would need a roster entry, and the
roster would stop meaning "these may resolve a view". One function that
returns a `Status` costs one entry and cannot resolve anything.

**Leave the message alone and document it.** The message *is* the
documentation for anybody who does not read `docs/views.md`, which is most
people hitting it.

**Say "is a view" and stop there, without the base table.** This was written,
committed to a working tree, and then withdrawn on re-reading §4, which had
already settled the same question for the write refusal: "the refusal should
name the base table, because the useful next step is to write to that." A join
refusal saying less than the write refusal about the same view is an
inconsistency nobody could explain, and §2 points the same way — the caller
needs the grant on the base table regardless.

The reason first written for withholding it was that the refusal is produced
before any grant is checked, so it reaches a caller who may hold nothing, and
a *link* between two names is more than the table-existence disclosure
`authorized_table` documents as deliberate. That argument is real and it is
small: table names are already enumerable to such a caller, because an unknown
name answers `NOT_FOUND` and a known one with no grant answers
`PERMISSION_DENIED`. What the message adds over what two requests already buy
is which of those tables this view reads — one probe, against every operator
reading a refusal they cannot act on.

**Collapse the two refusals so a view is indistinguishable from a typo.** The
other direction, and it would hide a real misconfiguration behind a message
that says the opposite of the truth. `authorized_table` already records why
this repository does not hide existence: it makes the common case — a genuine
mistake — indistinguishable from an attack nobody is mounting.

## Evidence

`cargo test -p slate-serverd`: 197 + 3 + 4 + 13 + 7 + 19 + 49 + 2 + 7 = 301
tests, all passing. `cargo test -p slate-server --lib --test security_probe
--test schema_check --test server --test rls_join`: 22 + 6 + 18 + 15 + 25 = 86,
all passing — the suites that assert refusals, run because the refusal moved.

`only_the_query_path_knows_what_a_view_is` grew from three paths to six —
insert, delete_where, get, join, aggregate, explain — and now asserts the
**message** rather than only the code. That is the step-2 lesson applied
directly: a test that asserts a status code cannot see a change in what the
status code's message contains, and here the message is the entire change.
`chain` and the relation steps are not sampled; they share
`authorize_join_inputs` and `Head::table` with paths that are, and the
structural half of the claim is that all six reach the one function.

`a_name_that_is_neither_gets_the_ordinary_refusal` is the contrast case and
exists because the positive assertion alone is satisfiable by a function that
calls *every* unknown name a view. It asserts the ordinary sentence, and
asserts the absence of both "is a view" and the base table's name.

Rust mutations, via `scripts/mutate.py`, four runs, all caught by a named test:

- `no_such_table` never recognising a declared view → caught by
  `only_the_query_path_knows_what_a_view_is`.
- the refusal naming the view where it should name the base table → caught by
  the same test, which is why the assertion is on the whole phrase rather than
  on the substring "is a view".
- the ordinary refusal rewritten to call an unknown name a view → caught by
  `a_name_that_is_neither_gets_the_ordinary_refusal`, which is the case that
  test exists for.
- the first of these was written twice: the obvious anchor
  `if let Some(view) = self.views.get(name) {` occurs in
  `authorized_read_source` as well, and `mutate.py` refused the run rather than
  patching the wrong site. That is lie 1 from its own docstring, caught by the
  tool rather than by attention.

**The roster mutation could not go through `mutate.py`, and was run by hand.**
`scripts/check_handlers.py` is a guard, not a test suite — it prints one `ok`
line and no `N passed, M failed` — so the `python` dialect scored the run as
"no test results at all" and refused it, correctly. Demonstrated instead by
copying the script to a scratch path, renaming the roster key there, and
running that copy against the real tree: it reports both directions, the
unrostered read at `service.rs:490` naming `no_such_table`, and the stale entry
naming `no_such_table_`. The working tree was never mutated.

`sh scripts/check.sh`: 34 passed. `python3 scripts/test_check_handlers.py`: 27
passed. `python3 site/check/docs.py`: green.

**Three of the guard's own tests broke and were repaired by extending a
fixture**, which is exactly the move the step-2 entry warned about. The repair
is legitimate here and the difference is worth stating: step 2 added a *rule*
and extended the fixture, so the fixture hid the rule's absence of tests. This
change adds a roster *entry* and no rule, so there is no new behaviour for a
test to cover — the rule that was already tested is the one still running. The
staleness check is what broke: every positive fixture failed on a roster entry
naming a function the fixture did not contain, which is the check doing its
job.

## What this does not do

**The demo still declares no view.** `examples/explorer/head.toml` has no
`[[views]]` section, so the three-SDK conformance runner and the browser e2e
still never start a server with a view in it. That was the other half of #257
and it is a larger change — each adapter holds a table allowlist, and two of
the three clients cannot build a query without a declared schema, so "how does
a schema-holding client name a view?" has to be answered first. It has an
answer (a view may not narrow columns, and `query` sends no fingerprint, so the
base table's column list is the view's) and nothing has tested it.

**`chain` and the relation steps are argued, not asserted.** See above.

**Nothing asserts that `Head::table` stays the only producer of this
refusal.** `convert.rs` has two more, unreachable for a view today because of
rule 3; a fourth added somewhere that authorises later would answer "no table
named" again and nothing would notice. The step-1 entry recorded the same gap
for `table_by_name` being the only name-to-`TableDef` lookup, and this is the
same missing guard seen from the refusal side.

**The disclosure judgement is a judgement.** No measurement says one probe's
worth of linkage is acceptable; the argument is that table names are already
enumerable and that §2 sends the caller to the base table anyway. If a
deployment needs name-hiding, the thing to change is the `NOT_FOUND` /
`PERMISSION_DENIED` split, which is where the enumeration actually comes from,
and this message would follow from that rather than be fixed separately.
