# A plain query may read through a view; every other handler still refuses the name, and a guard keeps it that way.

- **Date:** 2026-09-21
- **Author:** Claude Code, working #256 (F5b)
- **Touches:** `crates/slate-server/src/{views.rs,service.rs,lib.rs}`, `crates/slate-serverd/src/{views.rs,main.rs}`, `crates/slate-serverd/tests/views.rs`, `scripts/{check_handlers.py,test_check_handlers.py}`, `docs/views.md`, `site/docs/roadmap.html`
- **Kind:** feature

## What changed

`slate-server` gains a `views` module — a name bound to a base table and an
`Expr` — and `Head` gains a registry of them, installed by `serving_views` and
read by exactly one new function, `authorized_read_source`. That function
resolves a name in the registry first, authorises the **base** table through
the same `authorized_table` every other read uses, and returns the predicate.
`query` calls it and ANDs the predicate onto the caller's filter.
`slate-serverd` lowers each declared view's `QuerySpec` with `slate_sql::lower`
at startup and hands the result over, so `slate-server` gains no SQL parser.

`scripts/check_handlers.py` grows rule 6: every read of `self.views` must be in
a function on the `RESOLVES_VIEWS` roster. That is what makes a second
view-resolving path a diff somebody has to justify rather than a line nobody
notices.

## Why

This is step 2 of `docs/views.md` §3a, and the step-1 commit's whole argument
was that it should be separable: declare first, read last. What makes it safe
is that the opt-in is a *different function*. Widening `authorized_table` —
the obvious move, one line — would have opted every handler in at once,
including the writes §4 refuses and the joins §1 has no answer for, because a
joined row has no single base table to authorise against.

The composition is `view.and(caller)`. An `OR` would let a caller reach rows
the view excludes simply by asking for them, which makes a view a suggestion
rather than a bound.

## Alternatives rejected

**Widen `authorized_table`.** One line, and it opts in nineteen handlers. The
shape of every finding in `security-review.md` is a fix that covered one path
of several; this is that shape run the other way, and the difference is only
that the uncovered paths happen to be the safe ones. A capability that spreads
by default is the same defect as a check that does not.

**Put the `Views` map in `HeadConfig`.** Rejected for the reason the `writes`
field already records in that struct's own comment: every existing caller
builds `HeadConfig` literally, and a new field breaks each of them for
something all but one wants to leave unset. `serving_views` follows
`observing_writes`, consuming, so there is no "the views arrived late" state.

**Give `slate-server` the SQL parser.** It would put `slate-sql` into a crate
every client harness links, for something used once at startup. Lowering in
`slate-serverd` also means a caller embedding `slate-server` as a library can
declare a view by building an `Expr` directly, rather than writing SQL to have
it parsed straight back.

**Resolve the view to a `TableId` at startup and store that.** Faster per read
and wrong: a handler holding an id has skipped the name resolution that
`authorized_table` performs, and rule 1 of the handler guard would not see it
because there is no `self.table(..)` call. Storing the *name* forces every read
through the authorising resolver.

**Skip rule 6 and rely on the one call site.** "Only one handler calls it" is a
claim about today's source, which is exactly the class of claim this repository
has been wrong about three times — the cross-tenant refusal covering one
constructor of two, the duplicate header one `Authenticator` of two, finding 8
covering four handlers of eight. The roster costs eleven lines.

## Evidence

**A mutation found a missing test, and it was the security-relevant one.**
Replacing `authorized_table` with the bare `self.table` inside
`authorized_read_source` — a view resolving its base table without checking
the grant — **survived** all five tests that existed at the time. It survives
because the kernel authorises again inside the planner, so the *rows* are
still protected and the status code is `PERMISSION_DENIED` either way. What
changes is *when* the refusal happens and therefore what it can say: with the
bare resolver, `query_from_proto` runs first and reports the base table's
width in the error. That is security finding 8 exactly, reappearing on a path
it was never written about, and none of the five tests could see it because
all five assert a code.

`a_view_does_not_leak_the_base_tables_width_to_a_caller_with_no_grant` is the
sixth, shaped like the original probe: an ungranted caller asks for column 99
and the refusal must neither report "2 columns" nor echo the ordinal. It
catches the mutation. The general lesson is the one the repository keeps
relearning — **a test that asserts a status code cannot see a change in what
the status code's message contains**, and for finding 8 the message *is* the
vulnerability.

`crates/slate-serverd/tests/views.rs`, six tests against the real binary:
a read through the view returns 2 of 3 rows while the table returns 3; a
caller's filter that *would* match an excluded row returns only the admitted
one; the caller's ordinals address the base table; `insert`, `delete_where` and
`explain` all answer `NOT_FOUND` for the view's name while `explain` answers
for the table; a role granted nothing gets `PERMISSION_DENIED` rather than
rows; and the finding-8 probe above.

Rust mutations, via `scripts/mutate.py`: the view's predicate dropped, the
caller's filter dropped instead, the composition turned into an `OR`, the
registry never consulted, and the grant check skipped — each caught by named
tests. One case first reported `NOTHING RAN` because I wrote `Expr::any_of`,
which does not exist; the script refused to score it rather than calling it a
survivor, which is the third of the four lies its own docstring lists.

`scripts/test_check_handlers.py`: 27 pass. Three of those broke when rule 6
arrived and needed the shared `PREAMBLE` extended; four are new and exist
because of the finding below. `python3 scripts/check_handlers.py` reports "2
view-registry reads all rostered".

**Rule 6 shipped with no tests of its own, and a mutation run said so.**
Breaking its roster check, its never-fires guard and its staleness check all
*survived*: the existing cases passed because the `PREAMBLE` had been extended
to satisfy the new rule, which is precisely the shape of a rule that is present
and unexercised. Extending a fixture to make a suite green is not the same as
testing the thing the fixture now contains, and only the mutation run
distinguished them. The four cases added afterwards catch all four mutations,
including a fifth written to pin the `{owner}` fix.

Rule 6 was exercised against a hand-written rogue file before being trusted: a
handler reading `self.views` with a context but no `authorized_table` is
reported by name. That run also found a real defect — the message printed the
literal `{owner}` rather than the function, because the interpolation sat on a
continuation line that was not an f-string. **Rule 2's message had the same bug
and had presumably always had it**, so both are fixed. A guard whose message
cannot name the offending function costs the reader a grep.

Two test expectations I wrote from reading were wrong, and the suite said so:
`Identity::nobody()` sends no principal at all and gets `UNAUTHENTICATED`, not
`PERMISSION_DENIED` — a different answer to a different question, so the test
now uses an authenticated role with no grants; and `actions = ["all"]` does not
include `explain`, which is privileged, so the control case needed
`everything`.

## What this does not do

**Only `query`.** `get`, `join`, `chain`, `aggregate`, every explain, every
write and every relation still resolve through `Catalog::table_by_name` and
answer `NOT_FOUND` for a view. For a write that is correct and §4 says so. For
a join it is a message that should be better, and the honest version —
"`recent` is a view, and a join cannot read through one yet" — needs the join
path to know views exist, which is the thing this commit deliberately did not
do.

**No view over a view.** A view's base name is resolved against the catalog,
not against the registry, so naming one view inside another is refused at load
as an unknown table. Not argued for, just not built; the composition would be
an `and` of two predicates and the loop would need a cycle check.

**Nothing measures the cost.** A view adds one `BTreeMap` lookup per read and
one `Expr::and`, which simplifies `True` away — so an ordinary table's read
should be unchanged and a view's should cost what the extra conjunct costs the
planner. That is a prediction from reading the code, not a measurement, and it
is exactly the kind this repository has been wrong about before. It is not
benchmarked because `slate-headbench` has no view case, and adding one is its
own task.

**The demo declares no view.** `examples/explorer/head.toml` still has no
`[[views]]` section, so the three-SDK conformance runner and the browser e2e
never exercise one. The feature is proven against a real server in
`tests/views.rs` and nowhere else.

**The Rust mutations cover the read path, not the registry's contents.**
`scripts/mutate.py` breaks the composition, the resolver's view branch and the
base-table authorisation; nothing mutates `views::lowered`, because a view
whose predicate lowered wrongly would need a fixture whose `WHERE` distinguishes
two lowerings, and `lower::build` has its own suite in `slate-sql`.
