#!/bin/sh
# Run the helpdesk over SlateDB, on an object store, and re-read it after a
# restart.
#
# Everything else in this crate runs over `MemoryStore`, which is a
# `BTreeMap`. The claim this settles is the one the crate is built on:
# `Helpdesk<S>` is generic over `KvStore`, and until something drove it
# against an LSM on a bucket that was a claim about a type parameter.
#
#     sh examples/helpdesk/run.sh              # a local-filesystem bucket
#     sh examples/helpdesk/run.sh --smoke      # the same, seven tickets
#     sh examples/helpdesk/run.sh --s3         # a real signed S3 server
#
# `--release`, always. `CLAUDE.md` records the measurement behind that: the
# nine `slate-slatedb` examples are 131 MB at `--release` against well over
# 1.4 GB in debug, which is `-C debuginfo=2` and nothing else, and a debug
# build of this one on the development container dies with a linker `Bus
# error` or an `LLVM ERROR: IO failure on output stream` — both of which are
# ENOSPC wearing a disguise. CI has room and does not care; this is for here,
# and it costs nothing there.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
work=$(mktemp -d)

s3=
smoke=
for argument in "$@"; do
  case "$argument" in
    --s3) s3=1 ;;
    --smoke) smoke=--smoke ;;
    *) echo "usage: run.sh [--s3] [--smoke]; \`$argument\` is not an option" >&2; exit 2 ;;
  esac
done

# Wait for the server before removing its directory, and never let the
# cleanup decide the exit code — `examples/retention/run.sh` met both: an
# `rm -rf` raced a dying process, failed with "Directory not empty", and
# because it was the trap's last command it became the script's status, so a
# passing run reported failure.
cleanup() {
  status=$?
  if [ -n "${s3_pid:-}" ]; then
    kill "$s3_pid" 2>/dev/null || true
    wait "$s3_pid" 2>/dev/null || true
  fi
  rm -rf "$work" 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT

echo "building the runner (release)..."
CARGO_INCREMENTAL=0 cargo build -q --release -p slate-helpdesk --example over_slatedb

if [ -n "$s3" ]; then
  echo "starting an S3 server..."
  CARGO_INCREMENTAL=0 cargo build -q --release -p slate-slatedb --example s3_server
  "$root/target/release/examples/s3_server" --bucket slate-helpdesk > "$work/s3.log" 2>&1 &
  s3_pid=$!
  # Wait for the handshake line rather than polling the port, which closes
  # the race between bind and accept. Same line `slate-serverd` prints.
  for _ in $(seq 300); do
    grep -q '^LISTENING ' "$work/s3.log" 2>/dev/null && break
    sleep 0.2
  done
  if ! grep -q '^LISTENING ' "$work/s3.log"; then
    echo "the S3 server never announced itself:" >&2
    cat "$work/s3.log" >&2
    exit 1
  fi
  # The server prints its own `SLATE_S3_*` settings in a shape a shell can
  # read, which is how the runner is pointed at it without this script
  # knowing the credentials.
  # shellcheck disable=SC2046
  export $(grep '^SLATE_S3_' "$work/s3.log" | xargs)
fi

# shellcheck disable=SC2086
"$root/target/release/examples/over_slatedb" $smoke --dir "$work/store"
