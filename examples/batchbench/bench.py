"""N single inserts against one batch of N, through the Python SDK.

The shape `slate-headbench` uses, from a client instead of from the wire, for
the reason `README.md` gives: the multiplier a client sees is the wire's minus
the per-operation work a client does and a batch does not save.

Reports a median and a range over five runs. Does not assert: see the README.
"""

from __future__ import annotations

import argparse
import statistics
import time

from slate import (
    Atomicity,
    Batch,
    Client,
    Column,
    Identity,
    Session,
    Table,
    ValueType,
    u64,
)

BENCH = Table(
    "bench",
    [Column("id", ValueType.U64), Column("name", ValueType.STR)],
    primary_key=["id"],
)


# `Session`, not `object`. It was `object` with two
# `# type: ignore[attr-defined]` comments, which is a mypy spelling that ty
# does not read — so the annotation was wrong, the suppressions were inert, and
# neither checker had ever seen these two calls.
def singles(session: Session, base: int, rows: int) -> float:
    start = time.perf_counter()
    for n in range(rows):
        session.insert(BENCH, [[u64(base + n), f"row-{n}"]])
    return time.perf_counter() - start


def batched(session: Session, base: int, rows: int) -> float:
    batch = Batch(Atomicity.ALL_OR_NOTHING)
    batch.insert(BENCH, [[u64(base + n), f"row-{n}"] for n in range(rows)])
    start = time.perf_counter()
    session.batch(batch)
    return time.perf_counter() - start


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--address", required=True)
    parser.add_argument("--rows", type=int, default=100)
    parser.add_argument("--runs", type=int, default=5)
    args = parser.parse_args()

    client = Client(
        args.address, Identity("u64:1", tenant="u64:1", roles=["app"])
    )
    client.wait_for_ready()
    session = client.session()
    base = 1_000_000

    single_times: list[float] = []
    batch_times: list[float] = []
    for run in range(args.runs):
        # Disjoint key ranges per run and per arm: an insert refuses a taken
        # key, and a second run over the same keys would measure the refusal.
        single_times.append(singles(session, base + run * 2 * args.rows, args.rows))
        batch_times.append(batched(session, base + run * 2 * args.rows + args.rows, args.rows))
    client.close()

    report("python", args.rows, single_times, batch_times)


def report(name: str, rows: int, singles_s: list[float], batches_s: list[float]) -> None:
    per_single = [t / rows * 1e6 for t in singles_s]
    per_batch = [t / rows * 1e6 for t in batches_s]
    print(
        f"{name}\t{rows}\t"
        f"{statistics.median(per_single):.1f}\t{min(per_single):.1f}\t{max(per_single):.1f}\t"
        f"{statistics.median(per_batch):.1f}\t{min(per_batch):.1f}\t{max(per_batch):.1f}\t"
        f"{statistics.median(per_single) / statistics.median(per_batch):.1f}"
    )


if __name__ == "__main__":
    main()
