#!/bin/sh
# Run every example in this crate. See `scripts/run_examples.sh`, which this
# delegates to and which explains why there is one runner rather than two.
#
#     crates/slate-slatedb/run.sh            # at their recorded sizes
#     crates/slate-slatedb/run.sh --smoke    # smallest fixture each accepts
#
# These are where the planner's `SCAN_ROW_COST` and `POINT_READ_COST` come
# from, so what they measure is what the query planner believes.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
exec sh "$root/scripts/run_examples.sh" slate-slatedb "$@"
