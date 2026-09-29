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

 # --- the 2026-09-13/14 backlog, triaged 2026-09-28 --------------------------
 #
 # Forty closures in one batch, because the paragraph widening in
 # `ledger/2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md` made
 # two days of caveats visible at once and most had been answered by work
 # nobody had connected back to them. Each needle was checked with `git grep`
 # before its row was written, which is the step that turned one
 # plausible-looking closure — "plan.rs costs a grouped join as grouped" —
 # into the withdrawal below, which is what actually happened.
 "explain-aggregate-in-a-transaction": ("explainAggregateIn", "clients/typescript/src/client.ts"),
 "quickstart-install-lines": ("check_install_lines", "site/check/quickstarts.py"),
 "free-ports-guard": ("ip_local_port_range", "scripts/test_free_ports.py"),
 "ceilings": ("ExecutionLimits", "crates/slate-kernel/src/limits.rs"),
 "readme-swept": ("Status", "README.md"),
 "explain-grouped": ("grouped read narrows its", "crates/slate-kernel/src/explain.rs"),
 "conformance-runner": ("conformance", "examples/explorer/run.sh"),
 "pages-deploy": (None, ".github/workflows/pages.yml"),
 "grouped-chain-wire": ("GroupedSource", "crates/slate-server/src/convert.rs"),
 "go-schemacheck": ("SchemaCheck", "clients/go/slate/schema.go"),
 "ts-renamed-column": (
     "a renamed column is accepted under its previous name",
     "clients/typescript/test/schema.test.ts",
 ),
 "go-scalar": ("Scalar", "clients/go/slate/query.go"),
 "ts-scalar": (None, "clients/typescript/src/scalar.ts"),
 "prebuilt-binary": ("SLATE_SERVERD", "clients/go/slate/harness_test.go"),
 "testserver-member": ("clients/python/testserver", "Cargo.toml"),
 "narrowed-chain": ("fn narrowed_chain", "crates/slate-kernel/src/read.rs"),
 "authorise-before-convert": (
     "before the query is converted",
     "crates/slate-server/tests/security_probe.rs",
 ),
 "demo-frontend-tests": (None, "examples/explorer/web/test/api.test.ts"),
 "demo-decade": ("decade", "examples/explorer/backends/go/handlers.go"),
 "demo-ports": ("VITE_GO_URL", "examples/explorer/web/src/api.ts"),
 "demo-grouped-plan": ("grouped", "examples/explorer/web/src/panels.tsx"),
 "grouped-cost-withdrawn": (
     "the_per_row_term_is_symmetric",
     "crates/slate-kernel/tests/grouped_chain_oracle.rs",
 ),
 "workbench-having": ("HAVING", "site/workbench.js"),
 "wasm-lazy-import": ("await import", "site/workbench.js"),
 "wasm-chain": ("chain", "crates/slate-wasm/tests"),
 "python-packaging": (None, "clients/python/tests/test_packaging.py"),
 "runner-teardown": ("kill", "examples/explorer/run.sh"),
 # --- the 2026-09-14/15/16 backlog, triaged 2026-09-28
 'wasm-in-ci': ('wasm', '.github/workflows/ci.yml'),
 'wasm-writes': ('insert', 'crates/slate-wasm/src/lib.rs'),
 # --- the 2026-09-16/17 backlog, triaged 2026-09-28
 'demo-batch': ('batch', 'examples/explorer/web/src/panels.tsx'),
 'demo-predicate-write': ('predicate', 'examples/explorer/web/src/panels.tsx'),
 'demo-relations': ('relat', 'examples/explorer/web/src/panels.tsx'),
 'conformance-relations': ('relat', 'examples/explorer/conformance/conformance.py'),
 'through-nested': ('through', 'crates/slate-server/proto/slate/v1/records.proto'),
 'ts-related': ('related', 'clients/typescript/src/client.ts'),
 'ctes-decided': (None, 'docs/ctes.md'),
 'gap-list-reread': ('gap', 'README.md'),
 # --- the 2026-09-18 backlog, triaged 2026-09-28
 'decimal-clients': (None, 'clients/go/slate/decimal_test.go'),
 'decimal-arith': ('Decimal', 'crates/slate-kernel/src/scalar.rs'),
 'metrics-endpoint': (None, 'crates/slate-serverd/src/metrics.rs'),
 'trailer-counted': ('trailer', 'crates/slate-serverd/src/observe.rs'),
 # --- the rest of the 2026-09-18 backlog, triaged 2026-09-28
 'decimal-literal-sql': ('Decimal', 'crates/slate-sql/src/sql.rs'),
 'conformance-decimal': ('decimal', 'examples/explorer/conformance/conformance.py'),
 'root-python-checks': ('ruff', 'scripts/check.sh'),
 'poll-interval-answered': ('poll', 'crates/slate-kernel/src/pool.rs'),
 'generated-row-types': (None, 'scripts/codegen.py'),
 # --- the 2026-09-19 backlog, triaged 2026-09-28
 'every-bad-field': ('violations', 'clients/typescript/src/details.ts'),
 'violation-column': ('column', 'clients/typescript/src/details.ts'),
 'conformance-shipments': ('shipments', 'examples/explorer/conformance/conformance.py'),
 'generated-decoders-run': (None, 'clients/go/slate/generated_test.go'),
 'soft-delete-restore': ('restore', 'crates/slate-kernel/src/record.rs'),
 'purge-counted': ('purge', 'crates/slate-serverd/src/observe.rs'),
 'published-checks': ('column', 'crates/slate-kernel/src/security.rs'),
 'must-differ-pairs': ('MUST_DIFFER', 'examples/explorer/conformance/conformance.py'),
 # --- the 2026-09-20/21 backlog, triaged 2026-09-28
 'four-paths-probed': ('retired', 'ledger/2026-09-20-four-say-absent-one-says-present.md'),
 # --- the last of the invisible backlog, triaged 2026-09-28
 'check-sh-venv': ('venv', 'scripts/check.sh'),
 'latency-quantiles': ('quantile', 'crates/slate-serverd/src/observe.rs'),
 'request-id': ('request_id', 'crates/slate-serverd/src/observe.rs'),
 'views-toml-declared': ('views', 'crates/slate-serverd/src/config.rs'),
 'sql-own-tests': (None, 'crates/slate-sql/tests/front_end.rs'),
 'sql-lowering': (None, 'crates/slate-sql/src/lower.rs'),
 'views-doc': (None, 'docs/views.md'),
 'site-html-citations': ('.html', 'scripts/check_cited_docs.py'),
 # --- the scale hole, closed on 2026-09-18 and noticed on the 28th
 'scale-in-fingerprint': ('column.scale()', 'crates/slate-server/src/fingerprint.rs'),
 "alias-sql": ("alias", "crates/slate-sql/tests/front_end.rs"),
 "having-brackets": ("having_predicate", "crates/slate-sql/src/lib.rs"),
 "full-join-unmatched": ("BothUnmatchedSidesOfAFullJoin", "clients/go/slate/scalar_test.go"),
 "array-in-the-ui": ("array", "examples/explorer/backends/go/seed.go"),
 "sole-writer-refusal": ("SoleWriterNotEstablished", "crates/slate-kernel/src/migrate.rs"),
 "bad-section-run": ("refuses()", "scripts/run_examples.sh"),
 "ambiguity": ("ambigu", "crates/slate-sql/src"),
 "array-codegen": ("element", "scripts/codegen.py"),
 "dialect-corpus": ("CORPUS", "scripts/test_mutate.py"),
 "node-wrapper-counted": ("will not load is counted", "scripts/test_mutate.py"),
 "marker-deserved": ("def undeserved", "scripts/check_cost_prose.py"),
 "struck-span-flag": ("inside: bool = False", "scripts/check_cost_prose.py"),
 "sources-complete": ("def unscanned", "scripts/check_handlers.py"),
 "secret-roster": ("def holders", "scripts/check_secret_types.py"),
 "argv-roster": ("READS_ARGS", "scripts/check_examples_roster.py"),
 "prose-citations": ("def prose_files", "scripts/check_cited_docs.py"),
 "minio-image": ("chainguard/minio", ".github/workflows/ci.yml"),
 "fixtures-alive": ("FIXTURES exempts", "scripts/check_cited_docs.py"),
 "runner-continue": ("ASKS_HANDSHAKE", "scripts/check_examples_roster.py"),
 "tracker-in-ci": ("scripts/caveats.py", ".github/workflows/ci.yml"),
 "headbench-smoke": ("crates/slate-headbench/run.sh --smoke", ".github/workflows/ci.yml"),
 "write-timed": ("a_write_reports_what_the_index_maintenance_cost", "crates/slate-wasm/tests/timing.rs"),
 "element-spelling": ("def element_spelling", "scripts/codegen.py"),
 "client-five": ("Windows, arrays, full-text, views and soft delete", "site/docs/clients.html"),
 "docs-in-a-browser": (None, "site/check/pages.py"),
 "silent-case": ("def silent_cases", "examples/explorer/conformance/conformance.py"),
 "restore-report": ("def restore_report", "scripts/mutate.py"),
 "purge-grants": ("a_purge_needs_all_three_grants_and_says_which_is_missing", "crates/slate-kernel/tests/soft_delete.rs"),
 "outer-computed": ("a_chains_computed_value_over_an_unmatched_step_reads_nulls", "crates/slate-kernel/tests/chain.rs"),
 "stops-at-reread": ("**Superseded by N1.**", "docs/orm-comparison.md"),
 "arrays-answered": ("Left open here, and answered by the build", "docs/arrays.md"),
 "views-answered": ("Left open here; one answered, one still open", "docs/views.md"),
 "sole-missing-table": ("NAMES_A_MISSING_TABLE", "scripts/check_handlers.py"),
 "converter-callers": ("a handler converting before authorising fails", "scripts/test_check_handlers.py"),
 "convert-audited": ("pub fn aggregate_from_proto_query", "crates/slate-server/src/convert.rs"),
 "cte-docs-updated": ("built, and it was the views row", "docs/ctes.md"),
 "cte-repeated-column": ("a_repeated_column_in_a_cte_projects_it_twice_just_as_it_does_directly", "crates/slate-sql/tests/front_end.rs"),
 "unmatched-go": ("TestAnInputsComputedValueIsNilOnAnUnmatchedOuterSide", "clients/go/slate/scalar_test.go"),
 "unmatched-ts": ("undefined on an unmatched outer side", "clients/typescript/test/scalar.test.ts"),
 "ci-conclusion": ("Read the run's conclusion", "CLAUDE.md"),
 "site-cost-prose": ("site.glob", "scripts/check_cost_prose.py"),
 "node24-actions": ("actions/checkout@v5", ".github/workflows/ci.yml"),
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
 "residual-field": ("RESIDUAL", "scripts/caveats.py"),
 "reviewed-field": ("REVIEWED", "scripts/caveats.py"),
 "exempt-because": ("EXEMPT_BECAUSE", "scripts/check_closed_caveats.py"),
 "conformance-restore-live": ("api/restore", "examples/explorer/conformance/conformance.py"),
 "demo-restore-panel": ("restore-run", "examples/explorer/web/src/panels.tsx"),
 "reverse-sweep": (None, "ledger/2026-09-26-the-reverse-sweep-found-six.md"),
 "closed-audited": (None, "ledger/2026-09-26-the-closed-verdicts-audited.md"),
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
 "zones-dst": ("tzdata", "crates/slate-kernel/src/zones.rs"),

 # --- round trips counted in all three clients, 2026-09-28 -------------------
 #
 # Each needle is the instrument rather than the assertion: an interceptor
 # installed on the client's own channel. A closure reverted by deleting the
 # test takes the file with it; one reverted by loosening the assertion leaves
 # the needle, which is the coarseness argued for above.
 "ts-round-trips": (
     "interceptors",
     "clients/typescript/test/roundTrip.test.ts",
 ),
 "ts-channel-options": ("options", "clients/typescript/src/client.ts"),
 "go-paging-count": (
     "WithChainStreamInterceptor",
     "clients/go/slate/related_test.go",
 ),
 "retired-row-only-predicate": (
     "a_predicate_that_selects_only_the_retired_row_touches_nothing",
     "crates/slate-kernel/tests/soft_delete.rs",
 ),

 # --- the grouped round-trip claims, all measured 2026-09-28 ----------------
 #
 # `WITNESSED` holds one name per closure, and both closures below are of a
 # caveat that grouped several claims — four in one case, two in the other.
 # So each names the *first* of its group and the rest ride on it, which is a
 # real weakness of the roster and is stated rather than papered over: a
 # revert that deleted only `clients/typescript/test/roundTrip.test.ts` would
 # leave `many-to-many-two-reads` present and this guard quiet. What saves it
 # here is that the other three are each witnessed in their own right by the
 # closures of the caveats they came from — `ts-round-trips`,
 # `ts-channel-options` and `go-paging-count`, all above — so the roster does
 # cover them, just not through this row.
 "many-to-many-two-reads": (
     "a_many_to_many_is_two_reads_whatever_the_parent_count",
     "crates/slate-orm/tests/read_counts.rs",
 ),
 "py-round-trips": ("UnaryStreamClientInterceptor", "clients/python/tests/test_round_trips.py"),
}

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
    "deleted": (
        "closed by deleting the thing the caveat was about. A removal leaves no "
        "artifact by construction — the only witness would be a file asserting "
        "that another file is gone, which goes stale the moment somebody writes "
        "a third file with the same name"
    ),
    "tagged": (
        "closed by a git tag, which is a ref rather than a file. `v0.0.1` is on "
        "the remote — `git ls-remote --tags origin` shows it — and the working "
        "tree carries the workflow that reacts to a tag, not the fact that one "
        "was pushed. A file asserting the tag exists would be a second copy of it"
    ),
    "stamped": (
        "closed by the state of `docs/caveat-status.json` itself — a count of "
        "caveats in some state, answered by the file reaching a different "
        "count. Unread ones are answered by every open verdict carrying a "
        "`checked` date and untriaged ones by every caveat carrying a verdict "
        "at all; `caveats.py --unread` and `caveats.py` are the two checks, "
        "and `check.sh` runs both. A witness pointing at the file this guard "
        "already parses would be circular"
    ),
}

#: Why each exempt closure leaves nothing in the tree — one sentence per
#: caveat, not one per kind.
#:
#: `EXEMPT` began as three kinds covering sixteen rows, and
#: `ledger/2026-09-26-a-witness-for-every-closure.md` recorded what that costs:
#: "a closure filed under `=read` because writing a witness was hard looks
#: exactly like one filed there because no witness exists." A kind is a
#: category; a reason is an argument, and only the second can be wrong in a way
#: a reader can see.
EXEMPT_BECAUSE: dict[tuple[str, str], str] = {
    # The same-day census, 2026-09-29. Six of these say "the rest of the open
    # caveats are unread", which is answered by every open verdict now
    # carrying a `checked` date from one of the two census passes — a state of
    # `docs/caveat-status.json`, which is the file this guard already parses.
    ('2026-09-28-the-scale-hole-was-closed-ten-days-ago.md',
     'It does not re-read the other 113 open caveats against the t'):
        'closed by the two census passes, which between them read every open caveat against the tree. The artifact is a `checked` date on every open verdict in `docs/caveat-status.json`; a witness pointing at the file this guard parses would be circular',
    ('2026-09-28-two-limits-that-are-weaker-than-their-names.md',
     'It does not re-read the rest of the open caveats.'):
        'closed by the two census passes. Same artifact as the row above: the reading leaves a date per verdict and nothing else, because a caveat that survives a reading changes no file',
    ('2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md',
     'It reads eleven of 116, and changes four.'):
        'closed by the two census passes reading the other 105. Same artifact: a `checked` date per open verdict',
    ('2026-09-28-twenty-five-caveats-read-and-eight-greps-that-lied.md',
     'It does not examine the other ninety-one.'):
        'closed by the two census passes reading the other ninety-one. Same artifact: a `checked` date per open verdict',
    ('2026-09-28-fifty-one-of-ninety-nine-read-at-random.md',
     'It leaves forty-eight of the ninety-nine unread'):
        'closed by the second census pass reading exactly those forty-eight. Same artifact: a `checked` date per open verdict',
    ('2026-09-28-a-random-twelve-found-nothing.md',
     'It reads twelve, so the interval is wide.'):
        'closed by widening the sample to all ninety-nine, which is a count of caveats read and leaves no file of its own. The interval it produces is prose in the census entry',
    ('2026-09-28-a-random-twelve-found-nothing.md',
     'It says nothing about the `deliberate` verdicts.'):
        'closed by drawing the first sample from the `deliberate` population and reading it. Fifteen verdicts read, one found reversed; the reading is the artifact and the one reversal is a single field in the tracker, which a witness pointing at the tracker could not distinguish from any other edit',
    ('2026-09-28-fifty-one-of-ninety-nine-read-at-random.md',
     'It still says nothing about the 890 `deliberate` verdicts.'):
        'closed by drawing the first sample from the `deliberate` population and reading it. Fifteen verdicts read, one found reversed; the reading is the artifact and the one reversal is a single field in the tracker, which a witness pointing at the tracker could not distinguish from any other edit',
    ('2026-09-14-the-testserver-joins-the-workspace.md',
     '`scripts/build_testserver.sh` is now unnecessary — it existe'):
        'closed by deleting `scripts/build_testserver.sh`. `git grep build_testserver` now finds only this entry\'s own sentence, and the entry records the deletion inline',
    ('2026-09-14-the-settings-landed.md',
     'The release upload is still the one thing in this repository'):
        'closed by pushing `v0.0.1`, the tag `softprops/action-gh-release` waits for. A git ref is not a file, so the tree carries the workflow that reacts to a tag and not the fact that one was pushed',
    ('2026-09-14-ci-that-had-never-run.md',
     'What follows was true until that tag, and is left because it'):
        'closed by pushing `v0.0.1`, which is a git tag and not a file. `git ls-remote --tags origin` shows it; nothing in the working tree records that the release workflow has now run, and adding a file that said so would be a second copy of the tag',
    ('2026-09-27-five-narrowed-caveats-were-invisible-to-the-staleness-report.md',
     'Three residuals are now on the worklist and have not been re'):
        'closed by re-reading the three residuals against the tree. Two still held '
        'unchanged and the third had gone stale by a count, so the only artifact is '
        'a corrected citation inside the tracker itself',
    ('2026-09-14-a-real-dataset.md',
     'The 1,290 bytes a row was measured and not investigated.'):
        'closed by a measurement that found where the bytes go. The finding is '
        'the entry; the tree carries the cut that followed, not the investigation',
    ('2026-09-15-a-value-belonging-to-the-join.md',
     'The extra flatten on the grouped path is unmeasured.'):
        'closed by measuring it and finding the flatten too small to see. A null '
        'measurement changes no file by construction',
    ('2026-09-20-the-frame-that-hid-two-findings.md',
     'It does not re-run the review under a wider frame.'):
        'closed by re-running the review. A review produces findings, and the '
        'findings that had code became commits with their own caveats',
    ('2026-09-20-the-frame-that-hid-two-findings.md',
     'The "probed and clean" section was not re-read against the s'):
        'closed by re-reading that section under the wider frame. It came back '
        'clean, which is prose and nothing else',
    ('2026-09-20-the-other-way-to-build-a-catalog.md',
     'It does not re-examine findings 2 through 8 for the same cla'):
        'closed by the re-examination, which found the reach-around class in '
        'two places; both fixes have their own witnessed caveats',
    ('2026-09-20-where-else-does-this-live.md',
     'It is a reading, not a probe, for findings 3 and 4.'):
        'closed by probing them. The probes are tests in security_probe.rs and '
        'are witnessed by the caveats that asked for them; this row is the '
        'reading that preceded them',
    ('2026-09-20-you-cannot-generate-a-catalog-from-a-hash.md',
     'It does not touch the other six open rows'):
        'closed by reading each of the six for its real shape. The output is '
        'six corrected gap-table rows, which are prose in a table',
    ('2026-09-21-one-entry-per-term.md',
     'The memory store cannot measure any of this.'):
        'closed by measuring it somewhere that can — the inverted-index walk on '
        'a loopback S3 server. The numbers are in the entry',
    ('2026-09-21-point-read-cost-is-three-times-what-it-measures.md',
     'It does not explain why the old figures were what they were.'):
        'closed by the explanation: the block cache was compiled out. That is a '
        'finding about a build that no longer exists, so there is nothing in '
        'this tree for it to be',
    ('2026-09-21-point-read-cost-is-three-times-what-it-measures.md',
     '`SCAN_ROW_COST` is untouched and at least one measurement of'):
        'closed by resolving the cold-full-scan disagreement, which left '
        'SCAN_ROW_COST where it was. A measurement that moves no constant '
        'changes no file',
    ('2026-09-21-the-constant-was-right-about-a-build-that-no-longer-exists.md',
     'It does not re-derive the constant.'):
        'closed by re-deriving it at release with the cache on, and finding the '
        'shipped value right. The constant did not move',
    ('2026-09-21-the-inverted-index-walks-like-any-other.md',
     'It does not settle `POINT_READ_COST`.'):
        'closed by the re-measurement that settled it. Same null result: the '
        'value stands, so no line changed',
    ('2026-09-25-the-grammar-comment-outlived-the-grammar.md',
     'The `slate-wasm` suite was not run.'):
        'closed by running it — 226 passed across 14 files. A run leaves a '
        'transcript and not a file',
    ('2026-09-25-the-open-caveats-nobody-re-reads.md',
     '270 open caveats are still unread.'):
        'closed by reading them. The artifact is docs/caveat-status.json '
        'itself, which check_caveat_citations.py and caveats.py --unread both '
        'read; a witness pointing at the file this guard already parses would '
        'be circular',
    ('2026-09-26-a-cached-go-run-is-not-a-run.md',
     'The other dialects are not audited for their own version of '):
        'closed by auditing them and finding go the only one with a result '
        'cache. Four negative findings, which are the entry',
    ('2026-09-26-two-ways-to-find-a-stale-caveat-that-do-not-work.md',
     '267 open caveats are still unread'):
        'same as the row above: closed by the reading pass, whose artifact is '
        'the status file this guard parses',
    ('2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md',
     'The 468 are untriaged, and that is the work this surfaced ra'):
        'closed by triaging all 468 over four commits on the same day. The '
        'artifact is docs/caveat-status.json, which this guard already parses; '
        'a witness pointing at it would be circular',
    ('2026-09-28-the-first-two-days-of-the-invisible-backlog.md',
     '364 remain, and they are the 2026-09-15 to 2026-09-27 entrie'):
        'same as the row above: a progress marker in a batch, retired by the '
        'batch after it. The count it names is a state of the status file',
    ('2026-09-28-the-invisible-backlog-part-two.md', '283 remain'):
        'same as the row above: a progress marker retired by the next batch',
    ('2026-09-28-the-invisible-backlog-part-three.md', '130 remain'):
        'same as the row above, and the last of the four. A caveat that counts '
        'how much of itself is left is true only until the next commit, which '
        'is an argument for writing fewer of them rather than for a witness',
    ('2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md',
     'It does not check whether other entries carry the withdrawn '):
        'closed by reading the four entries and their commit timestamps against '
        "35d9182's. All four predate the fix, so each was true when written and "
        'nothing needed striking. A null result changes no file by construction',
}

#: Every `closed` caveat, and the witness that must still be in the tree.
#:
#: Keyed the way `caveats.py` keys a verdict — entry filename and the first 60
#: characters of the bullet — so a reworded caveat orphans its row here for the
#: same reason it orphans its verdict, and is read again rather than silently
#: keeping a witness chosen for a different claim.
WITNESSED: dict[tuple[str, str], str] = {
    # A `deliberate` whose decision the next day's work reversed, found by the
    # first random sample ever drawn from that verdict.
    ('2026-09-27-the-guard-for-two-copies-could-not-read-the-second-one.md',
     '`.html` was added to one guard, and the sweep for others fou'):
        'site-html-citations',
    ('2026-09-14-ports-below-the-ephemeral-range.md',
     'Nothing tests the allocator itself. There is no case asserti'):
        'free-ports-guard',
    ('2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md',
     'It does not check whether other entries carry the withdrawn '): '=read',
    # --- two open verdicts re-read against the tree on 2026-09-28 and found false
    ('2026-09-14-explaining-a-grouped-read.md',
     'No client exposes `ExplainAggregate` inside a transaction ex'):
        'explain-aggregate-in-a-transaction',
    ('2026-09-14-self-checking-quickstarts-and-conformance.md',
     'The quickstart check does not verify the *prose* around the '):
        'quickstart-install-lines',
    # --- four progress markers, each retired by the batch after it
    ('2026-09-28-the-tracker-could-not-see-a-third-of-the-caveats.md',
     'The 468 are untriaged, and that is the work this surfaced ra'): '=stamped',
    ('2026-09-28-the-first-two-days-of-the-invisible-backlog.md',
     '364 remain, and they are the 2026-09-15 to 2026-09-27 entrie'): '=stamped',
    ('2026-09-28-the-invisible-backlog-part-two.md', '283 remain'): '=stamped',
    ('2026-09-28-the-invisible-backlog-part-three.md', '130 remain'): '=stamped',
    # --- the scale hole, closed on 2026-09-18 and noticed on the 28th
    ('2026-09-18-arithmetic-over-money-and-the-expressions-that-are-refused.md',
     "Nothing checks a client's declared scale against the server'"):
        'scale-in-fingerprint',
    ('2026-09-18-decimals-and-conditional-updates-in-three-clients.md',
     "Nothing checks a client's declared scale against the server'"):
        'scale-in-fingerprint',
    ('2026-09-18-nineteen-ninety-nine-is-a-decimal-not-a-float.md',
     "Nothing checks a client's idea of a column's scale against t"):
        'scale-in-fingerprint',
    ('2026-09-18-the-three-sdks-compared-on-a-decimal.md',
     "Nothing here checks a client's declared scale against the se"):
        'scale-in-fingerprint',
    ('2026-09-18-three-clients-and-the-integer-they-would-all-have-reached-for.md',
     'No client-side knowledge of scale.'):
        'scale-in-fingerprint',
    # --- the last of the invisible backlog, triaged 2026-09-28
    ('2026-09-18-ty-resolves-against-whatever-you-happen-to-have.md',
     'It does not make the two environments agree.'):
        'check-sh-venv',
    ('2026-09-18-what-the-node-says-about-its-requests.md',
     'No histogram, so no percentiles.'):
        'latency-quantiles',
    ('2026-09-18-what-the-node-says-about-its-requests.md',
     'A failure raised in a trailer counts as a success.'):
        'trailer-counted',
    ('2026-09-18-what-the-node-says-about-its-requests.md',
     'No request id.'):
        'request-id',
    ('2026-09-21-refusing-a-view-everywhere-else-is-free.md',
     'It builds no view.'):
        'views-toml-declared',
    ('2026-09-21-slate-sql-gets-its-own-tests.md',
     "`Schema<'_>` is the only way in, and it takes a slice of `Ta"):
        'views-toml-declared',
    ('2026-09-21-slate-sql-is-its-own-crate.md',
     'The lowering has not moved.'):
        'sql-lowering',
    ('2026-09-21-slate-sql-is-its-own-crate.md',
     'No view exists yet'):
        'views-toml-declared',
    ('2026-09-21-slate-sql-is-its-own-crate.md',
     '`slate-sql` has no tests of its own.'):
        'sql-own-tests',
    ('2026-09-21-the-lowering-follows-the-parser.md',
     'No view exists yet.'):
        'views-toml-declared',
    ('2026-09-21-the-lowering-follows-the-parser.md',
     '`slate-sql` still has no tests of its own.'):
        'sql-own-tests',
    ('2026-09-21-the-views-blocker-was-a-crate-name.md',
     'It builds no view.'):
        'views-toml-declared',
    ('2026-09-21-the-views-blocker-was-a-crate-name.md',
     'It does not prove the extraction is clean.'):
        'sql-lowering',
    ('2026-09-21-the-views-blocker-was-a-crate-name.md',
     'It leaves the other two open questions open'):
        'views-doc',
    ('2026-09-22-four-dependencies-and-a-tool-that-was-lying.md',
     'What survives the audit is narrower and worth more: **a muta'):
        'mutation-records',
    ('2026-09-27-the-guard-for-two-copies-could-not-read-the-second-one.md',
     'What the grep cannot say is whether a guard *should* read th'):
        'site-html-citations',
    # --- the 2026-09-20/21 backlog, triaged 2026-09-28
    ('2026-09-20-the-accessor-three-adapters-now-call.md',
     'Still no `restore`.'):
        'soft-delete-restore',
    ('2026-09-20-the-attribute-the-builder-already-had.md',
     'No `restore`.'):
        'soft-delete-restore',
    ('2026-09-20-the-column-the-catalog-knew-about.md',
     'There is still no `restore`.'):
        'soft-delete-restore',
    ('2026-09-20-the-row-that-is-both-there-and-not.md',
     'It does not check the other write paths.'):
        'four-paths-probed',
    # --- the 2026-09-19 backlog, triaged 2026-09-28
    ('2026-09-19-a-check-that-names-its-field.md',
     'The write path still stops at the first failing check, so a '):
        'every-bad-field',
    ('2026-09-19-a-check-that-names-its-field.md',
     'No client reads the new metadata. Python, Go and TypeScript '):
        'violation-column',
    ('2026-09-19-a-real-enum-and-a-real-soft-delete.md',
     'Nothing **reads** the new table yet. No conformance case que'):
        'conformance-shipments',
    ('2026-09-19-a-real-enum-and-a-real-soft-delete.md',
     'The retired row is invisible to every current caller, becaus'):
        'conformance-include-deleted',
    ('2026-09-19-a-real-enum-and-a-real-soft-delete.md',
     "No test asserts the demo's *generated* schema module imports"):
        'generated-decoders-run',
    ('2026-09-19-a-row-that-is-gone-but-still-there.md',
     'There is no `restore` and no reaper. Un-deleting is an ordin'):
        'soft-delete-restore',
    ('2026-09-19-a-row-with-names-on-it.md',
     'The Go and TypeScript decoders are **compiled but not execut'):
        'generated-decoders-run',
    ('2026-09-19-asking-for-the-rows-that-are-gone.md',
     '`MUST_DIFFER` holds one pair. Several other flags have the s'):
        'must-differ-pairs',
    ('2026-09-19-asking-for-the-rows-that-are-gone.md',
     'Nothing tests `include_deleted` on a *join* input, which is '):
        'conformance-include-deleted',
    ('2026-09-19-every-bad-field-at-once.md',
     'No client surfaces the set. All three SDKs show the status m'):
        'every-bad-field',
    ('2026-09-19-every-bad-field-at-once.md',
     'The third recommendation — publishing checks so a client can'):
        'published-checks',
    ('2026-09-19-forgetting-a-retired-row.md',
     "No metric or log line counts purged rows, so a sweep's effec"):
        'purge-counted',
    ('2026-09-19-the-regex-hole-that-was-not-there.md',
     'It does not build any of the three remaining recommendations'):
        'published-checks',
    ('2026-09-19-where-validation-lives.md',
     'It does not build any of the four things it recommends, and '):
        'published-checks',
    ('2026-09-19-the-column-the-caller-cannot-set.md',
     'No soft delete, and no other hooks.'):
        'soft-delete-restore',
    # --- the rest of the 2026-09-18 backlog, triaged 2026-09-28
    ('2026-09-18-arithmetic-over-money-and-the-expressions-that-are-refused.md',
     'The SQL front end still parses `19.99` as a float.'):
        'decimal-literal-sql',
    ('2026-09-18-decimals-and-conditional-updates-in-three-clients.md',
     'No decimal arithmetic.'):
        'decimal-arith',
    ('2026-09-18-decimals-and-conditional-updates-in-three-clients.md',
     'No three-SDK conformance case yet.'):
        'conformance-decimal',
    ('2026-09-18-pin-the-deployed-run-to-one-snapshot.md',
     'It does not resolve the poll-interval discrepancy above'):
        'poll-interval-answered',
    ('2026-09-18-swap-mypy-for-ty-astrals-checker.md',
     'It does not touch the other Python in this repository.'):
        'root-python-checks',
    ('2026-09-18-the-catalog-writes-the-declaration-nobody-should-type.md',
     'It generates a declaration, not a row type.'):
        'generated-row-types',
    ('2026-09-18-the-docs-catch-up-with-two-shipped-items.md',
     "The README's closed-items list was not audited."):
        'gap-list-reread',
    ('2026-09-18-the-failure-that-arrives-after-the-answer-has-started.md',
     '`late` is cumulative, like everything else on the line.'):
        'metrics-endpoint',
    ('2026-09-18-the-python-checks-that-were-installed-and-never-run.md',
     'It does not check the other Python in this repository.'):
        'root-python-checks',
    # --- the 2026-09-18 backlog, triaged 2026-09-28
    ('2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md',
     'No client speaks either of these yet.'):
        'decimal-clients',
    ('2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md',
     'No decimal arithmetic anywhere.'):
        'decimal-arith',
    ('2026-09-18-a-decimal-and-a-conditional-update-on-the-wire.md',
     '`delete_if_unchanged` does not exist'):
        'delete-if-unchanged',
    ('2026-09-18-a-mean-and-a-maximum-do-not-describe-a-latency.md',
     "The quantiles are over the process's whole life, not the las"):
        'metrics-endpoint',
    ('2026-09-18-a-mean-and-a-maximum-do-not-describe-a-latency.md',
     'A failure raised in a trailer still counts as a success'):
        'trailer-counted',
    # --- the 2026-09-16/17 backlog, triaged 2026-09-28
    ('2026-09-16-batch-in-three-clients-and-a-token-that-only-survives-batched.md',
     'The demo frontend still has no batch button, like predicate '):
        'demo-batch',
    ('2026-09-16-chains-were-never-missing-from-the-wire.md',
     "The chain cases do not cover a chain's *computed* value or a"):
        'conformance-runner',
    ('2026-09-16-chains-were-never-missing-from-the-wire.md',
     'The other `grep`-based rows in that table have not been re-c'):
        'gap-list-reread',
    ('2026-09-16-many-to-many-is-a-composition-not-a-third-relationship.md',
     'No client has it. `load_related_through` is a record-layer f'):
        'through-nested',
    ('2026-09-16-many-to-many-is-a-composition-not-a-third-relationship.md',
     'Nesting is not addressed: `load_related_through` goes one ho'):
        'through-nested',
    ('2026-09-16-nested-loading-and-a-depth-limit-with-nothing-to-limit.md',
     'Still record-layer only. No client can nest, for the same re'):
        'through-nested',
    ('2026-09-16-predicate-writes-in-three-clients-and-a-guard-that-never-ran.md',
     'The demo frontend does not show predicate writes. The adapte'):
        'demo-predicate-write',
    ('2026-09-16-predicate-writes-on-the-wire-and-eight-errors-that-read-as-a-crash.md',
     'No client has these yet. The RPCs and the wire tests exist; '):
        'conformance-runner',
    ('2026-09-16-relations-in-typescript-and-a-proto-copy-that-had-drifted.md',
     'The conformance runner still does not exercise relations. Al'):
        'conformance-relations',
    ('2026-09-16-the-go-relation-client-and-a-comment-that-cost-eight-minutes.md',
     'No TypeScript `related` yet, so the conformance runner still'):
        'ts-related',
    ('2026-09-16-the-three-sdks-compared-on-relations-and-a-refusal-that-was-not-one.md',
     "The demo's UI has no relationship view. The endpoint exists "):
        'demo-relations',
    ('2026-09-16-what-the-other-orms-have-that-this-does-not.md',
     'Window functions, CTEs, views, arrays, full-text search and '):
        'ctes-decided',
    # --- the 2026-09-14/15/16 backlog, triaged 2026-09-28
    ('2026-09-14-the-kernel-in-a-browser.md',
     'No joins, aggregates or grouped reads through the binding ye'):
        'wasm-chain',
    ('2026-09-14-the-kernel-in-a-browser.md',
     'Writes are not exposed. The store is seeded and then read; a'):
        'wasm-writes',
    ('2026-09-14-the-kernel-in-a-browser.md',
     '596 KiB gzipped is not free, and nothing lazy-loads it yet.'):
        'wasm-lazy-import',
    ('2026-09-14-the-kernel-in-a-browser.md',
     'The wasm build is not in CI as of this commit, so nothing st'):
        'wasm-in-ci',
    ('2026-09-14-the-kernel-on-wasm.md',
     'The wasm target is not in CI as of this commit, so nothing s'):
        'wasm-in-ci',
    ('2026-09-14-the-settings-landed.md',
     'The release upload is still the one thing in this repository'):
        '=tagged',
    ('2026-09-14-the-testserver-joins-the-workspace.md',
     '`scripts/build_testserver.sh` is now unnecessary — it existe'):
        '=deleted',
    ('2026-09-14-writes-in-the-playground.md',
     'The panel does not expose any of this yet — this commit is t'):
        'workbench-having',
    ('2026-09-15-money-that-does-not-drift.md',
     'This is now the third feature on this branch wanting the sam'):
        'decimal-wire',
    ('2026-09-16-a-durability-check-that-cried-data-loss.md',
     "The deployed harness's *other* unfreshened reads are untouch"):
        'deployed',
    # --- the 2026-09-13/14 backlog, triaged 2026-09-28 ----------------------
    ('2026-09-13-a-grouped-chain-on-the-wire.md',
     '`EXPLAIN` still cannot describe a grouped chain, or a groupe'):
        'explain-grouped',
    ('2026-09-13-a-grouped-chain-on-the-wire.md',
     'The three clients can now *reach* a grouped chain, and only '):
        'conformance-runner',
    ('2026-09-13-a-site.md',
     'No deployment. The site is files in a directory; nothing pub'):
        'pages-deploy',
    ('2026-09-13-defects-found-by-review.md',
     'Two security findings stay open and neither is a patch: the '):
        'authorise-before-convert',
    ('2026-09-13-go-client.md',
     '`Join`, `Aggregate` and `ExplainJoin` are on the wire and ha'):
        'go-schemacheck',
    ('2026-09-13-go-client.md',
     'Nothing here is a differential *against the Python client*. '):
        'conformance-runner',
    ('2026-09-13-go-client.md',
     'The tests need `cargo` on the path and build the daemon, so '):
        'prebuilt-binary',
    ('2026-09-13-go-joins-and-aggregates.md',
     'No computed values in a query or join input, no vector simil'):
        'go-scalar',
    ('2026-09-13-go-joins-and-aggregates.md',
     "Nothing here compares the Go client's answers against the Py"):
        'conformance-runner',
    ('2026-09-13-grouped-joins-on-the-wire.md',
     'Grouping a chain is not built, in the kernel or here. A grou'):
        'grouped-cost-withdrawn',
    ('2026-09-13-grouped-joins-on-the-wire.md',
     '`aggregate_to_proto_query` still emits the single-table shap'):
        'workbench-having',
    ('2026-09-13-grouping-a-chain.md',
     'A grouped chain reads wider than it needs to, as above. It i'):
        'narrowed-chain',
    ('2026-09-13-grouping-a-chain.md',
     'There is no `EXPLAIN` for a grouped chain, or for a grouped '):
        'explain-grouped',
    ('2026-09-13-grouping-a-chain.md',
     '`group_by_chain` is not on the wire. The server refuses a ch'):
        'grouped-chain-wire',
    ('2026-09-13-in-list-per-row-cost.md',
     "Three of finding 7's four items are untouched here: `GROUP B"):
        'ceilings',
    ('2026-09-13-point-the-readme-at-everything-new.md',
     "The README's Status section still describes the project as o"):
        'readme-swept',
    ('2026-09-13-python-grouped-join-and-a-rotted-testserver.md',
     '`testserver` stays outside the root workspace, so nothing st'):
        'testserver-member',
    ('2026-09-13-python-grouped-join-and-a-rotted-testserver.md',
     'Grouping a chain is still unbuilt in the kernel; the client '):
        'grouped-chain-wire',
    ('2026-09-13-security-findings-4-and-8.md',
     'Only the four fingerprint-checking handlers authorise early.'):
        'authorise-before-convert',
    ('2026-09-13-the-explorer-and-what-it-found.md',
     'No frontend yet — this is the backend and the contract.'):
        'demo-frontend-tests',
    ('2026-09-13-the-explorer-and-what-it-found.md',
     '`groupBy: "decade"` is a documented refusal rather than a fe'):
        'demo-decade',
    ('2026-09-13-the-explorers-frontend.md',
     'No tests. The panels are checked by having been driven in a '):
        'demo-frontend-tests',
    ('2026-09-13-the-explorers-frontend.md',
     'No `groupBy: "decade"` — the UI offers it and the adapters r'):
        'demo-decade',
    ('2026-09-13-the-explorers-frontend.md',
     'The frontend talks to three hard-coded localhost ports. Fine'):
        'demo-ports',
    ('2026-09-13-typescript-client.md',
     '`Join`, `Aggregate` and `ExplainJoin` have no typed surface,'):
        'ts-scalar',
    ('2026-09-13-typescript-client.md',
     'Nothing compares the three clients against *each other*. All'):
        'conformance-runner',
    ('2026-09-13-typescript-joins-and-aggregates.md',
     'No computed values, vectors or `SchemaCheck`, matching the G'):
        'ts-scalar',
    ('2026-09-13-typescript-joins-and-aggregates.md',
     "Still nothing compares the three clients' answers against ea"):
        'conformance-runner',
    ('2026-09-14-a-playground-that-runs-the-kernel.md',
     'Filters, sort, projection, limit, offset. No joins, aggregat'):
        'workbench-having',
    ('2026-09-14-a-playground-that-runs-the-kernel.md',
     'One filter, not a conjunction. `WHERE a = 1 AND b > 2` is ex'):
        'workbench-having',
    ('2026-09-14-a-playground-that-runs-the-kernel.md',
     'Nothing lazy-loads the bundle: 596 KiB is paid by every visi'):
        'wasm-lazy-import',
    ('2026-09-14-ci-that-had-never-run.md',
     'What follows was true until that tag, and is left because it'):
        '=tagged',
    ('2026-09-14-explaining-a-grouped-read.md',
     "The demo's `/api/explain` still explains the ungrouped read;"):
        'demo-grouped-plan',
    ('2026-09-14-guards-against-the-recurring-mistakes.md',
     'The Go and Python clients have no equivalent of the TypeScri'):
        'python-packaging',
    ('2026-09-14-joins-conjunctions-and-aggregates-in-the-binding.md',
     '`authors` and `books` only — no chain of three, though the k'):
        'wasm-chain',
    ('2026-09-14-joins-conjunctions-and-aggregates-in-the-binding.md',
     'No `HAVING` and no group ordering, both of which the kernel '):
        'workbench-having',
    ('2026-09-14-joins-conjunctions-and-aggregates-in-the-binding.md',
     'The panel exposes none of this yet. This commit is the bindi'):
        'workbench-having',
    ('2026-09-14-ports-below-the-ephemeral-range.md',
     "The orphaned `go` and `node` processes the job's cleanup rep"):
        'runner-teardown',
    ('2026-09-14-schema-checks-in-go-and-typescript.md',
     "The reasoning was written from the client's side alone, with"):
        'ts-renamed-column',
    ('2026-09-14-self-checking-quickstarts-and-conformance.md',
     'The conformance mode picks free ports for the adapters but t'):
        'demo-ports',
    ('2026-09-25-or-in-having-too.md',
     'No parentheses, so still no nesting.'):
        'having-brackets',
    ('2026-09-27-the-unmatched-side-computed-nothing-and-nothing-said-so.md',
     'The unmatched side is produced by a left join only.'):
        'full-join-unmatched',
    ('2026-09-20-an-array-on-the-wire-and-in-three-clients.md',
     'The demo and the docs site show no array.'):
        'array-in-the-ui',
    ('2026-09-27-the-last-twenty-six-and-a-hazard-nobody-had-written-down.md',
     'The backfill hazard is documented and unenforced.'):
        'sole-writer-refusal',
    ('2026-09-25-parentheses-in-a-where.md',
     'No brackets in a `HAVING`'):
        'having-brackets',
    ('2026-09-26-a-mistyped-section-is-a-refusal-now.md',
     'Nothing in CI runs an example with a bad section.'):
        'bad-section-run',
    ('2026-09-26-the-refusal-nothing-ran.md',
     'Nothing has run the negative case end to end.'):
        'headbench-smoke',
    ('2026-09-14-what-the-timer-measures.md',
     'The write paths are not timed.'):
        'write-timed',
    ('2026-09-21-generate-an-array-column-in-three-languages.md',
     'The `element_type` key is read from `--print-schema` and ass'):
        'element-spelling',
    ('2026-09-21-a-view-reported-as-a-typo.md',
     'Nothing asserts that `Head::table` stays the only producer o'):
        'sole-missing-table',
    ('2026-09-20-the-caveat-was-already-true.md',
     'The by-caller list is a claim about callers that nothing che'):
        'converter-callers',
    ('2026-09-20-the-caveat-was-already-true.md',
     'I did not re-audit the rest of `convert.rs`.'):
        'convert-audited',
    ('2026-09-28-a-cte-read-once-is-the-query-it-inlines-into.md',
     'It updates no design note or comparison row.'):
        'cte-docs-updated',
    ('2026-09-28-a-cte-read-once-is-the-query-it-inlines-into.md',
     "A repeated column in a CTE's projection"):
        'cte-repeated-column',
    ('2026-09-15-the-values-that-arrived-and-vanished.md',
     "No test declares a computed value on an outer join's unmatch"):
        'unmatched-go',
    ('2026-09-27-an-expression-over-a-row-that-is-half-missing.md',
     'The client accessors on an unmatched side are still untested'):
        'unmatched-ts',
    ('2026-09-23-five-built-features-the-docs-never-mentioned.md',
     'The client-facing docs were not touched.'):
        'client-five',
    ('2026-09-16-what-the-other-orms-have-that-this-does-not.md',
     'The browser render was a one-off, not a check.'):
        'docs-in-a-browser',
    ('2026-09-21-the-conformance-runners-own-arithmetic.md',
     '`tally` cannot see a case that never ran.'):
        'silent-case',
    ('2026-09-26-a-cached-go-run-is-not-a-run.md',
     "The in-flight marker's post-restore check inherits the hole'"):
        'restore-report',
    ('2026-09-19-who-may-see-a-deleted-row.md',
     '`purge_deleted` now needs two grants.'):
        'purge-grants',
    ('2026-09-15-a-chains-computed-value-and-a-type-tag-in-a-label.md',
     'No outer-join step in a chain carries a computed value.'):
        'outer-computed',
    ('2026-09-19-a-stops-at-that-stopped-being-true.md',
     'Only N4 was checked.'):
        'stops-at-reread',
    ('2026-09-20-an-array-is-a-terminator-not-a-count.md',
     "The design note's two open questions are still open."):
        'arrays-answered',
    ('2026-09-27-a-heading-that-said-open-over-two-answers.md',
     'The other design notes were not checked for the same thing.'):
        'views-answered',
    ('2026-09-27-five-narrowed-caveats-were-invisible-to-the-staleness-report.md',
     'Three residuals are now on the worklist and have not been re'):
        '=read',
    ('2026-09-26-the-refusal-nothing-ran.md',
     'Neither guard can see a fourth kind of silent pass.'):
        'argv-roster',
    ('2026-09-27-setup-go-v6-pins-gotoolchain-and-the-generator-needed-1-25.md',
     'A ledger entry citing a ledger entry is checked by nothing.'):
        'prose-citations',
    ('2026-09-27-three-red-jobs-nobody-was-watching.md',
     "The MinIO job is still red, and it is not this repository's "):
        'minio-image',
    ('2026-09-27-a-ledger-entry-could-cite-anything.md',
     '`FIXTURES` still has no never-fires half.'):
        'fixtures-alive',
    ('2026-09-27-two-more-examples-ignored-a-mistyped-flag.md',
     'The exclusion of servers from rule 4 is a property of the ru'):
        'runner-continue',
    ('2026-09-25-a-reworded-caveat-is-a-different-claim.md',
     'Nothing prevents the next orphan.'):
        'tracker-in-ci',
    ('2026-09-20-the-secret-that-debug-prints-as-numbers.md',
     'It covers `Debug`, and a secret can leave by other doors.'):
        'secret-roster',
    ('2026-09-20-the-secret-that-debug-prints-as-numbers.md',
     'Two types, asserted to be the whole surface by enumeration.'):
        'secret-roster',
    ('2026-09-22-four-dependencies-and-a-tool-that-was-lying.md',
     "`mutate.py`'s refusal cannot catch a dialect that reads *too"):
        'dialect-corpus',
    ('2026-09-26-the-refusal-nothing-ran.md',
     "`node --test`'s per-file wrapper line is absent from the nod"):
        'node-wrapper-counted',
    ('2026-09-22-the-page-a-reader-actually-reads.md',
     'Nothing checks that a marker is still deserved.'):
        'marker-deserved',
    ('2026-09-22-widening-the-tree-was-not-widening-the-claim.md',
     'The `~~` exemption is coarser than the guard now needs.'):
        'struck-span-flag',
    ('2026-09-20-taking-a-catalog-was-not-the-hazard.md',
     'Rule 3 runs over `slate-server` and `slate-serverd` only.'):
        'sources-complete',
    ('2026-09-20-the-caveat-was-already-true.md',
     'Two crates, named explicitly.'):
        'sources-complete',
    ('2026-09-14-ci-clippy-is-newer-than-mine.md',
     'It does not close the reporting gap.'):
        'ci-conclusion',
    ('2026-09-22-the-stale-claim-was-in-the-file-everyone-reads-first.md',
     'It does not read `site/`'):
        'site-cost-prose',
    ('2026-09-14-ci-clippy-is-newer-than-mine.md',
     'It does not address the deprecation warnings'):
        'node24-actions',
    ('2026-09-26-a-sixth-verdict-for-a-caveat-half-done.md',
     '`narrowed` has no `residual` field.'):
        'residual-field',
    ('2026-09-26-the-reverse-sweep-found-six.md',
     '`checked` now means two different things.'):
        'reviewed-field',
    ('2026-09-26-a-witness-for-every-closure.md',
     '16 closures are exempt and are checked by nothing.'):
        'exempt-because',
    ('2026-09-20-the-helper-that-can-be-used-now.md',
     'Go and TypeScript never restore against a real server.'):
        'conformance-restore-live',
    ('2026-09-20-the-helper-that-can-be-used-now.md',
     'The demo does not show it.'):
        'demo-restore-panel',
    ('2026-09-26-a-verdict-is-not-a-reading.md',
     'The reverse direction was not swept.'):
        'reverse-sweep',
    ('2026-09-26-the-verdict-sweep-finished-and-corrected-itself.md',
     'The reverse sweep is still not done'):
        'reverse-sweep',
    ('2026-09-26-the-reverse-sweep-found-six.md',
     'The 195 `closed` verdicts have never been re-read by anythin'):
        'closed-audited',
    ('2026-09-26-the-closed-verdicts-audited.md',
     'Nothing re-reads a `closed` verdict when the thing that clos'):
        'closed-caveats-guard',
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
    ('2026-09-28-round-trips-counted-at-the-clients-channel.md',
     'TypeScript has no request counter at all, and only Python co'):
        'ts-round-trips',
    ('2026-09-28-the-go-client-was-already-counting.md',
     'It does not instrument TypeScript.'):
        'ts-channel-options',
    ('2026-09-28-the-go-client-was-already-counting.md',
     'Neither Go test counts paging.'):
        'go-paging-count',
    ('2026-09-20-four-say-absent-one-says-present.md',
     'The predicate paths were probed at one shape.'):
        'retired-row-only-predicate',

    # The same-day census, 2026-09-29. Six of its ten closures are about the
    # tracker's own state and are `=stamped`; these four name code.
    ('2026-09-28-the-invisible-backlog-part-two.md',
     'The four unmeasured round-trip claims are grouped here and n'):
        'many-to-many-two-reads',
    ('2026-09-28-two-reads-counted-rather-than-argued.md',
     'It leaves the other two unmeasured claims where they were.'):
        'py-round-trips',
    ('2026-09-28-the-scale-hole-was-closed-ten-days-ago.md',
     'It does not re-read the other 113 open caveats against the t'):
        '=stamped',
    ('2026-09-28-two-limits-that-are-weaker-than-their-names.md',
     'It does not re-read the rest of the open caveats.'):
        '=stamped',
    ('2026-09-28-reading-the-open-caveats-instead-of-grepping-them.md',
     'It reads eleven of 116, and changes four.'):
        '=stamped',
    ('2026-09-28-twenty-five-caveats-read-and-eight-greps-that-lied.md',
     'It does not examine the other ninety-one.'):
        '=stamped',
    ('2026-09-28-fifty-one-of-ninety-nine-read-at-random.md',
     'It leaves forty-eight of the ninety-nine unread'):
        '=stamped',
    ('2026-09-28-a-random-twelve-found-nothing.md',
     'It reads twelve, so the interval is wide.'):
        '=stamped',
    ('2026-09-28-a-random-twelve-found-nothing.md',
     'It says nothing about the `deliberate` verdicts.'):
        '=read',
    ('2026-09-28-fifty-one-of-ninety-nine-read-at-random.md',
     'It still says nothing about the 890 `deliberate` verdicts.'):
        '=read',
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

    exempt_rows = {k for k, w in WITNESSED.items() if w.startswith("=")}
    unreasoned = sorted(f"{e}: {k}" for e, k in exempt_rows - set(EXEMPT_BECAUSE))
    record(
        "every exempt closure says why it leaves nothing in the tree",
        not unreasoned,
        "\n      ".join(unreasoned),
    )
    stale_reason = sorted(f"{e}: {k}" for e, k in set(EXEMPT_BECAUSE) - exempt_rows)
    record(
        "no reason outlives the exemption it explains",
        not stale_reason,
        "\n      ".join(stale_reason),
    )

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
        f"the other {len(WITNESSED) - witnessed} are exempt, each with its own "
        f"reason in EXEMPT_BECAUSE"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
