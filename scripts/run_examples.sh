#!/bin/sh
# Run every example in a crate, and fail if any of them stops working.
#
#     sh scripts/run_examples.sh <crate> [--smoke]
#
# Two crates here hold benchmarks that `cargo` builds and nothing runs.
# `slate-headbench`'s five had four broken at once — three by `leadership`
# gaining authentication, a fourth by `EXPLAIN` becoming privileged — and were
# found only when #265 ran them. `slate-slatedb`'s eight are where the
# planner's two cost constants come from, and running them in #269 and #270
# found that one measured a warm cache while claiming to be cold and another
# fenced itself by opening a second writer. A benchmark nobody runs is a
# constant nobody re-measures, which is how `POINT_READ_COST` came to be three
# times its measured value.
#
# One script rather than one per crate. The alternative was a second copy of
# the first, differing in a directory and five environment variables, which is
# precisely the list-nobody-keeps-in-sync this whole file exists to argue
# against. Each crate keeps a thin `run.sh` so the paths already in
# `docs/performance.md` stay true.
#
# `--smoke` is what CI runs: every example, the smallest fixture each accepts.
# The numbers it prints are worthless and it does not pretend otherwise —
# nothing here asserts a duration, because a wall-clock threshold on a shared
# runner is a flake waiting for a slow morning. What it proves is that each one
# still runs against the current tree.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

crate=${1:-}
[ -n "$crate" ] || { echo "usage: run_examples.sh <crate> [--smoke]" >&2; exit 2; }
shift

smoke=
case "${1:-}" in
    --smoke) smoke=1 ;;
    "") ;;
    *) echo "usage: run_examples.sh <crate> [--smoke]" >&2; exit 2 ;;
esac

# Where the list comes from and where the binaries are, both overridable.
#
# Only the tests beside this file set them, and they are the reason this can be
# tested at all: the loop, the floor, the handshake and the counting are all
# properties of a *directory of binaries*, and against the real tree — where
# everything passes — a mutation to any of them survives. `scripts/mutate.py`
# said so about three of them, which is what these two variables are for.
examples_dir=${EXAMPLES_DIR:-$root/crates/$crate/examples}
binaries_dir=${BINARIES_DIR:-$root/target/debug/examples}
[ -d "$examples_dir" ] || {
    echo "no examples directory at $examples_dir" >&2
    exit 1
}

# How many each crate should have, so moving one out is a failure rather than a
# smaller green run. A number per crate rather than one shared floor, because
# "at least one" would not catch the case this is for.
case "$crate" in
    slate-headbench) least=5 ;;
    slate-slatedb) least=8 ;;
    *) echo "no expected example count for $crate; add one to run_examples.sh" >&2; exit 2 ;;
esac

# The list is read off disk rather than written here. A new benchmark is then
# covered by existing in the directory, which is the whole difference between
# this and a roster somebody has to remember to extend.
examples=$(ls "$examples_dir"/*.rs | sed 's|.*/||; s|\.rs$||')

# The never-fires half. `set -e` already covers the two loud cases — a renamed
# directory, or one holding no `.rs` at all, makes `ls` fail and aborts. What
# this catches is the quiet case those miss: a directory that still has *some*
# examples. Move one out, or leave a rename half done, and without this the
# loop runs the rest and prints a green summary over a tree with one missing.
count=$(printf '%s\n' "$examples" | grep -c .)
if [ "$count" -lt "$least" ]; then
    echo "found $count examples under $examples_dir; expected at least $least." >&2
    echo "Either they moved or this script is looking in the wrong place." >&2
    exit 1
fi

cd "$root"
echo "Building $count examples in $crate…"
# Incremental off, for the reason `CLAUDE.md` gives: it roughly halves what a
# build leaves on disk, and linking eight binaries against slatedb, s3s and
# tonic is one of the places this container runs out. The failure does not say
# ENOSPC — it says `rustc-LLVM ERROR: IO failure` and a linker `Bus error`, and
# reads exactly like broken code. Nothing here is incremental anyway: this
# builds once and runs.
# `SKIP_BUILD` is the tests' other half: they hand this a directory of
# binaries they wrote, and there is no crate to build.
if [ -z "${SKIP_BUILD:-}" ]; then
    CARGO_INCREMENTAL=0 cargo build -p "$crate" --examples
fi

if [ -n "$smoke" ]; then
    # Every knob at its smallest, both crates' spellings exported together.
    # Two names, one per crate — `HEADBENCH_*` and `SCALE_ROWS` — because each
    # is documented where its benchmarks are, and setting the union is cheaper
    # than renaming one and chasing the docs that quote it. An example that
    # reads neither runs at its recorded size, which is why the floors below
    # are the ones that matter for how long this takes.
    HEADBENCH_RUNS=1
    HEADBENCH_ROWS=500
    HEADBENCH_BATCHES=64
    HEADBENCH_LEVELS=1,4
    HEADBENCH_WINDOW_MS=100
    HEADBENCH_DURABLE_MS=200
    SCALE_ROWS=2000
    export HEADBENCH_RUNS HEADBENCH_ROWS HEADBENCH_BATCHES HEADBENCH_LEVELS \
           HEADBENCH_WINDOW_MS HEADBENCH_DURABLE_MS SCALE_ROWS
fi

# Examples that are *servers*, and the line each prints once it is up.
#
# `s3_server` serves until killed — it exists for `examples/deployed/run.sh` to
# point a head node at — so "runs to completion" is not a property it has, and
# a smoke run waiting for it to exit waits forever. That is what the first run
# of this script did.
#
# Skipping it was the obvious answer and is the one `CLAUDE.md` names: **a skip
# is green**. It prints `LISTENING <addr>` once bound, deliberately, as the
# handshake `examples/deployed/run.sh` waits for — so there is something to
# assert, and waiting for that line and then killing it proves as much about
# the binary as running a benchmark to completion does. One entry per server,
# so a second needs a line of its own rather than inheriting a guess.
handshake() {
    case "$1/$2" in
        slate-slatedb/s3_server) echo "LISTENING" ;;
        *) echo "" ;;
    esac
}

# How long to wait for a server's handshake before calling it broken.
HANDSHAKE_SECONDS=${HANDSHAKE_SECONDS:-30}

lines=""
failed=0
for name in $examples; do
    printf '\n--- %s\n' "$name"
    started=$(date +%s)
    wanted=$(handshake "$crate" "$name")
    if [ -n "$wanted" ]; then
        log=$(mktemp)
        "$binaries_dir/$name" >"$log" 2>&1 &
        server=$!
        waited=0
        until grep -q "$wanted" "$log" 2>/dev/null; do
            if [ "$waited" -ge "$HANDSHAKE_SECONDS" ] || ! kill -0 "$server" 2>/dev/null; then
                break
            fi
            sleep 1
            waited=$(( waited + 1 ))
        done
        if grep -q "$wanted" "$log" 2>/dev/null; then
            result=$(printf 'ok    %s, said %s in %ss' "$name" "$wanted" "$waited")
        else
            result=$(printf 'FAIL  %s, no %s in %ss' "$name" "$wanted" "$HANDSHAKE_SECONDS")
            failed=$(( failed + 1 ))
            sed 's/^/    /' "$log"
        fi
        kill "$server" 2>/dev/null || true
        wait "$server" 2>/dev/null || true
        rm -f "$log"
        lines="$lines$result
"
        continue
    fi
    if "$binaries_dir/$name"; then
        result=$(printf 'ok    %s, %ss' "$name" "$(( $(date +%s) - started ))")
    else
        code=$?
        result=$(printf 'FAIL  %s, exit %s' "$name" "$code")
        # Not `exit` — the point of running all of them is knowing how many are
        # broken, and `cargo test`'s fail-fast is named in `CLAUDE.md` as a way
        # to be wrong about exactly that. Four of one crate's five were broken.
        failed=$(( failed + 1 ))
    fi
    lines="$lines$result
"
done

# `ok    name` / `FAIL  name` and a closing `N passed, M failed`, unindented:
# this repository's house style for a runner, and `scripts/mutate.py` reads it
# as its `python` dialect — so a mutation to anything these exercise is scored
# rather than reported as "nothing ran". The summary prints on the failing path
# too, which is what the conformance runner had to be taught for the same
# reason.
printf '\n'
printf '%s' "$lines"
printf '\n%s passed, %s failed\n' "$(( count - failed ))" "$failed"
[ "$failed" -eq 0 ]
