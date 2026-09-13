#!/usr/bin/env python3
"""Break each thing this package does, and check a named test notices.

A passing suite measures effort. What it does not measure is whether any
particular line is load-bearing, and `docs/correctness.md` opens by pointing
out that every bug this project has found was found by something other than the
test that should have caught it.

So: for each mutation below, apply it, run the suite, and require that the
named test fails. A mutation nothing notices is a missing test, and is reported
as one rather than quietly dropped.

The mutations are exact string substitutions rather than a coverage-guided
mutator, because the interesting ones are semantic — "resolve every reference
to input 0", "stop threading the watermark" — and a mutator that flips
comparison operators would not produce any of them.

    python scripts/mutate.py            # run them all
    python scripts/mutate.py --list     # just say what they are
"""

from __future__ import annotations

import argparse
import dataclasses
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


@dataclasses.dataclass(frozen=True)
class Mutation:
    """One deliberate break, and the test that must notice it."""

    name: str
    path: str
    old: str
    new: str
    #: A substring of the test id that must appear in the failures.
    expect: str


MUTATIONS: list[Mutation] = [
    # --- the reference model ---------------------------------------------
    Mutation(
        "every reference resolves to input 0",
        "src/slate/expr.py",
        "        ref = pb.ColumnRef(input=self.input)",
        "        ref = pb.ColumnRef(input=0)",
        "test_a_join_input_addresses_its_own_position",
    ),
    Mutation(
        "join inputs are all given position 0",
        "src/slate/query.py",
        "        wire_input = JoinInput(table, len(self._inputs))",
        "        wire_input = JoinInput(table, 0)",
        "test_a_self_join_gives_the_two_inputs_different_positions",
    ),
    Mutation(
        "column ordinals are off by one",
        "src/slate/expr.py",
        "            index=ordinal,\n            label=f\"{self._table.name}.{name}\",",
        "            index=ordinal + 1,\n            label=f\"{self._table.name}.{name}\",",
        "test_a_single_table_query_addresses_input_zero",
    ),
    Mutation(
        "a computed reference is sent as a plain column",
        "src/slate/expr.py",
        'return ColumnRef(input=input, kind=_Kind.COMPUTED, index=index, label=f"computed {index}")',
        'return ColumnRef(input=input, kind=_Kind.COLUMN, index=index, label=f"computed {index}")',
        "test_a_computed_reference_carries_its_kind_not_an_offset",
    ),
    Mutation(
        "an aggregate reference is sent as a group key",
        "src/slate/expr.py",
        'return ColumnRef(input=0, kind=_Kind.AGGREGATE, index=index, label=f"aggregate {index}")',
        'return ColumnRef(input=0, kind=_Kind.GROUP_KEY, index=index, label=f"aggregate {index}")',
        "test_agg_group_having_over_an_aggregate",
    ),
    Mutation(
        "a join condition may name any input on its own side",
        "src/slate/query.py",
        "            if own.input != self._input:",
        "            if False:",
        "test_a_join_condition_naming_another_input_on_its_own_side_is_refused",
    ),
    Mutation(
        "a join condition may name a later input as earlier",
        "src/slate/query.py",
        "        if earlier.input >= self._input:",
        "        if False:",
        "test_a_join_condition_naming_a_later_input_as_earlier_is_refused",
    ),
    Mutation(
        "the response side stops adding the table width",
        "src/slate/rows.py",
        "        at = self._table.width + index",
        "        at = index",
        "test_docs_computed",
    ),
    # --- values -----------------------------------------------------------
    Mutation(
        "a bare integer defaults to int64 instead of being refused",
        "src/slate/values.py",
        '        raise ValueTypeError(\n            f"the integer {value} is going into a slot',
        '        return pb.Value(int64_value=value)\n        raise ValueTypeError(\n            f"the integer {value} is going into a slot',
        "test_an_integer_with_no_declared_type_is_refused",
    ),
    Mutation(
        "the declared column type is ignored and everything is int64",
        "src/slate/values.py",
        "        if hint is ValueType.U64:\n            return pb.Value(uint64_value=value)",
        "        if False:\n            return pb.Value(uint64_value=value)",
        "test_an_integer_is_encoded_as_the_column_declares_it",
    ),
    Mutation(
        "a bool is encoded as an integer",
        "src/slate/values.py",
        "    if isinstance(value, bool):\n        return pb.Value(bool_value=value)",
        "    if False:\n        return pb.Value(bool_value=value)",
        "test_a_bool_is_not_an_integer",
    ),
    Mutation(
        "an integer comes back as a bare int, losing its width",
        "src/slate/values.py",
        '    if kind == "uint64_value":\n        return u64(value.uint64_value)',
        '    if kind == "uint64_value":\n        return i64(value.uint64_value)',
        "test_docs_all",
    ),
    # --- freshness --------------------------------------------------------
    Mutation(
        "the watermark is never advanced",
        "src/slate/freshness.py",
        "        if self._sequence is None or sequence > self._sequence:\n            self._sequence = sequence",
        "        return",
        "test_a_session_reads_its_own_write_without_the_caller_naming_a_token",
    ),
    Mutation(
        "reads stop carrying the session's freshness",
        "src/slate/client.py",
        "        implied = self._watermark.freshness()\n        return None if implied is None else implied.to_proto()",
        "        return None",
        "test_a_session_reads_its_own_write_without_the_caller_naming_a_token",
    ),
    Mutation(
        "a read's own sequence is not observed, so reads are not monotonic",
        "src/slate/client.py",
        "        if self._monotonic_reads and served_by is not None:\n            self._watermark.observe(served_by.sequence)",
        "        return",
        "test_monotonic_reads_pin_a_session_that_has_seen_the_writer",
    ),
    Mutation(
        "the watermark can move backwards",
        "src/slate/freshness.py",
        "        if self._sequence is None or sequence > self._sequence:",
        "        if True:",
        "test_the_watermark_is_readable_and_only_moves_forward",
    ),
    Mutation(
        "a commit does not feed the session's watermark",
        "src/slate/client.py",
        "            self._session._observe(response.sequence)",
        "            pass",
        "test_a_transaction_commit_feeds_the_session_watermark",
    ),
    Mutation(
        "freshness inside a transaction is accepted and ignored",
        "src/slate/client.py",
        '            raise ValueError(\n                "a read inside a transaction is served by that transaction, on the "',
        '            return asked.to_proto()\n            raise ValueError(\n                "a read inside a transaction is served by that transaction, on the "',
        "test_freshness_inside_a_transaction_is_refused_rather_than_ignored",
    ),
    # --- transactions -----------------------------------------------------
    Mutation(
        "a transaction is not rolled back when the body raises",
        "src/slate/client.py",
        "            transaction._rollback_quietly()\n            raise",
        "            raise",
        "test_a_transaction_rolls_back_on_an_exception",
    ),
    Mutation(
        "transact does not retry a conflict",
        "src/slate/client.py",
        "        for attempt in range(attempts):",
        "        for attempt in range(1):",
        "test_transact_retries_a_conflict_and_loses_no_update",
    ),
    Mutation(
        "an explicit rollback is followed by a commit",
        "src/slate/client.py",
        "        if not transaction._finished:\n            transaction._commit()",
        "        transaction._commit()",
        "test_an_explicit_rollback_discards_the_writes",
    ),
    # --- errors -----------------------------------------------------------
    Mutation(
        "UNKNOWN becomes retryable",
        "src/slate/errors.py",
        "class UnknownOutcome(SlateError):",
        "class UnknownOutcome(Retryable):",
        "test_unknown_is_not_retryable",
    ),
    Mutation(
        "the leader redirect is not recognised",
        "src/slate/errors.py",
        "    if kind is Unavailable and LEADER_KEY in trailers:\n        kind = NotLeader",
        "    if False:\n        kind = NotLeader",
        "test_a_write_to_a_follower_is_not_leader_with_the_leader_named",
    ),
    Mutation(
        "ABORTED is no longer mapped, so a conflict reads as INTERNAL",
        "src/slate/errors.py",
        "    grpc.StatusCode.ABORTED: Conflict,",
        "",
        "test_every_grpc_code_has_a_mapping",
    ),
    Mutation(
        "a gRPC failure is not translated at all",
        "src/slate/client.py",
        "        except grpc.RpcError as error:\n            raise from_rpc_error(error) from error\n\n    def _stream",
        "        except grpc.RpcError:\n            raise\n\n    def _stream",
        "test_not_found_for_a_missing_row_on_update",
    ),
    # --- streams ----------------------------------------------------------
    Mutation(
        "the first message is not read eagerly, so served_by is unknown",
        "src/slate/client.py",
        "        self._served_by: ServedBy | None = None\n        self._pump()",
        "        self._served_by: ServedBy | None = None",
        "test_served_by_is_populated_before_the_first_row",
    ),
    # --- query assembly ---------------------------------------------------
    Mutation(
        "an empty disjunction becomes TRUE",
        "src/slate/expr.py",
        "    items = list(parts)\n    if not items:\n        return FALSE",
        "    items = list(parts)\n    if not items:\n        return TRUE",
        "test_an_empty_disjunction_is_false_not_true",
    ),
    Mutation(
        "a projection of nothing becomes a projection of everything",
        "src/slate/query.py",
        '        """No stored columns at all, which is what a count wants."""\n        self._projection = pb.Projection()',
        '        """No stored columns at all, which is what a count wants."""\n        self._projection = pb.Projection(all_columns=True)',
        "test_a_projection_of_nothing_is_distinct_from_no_projection",
    ),
    Mutation(
        "COUNT(column) is sent as COUNT(*)",
        "src/slate/query.py",
        "        return Agg(pb.AGGREGATE_FUNCTION_COUNT_COLUMN, column)",
        "        return Agg(pb.AGGREGATE_FUNCTION_COUNT)",
        "test_count_star_carries_no_column_and_count_column_does",
    ),
    Mutation(
        "a sort key's direction is dropped",
        "src/slate/query.py",
        "            direction=pb.SORT_DIRECTION_DESC if self.descending else pb.SORT_DIRECTION_ASC,",
        "            direction=pb.SORT_DIRECTION_ASC,",
        "test_docs_filtered_paged",
    ),
    Mutation(
        "a query's limit is not sent",
        "src/slate/query.py",
        "        if self._limit is not None:\n            query.limit = self._limit\n        query.offset = self._offset",
        "        query.offset = self._offset",
        "test_docs_filtered_paged",
    ),
    Mutation(
        "a join's second condition is dropped",
        "src/slate/query.py",
        "        self._on.append((earlier, own_ref))",
        "        self._on[:] = [(earlier, own_ref)]",
        "test_join_two_keys",
    ),
    Mutation(
        "a join input's own filter is dropped",
        "src/slate/query.py",
        "        if self._filter is not None:\n            query.filter.CopyFrom(self._filter.to_proto())",
        "        if False:\n            query.filter.CopyFrom(self._filter.to_proto())",
        "test_join_right_filtered",
    ),
    Mutation(
        "a row of the wrong width is sent rather than refused",
        "src/slate/client.py",
        "            if len(values) != len(types):",
        "            if False:",
        "test_a_row_of_the_wrong_width_is_refused_before_it_is_sent",
    ),
    # --- the fixture and the oracle ---------------------------------------
    Mutation(
        "the declared schema gains a column the server does not have",
        "tests/fixture.py",
        '        Column("note", STR),\n    ],\n    primary_key=["id"],\n)',
        '        Column("note", STR),\n        Column("ghost", STR),\n    ],\n    primary_key=["id"],\n)',
        "test_the_declared_width_matches_the_rows_the_server_returns",
    ),
    Mutation(
        "the oracle file is silently truncated",
        "tests/oracle.py",
        "        self._answers: dict[str, Any] = json.loads(path.read_text())",
        "        self._answers: dict[str, Any] = {}",
        "test_docs_all",
    ),
    Mutation(
        "a committed protobuf stub drifts from the .proto",
        "src/slate/_proto/slate/v1/records_pb2_grpc.py",
        "GRPC_GENERATED_VERSION = ",
        "_DRIFT = 1\nGRPC_GENERATED_VERSION = ",
        "test_the_committed_stubs_match_the_proto",
    ),
]


def run_suite() -> set[str]:
    """The set of failing test names."""
    result = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "--tb=no", "-p", "no:randomly"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    return set(re.findall(r"^FAILED ([^\s:]+::\S+)", result.stdout, re.MULTILINE)) | set(
        re.findall(r"^ERROR ([^\s:]+::\S+)", result.stdout, re.MULTILINE)
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--list", action="store_true")
    parser.add_argument("--only", help="substring of a mutation name")
    args = parser.parse_args()

    chosen = [
        m for m in MUTATIONS if not args.only or args.only.lower() in m.name.lower()
    ]
    if args.list:
        for mutation in chosen:
            print(f"{mutation.name}\n    expects {mutation.expect}")
        return 0

    rows: list[tuple[str, str, str]] = []
    survivors = 0
    for mutation in chosen:
        path = ROOT / mutation.path
        original = path.read_text()
        if mutation.old not in original:
            rows.append((mutation.name, "NOT APPLIED", "the anchor text is gone"))
            survivors += 1
            print(f"!! {mutation.name}: anchor not found in {mutation.path}")
            continue
        path.write_text(original.replace(mutation.old, mutation.new, 1))
        try:
            failures = run_suite()
        finally:
            path.write_text(original)
        caught = sorted(f for f in failures if mutation.expect in f)
        if caught:
            rows.append((mutation.name, caught[0], f"{len(failures)} failing"))
            print(f"ok {mutation.name}\n     caught by {caught[0]} ({len(failures)} failing)")
        else:
            survivors += 1
            rows.append((mutation.name, "SURVIVED", ", ".join(sorted(failures)) or "nothing failed"))
            print(f"!! {mutation.name} SURVIVED — failures were: {sorted(failures)}")

    print("\n| mutation | caught by | note |")
    print("|---|---|---|")
    for name, caught, note in rows:
        print(f"| {name} | `{caught}` | {note} |")
    print(f"\n{len(rows) - survivors}/{len(rows)} caught")
    return 1 if survivors else 0


if __name__ == "__main__":
    raise SystemExit(main())
