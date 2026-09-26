#!/usr/bin/env python3
"""Every `closed` caveat verdict names something, and that something is here.

    python3 scripts/check_closed_caveats.py

`scripts/caveats.py` records a verdict per caveat and verifies only that a
`closed` one names *something*. `scripts/check_caveat_citations.py` then
verifies that any path in that name resolves. Neither asks the question the
verdict actually makes:

  > **167 `closed` verdicts rest on evidence of very uneven strength.** … Where
  > the caveat was about a test existing, or a guard covering a case, I matched
  > it against the completed task whose title names the same gap, which is good
  > evidence and not proof. A wrong `closed` is invisible.
  > — `ledger/2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md`

A reading pass on 2026-09-26 found that 188 of the 194 `closed` verdicts cite
an entry or a task title rather than anything in the tree, and recorded that as
the residual. This is the residual: a **witness** per closed caveat — a file
that must exist, or a string that must occur in a named file — chosen so that
it is present *because* the caveat was closed and would go away if the closure
were reverted.

# What a witness is, and what it is not

A witness is the cheapest artifact whose presence the closure implies. For "no
`HAVING` in the SQL front end" it is the word `HAVING` in the parser; for "the
demo declares no view" it is `[[views]]` in `examples/explorer/head.toml`; for
"nothing runs these benchmarks" it is `slate-slatedb` in `run_examples.sh`.

It is **not** proof the caveat is closed. A witness can be present for another
reason, and a feature can exist while the caveat's real complaint stands — the
class `2026-09-26-the-closed-verdicts-audited.md` found with "the 762 is still
not audited", where the work happened and answered the easier half. What the
witness catches is the other direction, and it is the direction a roster can
catch: the closure **regressing** — a file deleted, a feature reverted, a guard
removed — while the verdict keeps saying done and every listing keeps hiding it.

The witnesses are deliberately coarse. `("having", "…/front_end.rs")` passes on
any test mentioning `having`, not only the one the closure added. A tighter
witness would be a test name, and a test name is renamed by ordinary work: the
guard would then fail on a rename, a person would loosen it under time
pressure, and the roster would have taught people to weaken it. Coarse and
kept is worth more than exact and switched off — the same trade
`check_site_css.py` makes with substring matching, argued at length there.

# EXEMPT, and why it is a list rather than a rule

Some closures leave nothing in the tree. A measurement that came back null
changes no file; a review re-read under a wider frame produces an entry and no
artifact. Those are named in `EXEMPT` with the reason, one sentence each, in
the idiom this repository runs on: **a list you are forced to edit is a list
that stays true.** A rule — "exempt anything closed by a measurement" — would
absorb the next closure that should have had a witness and was not given one.

**Every `closed` verdict must be in `WITNESSED` or in `EXEMPT`.** A new one in
neither fails this check, which is the point: it is not possible to close a
caveat here without saying, in the tree, what closed it.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

#: `name -> (needle, path)`. A `needle` of `None` means the path must merely
#: exist. `path` is a `git grep` pathspec, so a directory covers its tree.
WITNESS: dict[str, tuple[str | None, str]] = {

 "alias-sql": ("alias", "crates/slate-sql/tests/front_end.rs"),
 "ambiguity": ("ambigu", "crates/slate-sql/src"),
 "array-codegen": ("element", "scripts/codegen.py"),
 "array-literal-sql": ("array", "crates/slate-serverd/src/lang/pred.rs"),
 "array-schema": ("Array", "crates/slate-schema/src/table.rs"),
 "array-wire": ("array", "crates/slate-server/proto/slate/v1/records.proto"),
 "as-view-clients": ("as_view", "clients/python/src/slate/schema.py"),
 "batch-clients": ("Batch", "clients/go/slate/batch.go"),
 "brackets-sql": ("bracket", "crates/slate-sql/src/lib.rs"),
 "bucket-provenance": (None, "crates/slate-wasm/tests/bucket_provenance.rs"),
 "build-output": (None, "scripts/check_build_output.py"),
 "build-stamp": ("build:", "crates/slate-slatedb/examples/ascending_walk.rs"),
 "calendar-builders": ("CalendarPart", "clients/python/src/slate/__init__.py"),
 "catalog-ts": (None, "examples/explorer/web/src/catalog.ts"),
 "chain-compute": ("pub compute", "crates/slate-kernel/src/chain.rs"),
 "chain-computed-clients": ("chain", "clients/python/tests/test_grouped_join.py"),
 "chain-sql": ("chain", "crates/slate-sql/src/sql.rs"),
 "check-sh-four": ("python-rest-ruff", "scripts/check.sh"),
 "checked-field": ("checked", "scripts/caveats.py"),
 "cited-docs": (None, "scripts/check_cited_docs.py"),
 "closed-caveats-guard": (None, "scripts/check_closed_caveats.py"),
 "codegen-print-schema": ("print-schema", "scripts/codegen.py"),
 "compute-input": ("ComputeSpec", "crates/slate-wasm/src"),
 "computed-clients": ("computed", "clients/go/slate/join_test.go"),
 "conformance-cases": ("case", "examples/explorer/conformance/conformance.py"),
 "conformance-include-deleted": ("include_deleted", "examples/explorer/conformance/conformance.py"),
 "conformance-passed": ("passed", "examples/explorer/conformance/conformance.py"),
 "conformance-restore": ("restore", "examples/explorer/conformance/conformance.py"),
 "conformance-window": ("window", "examples/explorer/conformance/conformance.py"),
 "contains-wire": ("contains", "crates/slate-server/proto/slate/v1/records.proto"),
 "correction-guard": (None, "scripts/check_retired_claims.py"),
 "cost-prose-docs": ("DOCS", "scripts/check_cost_prose.py"),
 "cost-prose-readme": ("README_GLOBS", "scripts/check_cost_prose.py"),
 "cursor-wire": ("cursor", "crates/slate-server/proto/slate/v1/records.proto"),
 "date-trunc-month": ("Month", "crates/slate-kernel/src/scalar.rs"),
 "decimal-literal": ("Decimal", "crates/slate-sql/src/lower.rs"),
 "decimal-scalar": ("Decimal", "crates/slate-kernel/src/scalar.rs"),
 "decimal-wire": ("decimal_value", "crates/slate-server/proto/slate/v1/records.proto"),
 "delete-if-unchanged": ("delete_if_unchanged", "crates/slate-kernel/src/record.rs"),
 "demo-restore": ("restore", "examples/explorer/web/src/api.ts"),
 "deployed": (None, "examples/deployed/run.sh"),
 "depth-limit": ("depth", "crates/slate-server/src/convert.rs"),
 "distinct-sql": ("DISTINCT", "crates/slate-sql/src/sql.rs"),
 "docs-stamped": ("build:", "docs/performance.md"),
 "four-deps": ("object_store", "crates/slate-slatedb/examples/bucket_layout.rs"),
 "generated-rows-live": ("editions", "examples/explorer/backends/go/handlers.go"),
 "grammar-cases": ("grammar", "crates/slate-sql/tests/front_end.rs"),
 "group-many-join": ("a_join_groups_by_more_than_one_key", "crates/slate-sql/tests/front_end.rs"),
 "handlers-converter": ("WIRE", "scripts/check_handlers.py"),
 "handlers-guard": (None, "scripts/check_handlers.py"),
 "handlers-rosters": ("AUTHENTICATORS", "scripts/check_handlers.py"),
 "handlers-sources": ("slate-serverd", "scripts/check_handlers.py"),
 "has-many-derive": ("has_many", "crates/slate-derive/src/lib.rs"),
 "having-join": ("having", "crates/slate-sql/tests/front_end.rs"),
 "having-sql": ("HAVING", "crates/slate-sql/src/sql.rs"),
 "head-toml-array": ('type = "array"', "examples/explorer/head.toml"),
 "head-toml-view": ("[[views]]", "examples/explorer/head.toml"),
 "headbench-view": ("view", "crates/slate-headbench/examples/head_concurrency.rs"),
 "if-unchanged-wire": ("if_unchanged", "crates/slate-server/proto/slate/v1/records.proto"),
 "include-deleted-clients": ("include_deleted", "clients/python/src"),
 "join-computed-wire": ("computed", "crates/slate-server/proto/slate/v1/records.proto"),
 "metrics-purge": ("purge", "crates/slate-serverd/src/observe.rs"),
 "mutate-dialects": ("pytest", "scripts/mutate.py"),
 "mutate-marker": ("marker", "scripts/mutate.py"),
 "mutate-should-panic": ("should_panic", "scripts/mutate.py"),
 "mutation-claims": (None, "scripts/check_mutation_claims.py"),
 "mutation-records": (None, "ledger/mutations"),
 "narrowed-verdict": ("narrowed", "scripts/caveats.py"),
 "or-sql": ("Or", "crates/slate-sql/src/lower.rs"),
 "order-chain-clients": ("test_ordering_a_chains_groups", "clients/python/tests/test_grouped_join.py"),
 "orderby-join": ("the_order_by_line_needs_a_group_by_on_a_join", "crates/slate-sql/tests/front_end.rs"),
 "panels-decade": ("decade", "examples/explorer/web/src/panels.tsx"),
 "playground-writes": ("insert", "site/workbench.js"),
 "predicate-writes-wire": ("delete_where", "crates/slate-server/proto/slate/v1/records.proto"),
 "provenance-readme": ("README_GLOBS", "scripts/check_table_provenance.py"),
 "purge-called": ("purge", "examples/explorer/backends/go/handlers.go"),
 "python-decoder": (None, "clients/python/tests/test_generated.py"),
 "quickstarts": (None, "site/check/quickstarts.py"),
 "readonly-window": ("read-only", "crates/slate-server/tests/leadership.rs"),
 "relations-wire": ("related", "crates/slate-server/proto/slate/v1/records.proto"),
 "restore-write": ("restore", "crates/slate-kernel/src/record.rs"),
 "retry-transact": ("Transact", "clients/go/slate/batch.go"),
 "returning-wire": ("returning", "crates/slate-server/proto/slate/v1/records.proto"),
 "run-examples": ("slate-slatedb", "scripts/run_examples.sh"),
 "none-last": (None, "scripts/check_none_last.py"),
 "sections-refused": (None, "crates/slate-headbench/src/sections.rs"),
 "scalar-go": (None, "clients/go/slate/scalar.go"),
 "security-probe": ("explain_join", "crates/slate-server/tests/security_probe.rs"),
 "site-claims": (None, "scripts/check_site_claims.py"),
 "site-css": ("styles.css", "scripts/check_site_css.py"),
 "stale-binary": ("older", "clients/python/tests/conftest.py"),
 "stored-schema": ("schema", "crates/slate-kernel/src/migrate.rs"),
 "table-provenance": (None, "scripts/check_table_provenance.py"),
 "through-wire": ("through", "crates/slate-server/proto/slate/v1/records.proto"),
 "time-fn-join": ("hour", "crates/slate-sql/src/lower.rs"),
 "timer-honest": (None, "crates/slate-wasm/tests/timing.rs"),
 "timing-batched": ("batch", "crates/slate-wasm/tests/timing.rs"),
 "type-mapping": ("Timestamp", "crates/slate-orm/src/field.rs"),
 "typed-failures": ("Violation", "clients/go/slate/details_test.go"),
 "validation-doc": (None, "docs/validation.md"),
 "verify-called": ("verify_of", "crates/slate-serverd/src/main.rs"),
 "view-over-view": ("view", "docs/views.md"),
 "view-read-path": ("view", "crates/slate-server/src/convert.rs"),
 "views-toml": ("views", "crates/slate-serverd/src/config.rs"),
 "wasm-stale": ("stale_sources", "site/check/workbench.py"),
 "window-clients": ("Window", "clients/go/slate/query.go"),
 "window-sql": ("OVER", "crates/slate-sql/src/sql.rs"),
 "window-wire": ("window", "crates/slate-server/proto/slate/v1/records.proto"),
 "workbench-chain": ("the aliased chain plans as three steps", "site/check/workbench.py"),
 "zones": (None, "crates/slate-kernel/src/zones.rs"),
 "zones-dst": ("tzdata", "crates/slate-kernel/src/zones.rs"),}

#: Closures that leave nothing in the tree, with the reason each leaves nothing.
#:
#: Written out rather than inferred from the verdict's prose, because the rule
#: that would infer them ("closed by a measurement") is exactly the rule that
#: would swallow the next closure somebody forgot to witness.
EXEMPT: dict[str, str] = {
    "measured": (
        "closed by a measurement. A number that came back null, or that moved a "
        "constant already under `check_cost_prose.py`, leaves no artifact of its "
        "own — the measurement is the entry, and `check_caveat_citations.py` "
        "already requires that entry to open"
    ),
    "read": (
        "closed by a reading or a probe recorded in an entry: a review "
        "re-examined under a wider frame, a set of gap rows re-read for their "
        "real shape. The output is prose and the entry is the artifact"
    ),
    "stamped": (
        "closed by the state of `docs/caveat-status.json` itself — a count of "
        "unread caveats, answered by every open verdict carrying a `checked` "
        "date. `caveats.py --unread` is the check, and `check.sh` runs it"
    ),
}

#: Every `closed` caveat, and the witness that must still be in the tree.
#:
#: Keyed the way `caveats.py` keys a verdict — entry filename and the first 60
#: characters of the bullet — so a reworded caveat orphans its row here for the
#: same reason it orphans its verdict, and is read again rather than silently
#: keeping a witness chosen for a different claim.
WITNESSED: dict[tuple[str, str], str] = {
    ('2026-09-20-cis-ruff-is-newer-than-mine-too.md',
     'Two occurrences, one rule.'):
        'none-last',
    ('2026-09-21-the-benchmarks-run-now-and-a-fourth-was-broken.md',
     'A nonsense section argument runs nothing and exits 0.'):
        'sections-refused',
    ('2026-09-26-the-closed-verdicts-audited.md',
     '188 `closed` verdicts still rest on a task title.'):
        'closed-caveats-guard',
    ('2026-09-13-a-site.md',
     'The quickstarts are not checked automatically.'):
        'quickstarts',
    ('2026-09-14-a-keyspace-viewer.md',
     'The bucket listing is static and nothing checks it is curren'):
        'bucket-provenance',
    ('2026-09-14-a-keyspace-viewer.md',
     'The WAL in that listing still holds 10.7 MB.'):
        'bucket-provenance',
    ('2026-09-14-a-kitchen-sink-example.md',
     'No `HAVING`.'):
        'having-sql',
    ('2026-09-14-a-kitchen-sink-example.md',
     'No `OR`, no sub-queries, no window functions, no date arithm'):
        'or-sql',
    ('2026-09-14-a-playground-that-runs-the-kernel.md',
     'No writes.'):
        'playground-writes',
    ('2026-09-14-a-real-dataset.md',
     'No date or time handling at all.'):
        'zones',
    ('2026-09-14-a-real-dataset.md',
     'The 1,290 bytes a row was measured and not investigated.'):
        '=measured',
    ('2026-09-14-what-the-timer-measures.md',
     'Nothing asserts the reported time is *accurate'):
        'timer-honest',
    ('2026-09-15-a-chains-computed-value-and-a-type-tag-in-a-label.md',
     "`ORDER BY` a chain's computed value is untested from a clien"):
        'order-chain-clients',
    ('2026-09-15-a-grouped-joins-order-and-a-name-that-meant-two-things.md',
     'Chains are still not in the SQL front end.'):
        'chain-sql',
    ('2026-09-15-a-grouped-joins-order-and-a-name-that-meant-two-things.md',
     'One group key per join, still.'):
        'group-many-join',
    ('2026-09-15-a-having-and-the-type-underneath-it.md',
     'No `HAVING` on a join or a chain.'):
        'having-join',
    ('2026-09-15-a-having-and-the-type-underneath-it.md',
     'No `OR`, in `HAVING` as in `WHERE`,'):
        'or-sql',
    ('2026-09-15-a-month-is-not-a-number-of-seconds.md',
     "Nothing checks the `slate-wasm` bundle's freshness the same "):
        'wasm-stale',
    ('2026-09-15-a-new-index-returns-nothing.md',
     '`verify` is not called by anything yet.'):
        'verify-called',
    ('2026-09-15-a-new-index-returns-nothing.md',
     'An additive column change is refused, not applied.'):
        'stored-schema',
    ('2026-09-15-a-second-group-key.md',
     '`HAVING` on a join or a chain is still a refusal.'):
        'having-join',
    ('2026-09-15-a-second-group-key.md',
     "Nothing here says the parser's own `HAVING` is reachable on "):
        'having-join',
    ('2026-09-15-a-value-belonging-to-the-join.md',
     'The extra flatten on the grouped path is unmeasured.'):
        '=measured',
    ('2026-09-15-a-value-belonging-to-the-join.md',
     'Go and TypeScript still cannot build any of this.'):
        'scalar-go',
    ('2026-09-15-a-value-belonging-to-the-join.md',
     "The wasm binding's `JoinSpec` still pins compute to the left"):
        'compute-input',
    ('2026-09-15-a-value-belonging-to-the-join.md',
     'Nothing reads `JoinedRow.computed` yet except the tests.'):
        'computed-clients',
    ('2026-09-15-having-on-a-join.md',
     '`OR` is still refused in HAVING'):
        'or-sql',
    ('2026-09-15-money-that-does-not-drift.md',
     'It does not cross the wire.'):
        'decimal-wire',
    ('2026-09-15-money-that-does-not-drift.md',
     'No arithmetic on decimals in `Scalar`.'):
        'decimal-scalar',
    ('2026-09-15-money-that-does-not-drift.md',
     'The SQL front end has no decimal literal.'):
        'decimal-literal',
    ('2026-09-15-one-group-over-everything.md',
     '`SELECT DISTINCT` is still not a keyword'):
        'distinct-sql',
    ('2026-09-15-one-space-for-a-joined-query.md',
     'Chains are untouched.'):
        'chain-sql',
    ('2026-09-15-one-space-for-a-joined-query.md',
     '`ORDER BY` is still refused on a join'):
        'orderby-join',
    ('2026-09-15-one-space-for-a-joined-query.md',
     "An aggregate's ambiguous unqualified name resolves left with"):
        'ambiguity',
    ('2026-09-15-one-table-twice.md',
     '`GROUP BY` on a join or chain still takes one key'):
        'group-many-join',
    ('2026-09-15-pages-that-do-not-shift.md',
     'No cursor on the wire.'):
        'cursor-wire',
    ('2026-09-15-relationships-in-the-derive.md',
     'Still nothing in the SDKs.'):
        'relations-wire',
    ('2026-09-15-relationships-in-the-derive.md',
     'No `through` / many-to-many.'):
        'through-wire',
    ('2026-09-15-relationships-without-n-plus-one.md',
     'There is no `#[record(has_many(...))]` attribute yet.'):
        'has-many-derive',
    ('2026-09-15-relationships-without-n-plus-one.md',
     'Nothing reaches the SDKs.'):
        'relations-wire',
    ('2026-09-15-the-guard-nothing-called.md',
     'The warning is not tested.'):
        'readonly-window',
    ('2026-09-15-the-hours-were-already-local.md',
     "The gRPC protocol does not carry a join's computed column."):
        'join-computed-wire',
    ('2026-09-15-the-hours-were-already-local.md',
     'Go and TypeScript still have no computed columns at all'):
        'scalar-go',
    ('2026-09-15-the-hours-were-already-local.md',
     'Chains still have no computed column of their own.'):
        'chain-compute',
    ('2026-09-15-the-hours-were-already-local.md',
     "A join's computed column reads the left table and its aggreg"):
        'compute-input',
    ('2026-09-15-the-hours-were-already-local.md',
     'Fixed offsets do not know about daylight saving'):
        'zones-dst',
    ('2026-09-15-the-hours-were-already-local.md',
     '`date_trunc` still stops at the day.'):
        'date-trunc-month',
    ('2026-09-15-the-lost-update.md',
     'Nothing on the wire.'):
        'if-unchanged-wire',
    ('2026-09-15-the-lost-update.md',
     'No `delete_if_unchanged`.'):
        'delete-if-unchanged',
    ('2026-09-15-the-values-that-arrived-and-vanished.md',
     "A chain's computed value is untested from any client."):
        'chain-computed-clients',
    ('2026-09-15-the-values-that-arrived-and-vanished.md',
     'Nothing prevents the next shape from being dropped the same '):
        'computed-clients',
    ('2026-09-15-the-wal-the-collector-would-not-take.md',
     'It does not make the listing self-checking.'):
        'bucket-provenance',
    ('2026-09-15-the-whole-thing-deployed.md',
     'One client.'):
        'deployed',
    ('2026-09-15-the-whole-thing-deployed.md',
     'One writer, no replicas.'):
        'deployed',
    ('2026-09-15-the-whole-thing-deployed.md',
     'No restart.'):
        'deployed',
    ('2026-09-15-the-year-there-was-no-year-to-key-on.md',
     'No timezone handling, at all.'):
        'zones',
    ('2026-09-15-the-year-there-was-no-year-to-key-on.md',
     'No `date_trunc` to a month or a year.'):
        'date-trunc-month',
    ('2026-09-15-the-year-there-was-no-year-to-key-on.md',
     'No time functions on a join or a chain.'):
        'time-fn-join',
    ('2026-09-15-the-year-there-was-no-year-to-key-on.md',
     'The clients cannot yet *build* one.'):
        'calendar-builders',
    ('2026-09-15-three-sdks-that-can-say-it.md',
     'The Python suite was running against a stale binary and I ne'):
        'stale-binary',
    ('2026-09-15-three-sdks-that-can-say-it.md',
     'No client can express a `Chain::compute` over three or more '):
        'chain-computed-clients',
    ('2026-09-15-three-sdks-that-can-say-it.md',
     "The demo's computed value is one expression."):
        'conformance-cases',
    ('2026-09-15-three-sdks-that-can-say-it.md',
     "Go and TypeScript still cannot read an *input's* own compute"):
        'computed-clients',
    ('2026-09-15-three-sdks-that-can-say-it.md',
     "The `decade` grouping changes what the demo's chart can draw"):
        'panels-decade',
    ('2026-09-15-three-tables-in-the-front-end.md',
     'There is no alias, so there is no useful chain over the taxi'):
        'alias-sql',
    ('2026-09-15-three-tables-in-the-front-end.md',
     '`GROUP BY` on a join or chain still takes exactly one key.'):
        'group-many-join',
    ('2026-09-15-three-tables-in-the-front-end.md',
     'Nothing outside the wasm crate is touched.'):
        'workbench-chain',
    ('2026-09-15-three-types-and-none-of-them-new.md',
     'Nothing crosses the wire.'):
        'type-mapping',
    ('2026-09-15-two-checks-the-zone-change-left-behind.md',
     'Nothing checks that a changed refusal has no stale assertion'):
        'correction-guard',
    ('2026-09-16-a-batch-is-a-round-trip-a-transaction-is-a-guarantee.md',
     'No client can call it.'):
        'batch-clients',
    ('2026-09-16-a-relationship-is-a-foreign-key-read-backwards.md',
     'Go and TypeScript do not have it.'):
        'relations-wire',
    ('2026-09-16-a-relationship-is-a-foreign-key-read-backwards.md',
     'One relationship per call, one level deep.'):
        'through-wire',
    ('2026-09-16-a-subquery-is-two-reads-not-an-operator.md',
     'One level, and no depth check.'):
        'depth-limit',
    ('2026-09-16-predicate-writes-and-the-increment-that-was-not-lost.md',
     'Nothing crosses the wire.'):
        'predicate-writes-wire',
    ('2026-09-16-predicate-writes-and-the-increment-that-was-not-lost.md',
     'No `RETURNING`.'):
        'returning-wire',
    ('2026-09-16-the-site-looks-like-the-family-it-belongs-to.md',
     "The landing page's claims are not checked by anything."):
        'site-claims',
    ('2026-09-16-the-site-looks-like-the-family-it-belongs-to.md',
     '`site/style.css` still carries rules for elements the workbe'):
        'site-css',
    ('2026-09-16-two-clients-could-wait-for-ever.md',
     'No retrying `transact` in Go or TypeScript.'):
        'retry-transact',
    ('2026-09-16-what-the-other-orms-have-that-this-does-not.md',
     'The plan leaves out validations, changesets and lifecycle ho'):
        'validation-doc',
    ('2026-09-18-untrack-the-benchmarks-build-output.md',
     'Nothing prevents the fourth example doing this again.'):
        'build-output',
    ('2026-09-19-a-purge-something-can-call.md',
     'No metric.'):
        'metrics-purge',
    ('2026-09-19-a-refusal-a-form-can-render.md',
     'Go and TypeScript still parse nothing.'):
        'typed-failures',
    ('2026-09-19-decoders-over-the-servers-own-rows.md',
     'Three of five decoders per language are still only run again'):
        'generated-rows-live',
    ('2026-09-19-every-generated-decoder-runs.md',
     'Still nothing decodes a row that came from the server.'):
        'generated-rows-live',
    ('2026-09-19-every-generated-decoder-runs.md',
     'The Python decoders have no equivalent coverage check.'):
        'python-decoder',
    ('2026-09-19-forgetting-a-retired-row.md',
     'Nothing calls it.'):
        'purge-called',
    ('2026-09-19-two-ruffs-not-one.md',
     'Nothing stops this recurring.'):
        'check-sh-four',
    ('2026-09-19-who-may-see-a-deleted-row.md',
     'No client exposes it.'):
        'include-deleted-clients',
    ('2026-09-19-who-may-see-a-deleted-row.md',
     'Nothing in the demo or the conformance corpus uses it.'):
        'conformance-include-deleted',
    ('2026-09-20-a-check-for-the-mistake-made-three-times.md',
     'It covers one file.'):
        'handlers-sources',
    ('2026-09-20-a-check-for-the-mistake-made-three-times.md',
     'It does not generalise the lesson.'):
        'handlers-rosters',
    ('2026-09-20-a-child-that-is-still-a-child.md',
     'It does not close the restore gap.'):
        'restore-write',
    ('2026-09-20-a-mutation-run-that-cannot-lie.md',
     'No mutation spec is committed anywhere.'):
        'mutation-records',
    ('2026-09-20-a-mutation-run-that-cannot-lie.md',
     'It assumes the command is a Rust test run.'):
        'mutate-dialects',
    ('2026-09-20-a-mutation-run-that-cannot-lie.md',
     'It does not enforce its own use.'):
        'mutation-claims',
    ('2026-09-20-a-path-in-a-string-literal-compiles.md',
     'Four suffixes, and no guard on the list.'):
        'handlers-rosters',
    ('2026-09-20-a-view-cannot-be-a-privilege-boundary.md',
     'It builds nothing'):
        'views-toml',
    ('2026-09-20-a-view-cannot-be-a-privilege-boundary.md',
     'Nested views are not decided.'):
        'view-over-view',
    ('2026-09-20-an-array-is-a-terminator-not-a-count.md',
     'No column can be declared an array.'):
        'array-schema',
    ('2026-09-20-an-array-is-a-terminator-not-a-count.md',
     'Nothing outside the kernel knows about arrays.'):
        'array-wire',
    ('2026-09-20-an-array-on-the-wire-and-in-three-clients.md',
     'No SQL array literal.'):
        'array-literal-sql',
    ('2026-09-20-an-array-on-the-wire-and-in-three-clients.md',
     'No generated client row types.'):
        'array-codegen',
    ('2026-09-20-four-demonstrations-nobody-could-open.md',
     'It does not cover `ledger/`, `CLAUDE.md`, `README.md` or `si'):
        'provenance-readme',
    ('2026-09-20-four-more-handlers-the-exemption-vouched-for.md',
     'The corrected exemption is still a claim about six callers.'):
        'handlers-guard',
    ('2026-09-20-four-more-handlers-the-exemption-vouched-for.md',
     'Only `join` and `aggregate` are probed; their `explain` twin'):
        'security-probe',
    ('2026-09-20-four-more-handlers-the-exemption-vouched-for.md',
     'The chain handlers were assumed to be the join handlers.'):
        'security-probe',
    ('2026-09-20-one-trait-two-implementations-one-fix.md',
     'It does not probe findings 7 and 8.'):
        'security-probe',
    ('2026-09-20-one-trait-two-implementations-one-fix.md',
     'It does not check the third `Authenticator`.'):
        'handlers-rosters',
    ('2026-09-20-one-trait-two-implementations-one-fix.md',
     'Nothing stops a fourth implementation repeating this.'):
        'handlers-rosters',
    ('2026-09-20-so-a-third-authenticator-cannot-be-wrong-quietly.md',
     'Nothing does this for the third pattern instance.'):
        'handlers-rosters',
    ('2026-09-20-taking-a-catalog-was-not-the-hazard.md',
     'The criterion is a signature, and signatures are not semanti'):
        'handlers-converter',
    ('2026-09-20-the-ceilings-and-the-paths-they-do-not-reach.md',
     'Finding 8 is still only read.'):
        'security-probe',
    ('2026-09-20-the-column-count-a-stranger-could-read.md',
     'Nothing stops the next handler getting this wrong.'):
        'handlers-guard',
    ('2026-09-20-the-decisions-an-array-column-needs.md',
     'It does not check the tag space, and that is the one thing t'):
        'array-schema',
    ('2026-09-20-the-decisions-an-array-column-needs.md',
     'It settles nothing about the clients.'):
        'array-wire',
    ('2026-09-20-the-element-type-lives-on-the-column.md',
     'Nothing outside the kernel can use an array.'):
        'array-wire',
    ('2026-09-20-the-element-type-lives-on-the-column.md',
     'That TOML message is a hand-written roster of types.'):
        'array-schema',
    ('2026-09-20-the-fourth-way-a-mutation-lies.md',
     'It does not make the harness dialect-agnostic.'):
        'mutate-dialects',
    ('2026-09-20-the-frame-that-hid-two-findings.md',
     'It does not re-run the review under a wider frame.'):
        '=read',
    ('2026-09-20-the-frame-that-hid-two-findings.md',
     'The "probed and clean" section was not re-read against the s'):
        '=read',
    ('2026-09-20-the-generator-refuses-an-array-once.md',
     'It does not generate anything for an array.'):
        'array-codegen',
    ('2026-09-20-the-generator-refuses-an-array-once.md',
     'The refusal list has one entry and no guard.'):
        'array-codegen',
    ('2026-09-20-the-other-way-to-build-a-catalog.md',
     '`RecordStore` still accepts any `Catalog`.'):
        'handlers-rosters',
    ('2026-09-20-the-other-way-to-build-a-catalog.md',
     'It does not re-examine findings 2 through 8 for the same cla'):
        '=read',
    ('2026-09-20-the-write-that-names-a-key.md',
     'No client helper.'):
        'as-view-clients',
    ('2026-09-20-the-write-that-names-a-key.md',
     'The three clients have no test for this.'):
        'conformance-restore',
    ('2026-09-20-three-sdks-agree-about-a-restore.md',
     'The demo UI still has no restore.'):
        'demo-restore',
    ('2026-09-20-where-else-does-this-live.md',
     'It is a reading, not a probe, for findings 3 and 4.'):
        '=read',
    ('2026-09-20-with-is-three-questions.md',
     "The refusal's message is prose with no guard on its accuracy"):
        'cited-docs',
    ('2026-09-20-you-cannot-generate-a-catalog-from-a-hash.md',
     'It does not decide whether to persist the schema.'):
        'stored-schema',
    ('2026-09-20-you-cannot-generate-a-catalog-from-a-hash.md',
     'It does not touch the other six open rows'):
        '=read',
    ('2026-09-20-you-cannot-generate-a-catalog-from-a-hash.md',
     '`--print-schema` was not examined as a partial answer.'):
        'codegen-print-schema',
    ('2026-09-21-a-generated-view-declaration.md',
     'No client library gained a `Table.as_view(name)`.'):
        'as-view-clients',
    ('2026-09-21-a-generated-view-declaration.md',
     "The web app's `VIEWS` is still hand-written."):
        'catalog-ts',
    ('2026-09-21-a-generated-view-declaration.md',
     'A view over a view is still unreachable'):
        'view-over-view',
    ('2026-09-21-a-scan-costs-what-the-reads-before-it-left-behind.md',
     'Nothing runs this example either.'):
        'run-examples',
    ('2026-09-21-a-threshold-calibrated-against-a-reading-that-moved.md',
     'It does not make the test machine-independent, and that is n'):
        'timing-batched',
    ('2026-09-21-a-tools-docstring-is-not-a-measurement-of-the-tool.md',
     "The conformance runner's `passed` count is cases minus findi"):
        'conformance-passed',
    ('2026-09-21-a-value-per-row-over-a-partition.md',
     'Nothing outside Rust can ask for a window.'):
        'window-wire',
    ('2026-09-21-a-value-per-row-over-a-partition.md',
     'The SQL front end refuses `OVER` rather than parsing it'):
        'window-sql',
    ('2026-09-21-a-view-reported-as-a-typo.md',
     'The demo still declares no view.'):
        'head-toml-view',
    ('2026-09-21-a-view-the-demo-can-show.md',
     'The Go and Node adapters read through the view unchecked.'):
        'head-toml-view',
    ('2026-09-21-a-view-the-demo-can-show.md',
     'No client library gained a way to say "this is a view".'):
        'as-view-clients',
    ('2026-09-21-a-view-the-demo-can-show.md',
     "Nothing measures a view's cost."):
        'headbench-view',
    ('2026-09-21-a-view-you-can-declare-and-cannot-use.md',
     'No read path uses a view.'):
        'view-read-path',
    ('2026-09-21-a-view-you-can-declare-and-cannot-use.md',
     'No example configuration declares a view.'):
        'head-toml-view',
    ('2026-09-21-a-window-crosses-the-wire-in-its-own-list.md',
     'No client SDK has a window surface.'):
        'window-clients',
    ('2026-09-21-a-window-crosses-the-wire-in-its-own-list.md',
     'The workbench still refuses `OVER`.'):
        'window-sql',
    ('2026-09-21-a-window-surface-in-three-languages.md',
     'The SQL front end still refuses `OVER` by name.'):
        'window-sql',
    ('2026-09-21-a-window-surface-in-three-languages.md',
     'The three clients are not compared against each other.'):
        'conformance-window',
    ('2026-09-21-an-array-literal-in-a-predicate.md',
     'It does not generate an array column.'):
        'array-codegen',
    ('2026-09-21-an-array-literal-in-a-predicate.md',
     'No `contains`.'):
        'contains-wire',
    ('2026-09-21-generate-an-array-column-in-three-languages.md',
     'Nothing executes the generated Go or TypeScript array code.'):
        'head-toml-array',
    ('2026-09-21-one-entry-per-term.md',
     'Nothing outside the kernel knows about it.'):
        'contains-wire',
    ('2026-09-21-one-entry-per-term.md',
     'The memory store cannot measure any of this.'):
        '=measured',
    ('2026-09-21-one-read-path-knows-what-a-view-is.md',
     'No view over a view.'):
        'view-over-view',
    ('2026-09-21-one-read-path-knows-what-a-view-is.md',
     'Nothing measures the cost.'):
        'headbench-view',
    ('2026-09-21-one-read-path-knows-what-a-view-is.md',
     'The demo declares no view.'):
        'head-toml-view',
    ('2026-09-21-one-runner-for-both-crates-of-benchmarks.md',
     "`mutate.py`'s restore has a hole."):
        'mutate-marker',
    ('2026-09-21-over-in-the-query-editor.md',
     'The three SDKs and the SQL front end are still not compared '):
        'conformance-window',
    ('2026-09-21-point-read-cost-is-three-times-what-it-measures.md',
     'It does not explain why the old figures were what they were.'):
        '=measured',
    ('2026-09-21-point-read-cost-is-three-times-what-it-measures.md',
     'Nothing re-runs these benchmarks.'):
        'run-examples',
    ('2026-09-21-point-read-cost-is-three-times-what-it-measures.md',
     '`SCAN_ROW_COST` is untouched and at least one measurement of'):
        '=measured',
    ('2026-09-21-the-benchmarks-run-now-and-a-fourth-was-broken.md',
     'The exit code is the whole assertion.'):
        'run-examples',
    ('2026-09-21-the-constant-moved-and-eight-files-did-not.md',
     'It reads `crates/slate-kernel` only.'):
        'cost-prose-readme',
    ('2026-09-21-the-constant-was-right-about-a-build-that-no-longer-exists.md',
     'It does not re-derive the constant.'):
        '=measured',
    ('2026-09-21-the-constant-was-right-about-a-build-that-no-longer-exists.md',
     'Nothing records which features a measurement was taken under'):
        'build-stamp',
    ('2026-09-21-the-constant-was-right-about-a-build-that-no-longer-exists.md',
     'The guard still reads Rust only.'):
        'cost-prose-readme',
    ('2026-09-21-the-inverted-index-walks-like-any-other.md',
     'It does not settle `POINT_READ_COST`.'):
        '=measured',
    ('2026-09-21-the-inverted-index-walks-like-any-other.md',
     'Nothing runs this file, either.'):
        'run-examples',
    ('2026-09-21-what-a-view-costs-and-three-benchmarks-that-could-not-run.md',
     'Nothing runs the examples themselves, still.'):
        'run-examples',
    ('2026-09-22-a-mutation-run-now-leaves-a-trace.md',
     "Nothing checks that a ledger entry's mutation claims match a"):
        'mutation-claims',
    ('2026-09-22-a-number-that-cannot-say-what-built-it.md',
     '`docs/` prose is still unguarded.'):
        'cost-prose-docs',
    ('2026-09-22-a-number-that-cannot-say-what-built-it.md',
     "The stamp's `slatedb` version is the only dependency version"):
        'four-deps',
    ('2026-09-22-a-number-that-cannot-say-what-built-it.md',
     'Nothing checks that a *recorded* table carries the line.'):
        'table-provenance',
    ('2026-09-22-a-recorded-table-that-cannot-say-what-built-it.md',
     'Nothing currently in `docs/` is stamped.'):
        'docs-stamped',
    ('2026-09-22-a-recorded-table-that-cannot-say-what-built-it.md',
     'It reads `docs/` only.'):
        'provenance-readme',
    ('2026-09-22-eleven-findings-against-three-commits-of-mine.md',
     'Nothing checks that a recorded table kept its build line.'):
        'table-provenance',
    ('2026-09-22-four-dependencies-and-a-tool-that-was-lying.md',
     'It does not stamp the versions of anything above `slate-slat'):
        'four-deps',
    ('2026-09-22-the-page-a-reader-actually-reads.md',
     'It reads `docs/` only.'):
        'cost-prose-readme',
    ('2026-09-23-the-cliff-does-not-reproduce-and-the-knob-does-place-it.md',
     '`mutate.py` cannot read a `should_panic` failure'):
        'mutate-should-panic',
    ('2026-09-24-a-view-a-client-can-name-without-the-generator.md',
     "The web app's `VIEWS` is still hand-written."):
        'catalog-ts',
    ('2026-09-25-the-disjunction-the-kernel-always-had.md',
     'No `OR` on a join or in `HAVING`.'):
        'or-sql',
    ('2026-09-25-the-disjunction-the-kernel-always-had.md',
     'No parentheses, so no nesting.'):
        'brackets-sql',
    ('2026-09-25-the-grammar-comment-outlived-the-grammar.md',
     'It does not audit the rest of the grammar block.'):
        'grammar-cases',
    ('2026-09-25-the-grammar-comment-outlived-the-grammar.md',
     'It does not check `ORDER BY needs a GROUP BY` either.'):
        'grammar-cases',
    ('2026-09-25-the-grammar-comment-outlived-the-grammar.md',
     'No chain case.'):
        'grammar-cases',
    ('2026-09-25-the-grammar-comment-outlived-the-grammar.md',
     'The `slate-wasm` suite was not run.'):
        '=read',
    ('2026-09-25-the-landing-pages-claims-are-checked-now.md',
     'It reads one page.'):
        'site-claims',
    ('2026-09-25-the-open-caveats-nobody-re-reads.md',
     '270 open caveats are still unread.'):
        '=stamped',
    ('2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md',
     'Nothing re-triages.'):
        'checked-field',
    ('2026-09-25-what-is-left-to-do-needs-a-list-not-a-count.md',
     '`sql.rs:34` may be stale, and I still have not confirmed it.'):
        'group-many-join',
    ('2026-09-26-a-cached-go-run-is-not-a-run.md',
     'The other dialects are not audited for their own version of '):
        '=read',
    ('2026-09-26-batch-six-found-nothing.md',
     'The formatting caveat is now wrong in a way no status can ex'):
        'narrowed-verdict',
    ('2026-09-26-ordering-a-chains-groups-by-what-it-computed.md',
     "Python still cannot order a chain's groups in a test."):
        'order-chain-clients',
    ('2026-09-26-the-stylesheet-had-nothing-dead-in-it.md',
     '`examples/explorer/web` has its own stylesheet and is not ch'):
        'site-css',
    ('2026-09-26-two-ways-to-find-a-stale-caveat-that-do-not-work.md',
     '267 open caveats are still unread'):
        '=stamped',
}


def present(needle: str | None, path: str, root: Path) -> bool:
    """Does this witness hold in `root`?"""
    if needle is None:
        return (root / path).exists()
    # No `path.exists()` guard first. One was here and a mutation survived it:
    # `git grep` on a pathspec matching nothing exits 1 *quietly* — no stderr,
    # not the 128 an unreadable repository gives — so the branch only saved a
    # subprocess on the rare missing-file case and answered identically. A
    # branch that cannot change an answer is the redundant-code cause
    # `scripts/mutate.py`'s survivor message names, and the fix for that cause
    # is to delete it rather than to record an `expect_survivor`.
    found = subprocess.run(
        ["git", "grep", "-q", "-F", "--", needle, "--", path],
        cwd=root,
        capture_output=True,
    )
    return found.returncode == 0


def closed(root: Path) -> list[tuple[str, str]]:
    """`(entry, key)` for every verdict recorded `closed`."""
    path = root / "docs" / "caveat-status.json"
    if not path.is_file():
        return []
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return []
    return [
        (v["entry"], v["key"])
        for v in raw.get("verdicts", [])
        if v.get("verdict") == "closed"
    ]


def check(root: Path = ROOT) -> list[tuple[str, bool, str]]:
    """`(what, ok, detail)` per check, in the shape the other guards use."""
    out: list[tuple[str, bool, str]] = []

    def record(what: str, ok: bool, detail: str = "") -> None:
        out.append((what, ok, detail))

    verdicts = closed(root)
    # The never-fires guard every check in this directory carries. A renamed
    # verdict string, or a moved status file, finds nothing and reads exactly
    # like a repository whose every closure is witnessed.
    record(
        "some verdict is recorded closed at all",
        bool(verdicts),
        f"{len(verdicts)} closed verdicts",
    )

    unrostered = [
        f"{entry}: {key}"
        for entry, key in verdicts
        if (entry, key) not in WITNESSED
    ]
    record(
        "every closed verdict names a witness or an exemption",
        not unrostered,
        "\n      ".join(unrostered),
    )

    bad_exempt = sorted(
        {w[1:] for w in WITNESSED.values() if w.startswith("=")} - set(EXEMPT)
    )
    record("every exemption used is one EXEMPT explains", not bad_exempt, ", ".join(bad_exempt))

    unknown = sorted(
        {w for w in WITNESSED.values() if not w.startswith("=")} - set(WITNESS)
    )
    record("every witness named is one WITNESS defines", not unknown, ", ".join(unknown))

    # An orphan is a witness row whose caveat was reworded or whose verdict
    # moved off `closed`. Reported rather than dropped, for the reason
    # `caveats.py` reports an orphaned verdict: the rewording is the news.
    live = set(verdicts)
    orphans = sorted(f"{e}: {k}" for e, k in WITNESSED if (e, k) not in live)
    record("no witness row outlives its closed verdict", not orphans, "\n      ".join(orphans))

    gone = []
    checked = 0
    for (entry, key), name in sorted(WITNESSED.items()):
        # One condition, not two. `name.startswith("=") or` was here and a
        # mutation survived dropping it: an exemption's name begins `=` and is
        # never a `WITNESS` key, so the second clause already covered it. Both
        # cases this skips are reported above rather than swallowed — an
        # exemption by `bad_exempt` if `EXEMPT` does not explain it, a typo by
        # `unknown` — so skipping here is not a silent pass.
        if name not in WITNESS:
            continue
        checked += 1
        needle, path = WITNESS[name]
        if not present(needle, path, root):
            want = f"{needle!r} in {path}" if needle else path
            gone.append(f"{entry}: {key}\n        wanted {want}  ({name})")
    record(
        "every witness is still in the tree",
        not gone,
        "\n      ".join(gone) if gone else f"{checked} checked",
    )
    return out


def main() -> int:
    failed = 0
    for what, ok, detail in check():
        if ok:
            print(f"ok    {what}" + (f"  ({detail})" if detail and "\n" not in detail else ""))
        else:
            failed += 1
            print(f"FAIL  {what}" + (f"\n      {detail}" if detail else ""))
    print()
    if failed:
        print(f"{failed} failed")
        return 1
    witnessed = sum(1 for w in WITNESSED.values() if not w.startswith("="))
    print(
        f"{witnessed} of {len(WITNESSED)} closed caveats have a witness in the tree; "
        f"the other {len(WITNESSED) - witnessed} are exempt for a reason EXEMPT gives"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
