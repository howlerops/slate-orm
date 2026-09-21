#!/bin/sh
# Run every benchmark in this crate. See `scripts/run_examples.sh`, which this
# delegates to and which explains why there is one runner rather than two.
#
#     crates/slate-headbench/run.sh            # at their recorded sizes
#     crates/slate-headbench/run.sh --smoke    # every section, smallest fixture
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
exec sh "$root/scripts/run_examples.sh" slate-headbench "$@"
