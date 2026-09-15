#!/bin/sh
# Stand the whole system up, load the workbench's dataset, and check it.
#
# Three processes and one bucket:
#
#   s3_server      a real S3 server on a real socket (`s3s` + `s3s-fs`)
#   slate-serverd  the head node, SlateDB on top of that bucket
#   check.py       the Python SDK over gRPC, against a fold done in Python
#
# Nothing is mocked between the gRPC socket and the object store. What is *not*
# production-grade is the S3 implementation itself — see the note in
# `examples/s3_server.rs` and the `minio` job in CI, which covers that layer.
#
#   ./run.sh              start everything, load, check, tear down
#   ./run.sh --keep       the same, but leave it running and print the address
#   ./run.sh --trips N    load only the first N trips, for a quick run
#
# Every port is picked by the kernel, so this can run twice at once and cannot
# fail confusingly because something else holds 50051.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

keep=
trips=
while [ $# -gt 0 ]; do
  case $1 in
    --keep) keep=1 ;;
    --trips) shift; trips=$1 ;;
    *) echo "usage: run.sh [--keep] [--trips N]" >&2; exit 2 ;;
  esac
  shift
done

work=$(mktemp -d)
cleanup() {
  status=$?
  [ -n "${s3_pid:-}" ] && kill "$s3_pid" 2>/dev/null || true
  [ -n "${head_pid:-}" ] && kill "$head_pid" 2>/dev/null || true
  [ -z "$keep" ] && rm -rf "$work"
  exit $status
}
trap cleanup EXIT INT TERM

echo "building..."
cargo build -q -p slate-slatedb --example s3_server
cargo build -q -p slate-serverd --bin slate-serverd

# --- the bucket -----------------------------------------------------------
#
# Wait for `LISTENING` rather than polling the port: the server prints it once
# bound and before it serves, which closes the race where a connection arrives
# between bind and accept.
echo "starting the object store..."
cargo run -q -p slate-slatedb --example s3_server -- --bucket slate-orm \
  > "$work/s3.env" 2> "$work/s3.log" &
s3_pid=$!
for _ in $(seq 100); do
  grep -q '^LISTENING ' "$work/s3.env" 2>/dev/null && break
  sleep 0.2
done
if ! grep -q '^LISTENING ' "$work/s3.env"; then
  echo "the S3 server never said it was listening; its log:" >&2
  cat "$work/s3.log" >&2
  exit 1
fi
# Everything after the LISTENING line is `NAME=value`, which is how the
# server hands over its endpoint and credentials.
set -a
. "$(grep -v '^LISTENING ' "$work/s3.env" > "$work/s3.sh"; echo "$work/s3.sh")"
set +a
echo "  bucket $SLATE_S3_BUCKET at $SLATE_S3_ENDPOINT"

# --- the head node --------------------------------------------------------
echo "starting the head node..."
"$root/target/debug/slate-serverd" --config "$here/head.toml" \
  > "$work/head.log" 2>&1 &
head_pid=$!
for _ in $(seq 300); do
  grep -q '^LISTENING ' "$work/head.log" 2>/dev/null && break
  sleep 0.2
done
if ! grep -q '^LISTENING ' "$work/head.log"; then
  echo "slate-serverd never said it was listening; its log:" >&2
  cat "$work/head.log" >&2
  exit 1
fi
address=$(grep '^LISTENING ' "$work/head.log" | head -1 | cut -d' ' -f2)
echo "  serving on $address"

# --- the data -------------------------------------------------------------
export PYTHONPATH="$root/clients/python/src${PYTHONPATH:+:$PYTHONPATH}"
echo "loading the January 2024 sample..."
python3 "$here/load.py" --address "$address" ${trips:+--trips "$trips"}

if [ -n "$keep" ]; then
  echo
  echo "  head node   $address"
  echo "  bucket      $SLATE_S3_ENDPOINT/$SLATE_S3_BUCKET"
  echo "  logs        $work"
  echo
  echo "  PYTHONPATH=$root/clients/python/src python3 $here/check.py --address $address"
  echo
  echo "running until interrupted."
  wait "$head_pid"
  exit 0
fi

echo
python3 "$here/check.py" --address "$address" ${trips:+--trips "$trips"}
