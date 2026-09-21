#!/bin/sh
# Run every benchmark in this crate, and fail if any of them stops working.
#
# Three of the five had been broken for weeks before anything ran one. The
# cause was a security fix — `leadership` gained authentication and the
# benchmarks warmed their channels with an anonymous request — and the reason
# nobody noticed is that `ci.yml` builds this crate and runs none of it. A
# fourth turned out to be broken by a *different* security fix, `EXPLAIN`
# becoming privileged, and was still broken after that first repair: the guard
# tests added then check the shape of the source and one round trip, and said
# in their own comments that they could not see an example authenticating with
# a role that lacks a grant. Running them is what sees it.
#
# `--smoke` is what CI runs: every benchmark, every section, the smallest
# fixture each will accept. The numbers it prints are worthless and it does not
# pretend otherwise — nothing here asserts a duration, because a wall-clock
# threshold on a shared runner is a flake waiting for a slow morning. What it
# proves is that each one still runs against the current tree.
#
# With no argument it runs them at their recorded sizes, which is what you want
# when you are reading the numbers rather than guarding the code.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)

smoke=
case "${1:-}" in
    --smoke) smoke=1 ;;
    "") ;;
    *) echo "usage: run.sh [--smoke]" >&2; exit 2 ;;
esac

# The list is read off disk rather than written here. A sixth benchmark is then
# covered by existing in the directory, which is the whole difference between
# this and a roster somebody has to remember to extend — and the failure this
# script exists because of was, at bottom, a list of things nobody ran.
examples=$(ls "$here/examples"/*.rs | sed 's|.*/||; s|\.rs$||')

# The never-fires half. `set -e` already covers the two loud cases — a renamed
# directory, or one holding no `.rs` at all, makes `ls` fail and aborts — and
# the comment here first claimed those were what this catches, which running it
# disproved. What it actually catches is the quiet case those miss: a directory
# that still has *some* benchmarks. Move one out, or leave a rename half done,
# and without this the loop runs the four that are left and prints
# `4 passed, 0 failed` over a tree with a benchmark missing from it.
count=$(printf '%s\n' "$examples" | grep -c .)
if [ "$count" -lt 5 ]; then
    echo "found $count benchmarks under $here/examples; expected at least 5." >&2
    echo "Either they moved or this script is looking in the wrong place." >&2
    exit 1
fi

cd "$root"
echo "Building $count benchmarks…"
# Incremental off, for the reason `CLAUDE.md` gives: it roughly halves what a
# build leaves on disk, and linking five binaries against slatedb, s3s and
# tonic is one of the places this container runs out. The failure does not say
# ENOSPC — it says `rustc-LLVM ERROR: IO failure` and a linker `Bus error`, and
# reads exactly like broken code. Nothing here is incremental anyway: this
# builds once and runs.
CARGO_INCREMENTAL=0 cargo build -p slate-headbench --examples

if [ -n "$smoke" ]; then
    # Every knob at its smallest. `LEVELS` matters most: `head_concurrency`
    # sweeps eight concurrency points by default and each one is a timed
    # window, so the sweep is the cost rather than the work per point.
    HEADBENCH_RUNS=1
    HEADBENCH_ROWS=500
    HEADBENCH_BATCHES=64
    HEADBENCH_LEVELS=1,4
    HEADBENCH_WINDOW_MS=100
    HEADBENCH_DURABLE_MS=200
    export HEADBENCH_RUNS HEADBENCH_ROWS HEADBENCH_BATCHES HEADBENCH_LEVELS \
           HEADBENCH_WINDOW_MS HEADBENCH_DURABLE_MS
fi

lines=""
failed=0
for name in $examples; do
    printf '\n--- %s\n' "$name"
    started=$(date +%s)
    if "$root/target/debug/examples/$name"; then
        result=$(printf 'ok    %s, %ss' "$name" "$(( $(date +%s) - started ))")
    else
        code=$?
        result=$(printf 'FAIL  %s, exit %s' "$name" "$code")
        # Not `exit` — the point of running all five is knowing how many are
        # broken, and `cargo test`'s fail-fast is named in `CLAUDE.md` as a way
        # to be wrong about exactly that. Four of these were broken at once.
        failed=$(( failed + 1 ))
    fi
    lines="$lines$result
"
done

# `ok    name` / `FAIL  name` and a closing `N passed, M failed`, unindented:
# that is this repository's house style for a runner, and `scripts/mutate.py`
# reads it as its `python` dialect — so a mutation to this script, or to
# anything these benchmarks touch, is scored rather than reported as "nothing
# ran". The summary prints on the failing path too, which is what the
# conformance runner had to be taught for the same reason.
printf '\n'
printf '%s' "$lines"
printf '\n%s passed, %s failed\n' "$(( count - failed ))" "$failed"
[ "$failed" -eq 0 ]
