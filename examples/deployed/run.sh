#!/bin/sh
# Stand the whole system up, load the workbench's dataset, and check it.
#
# Three processes and one bucket:
#
#   s3_server      a real S3 server on a real socket (`s3s` + `s3s-fs`)
#   slate-serverd  the head node, SlateDB on top of that bucket, two replicas
#   check.py       the Python SDK over gRPC, against a fold done in Python
#   go/check.go    the Go SDK, against the answers that fold produced
#   node/check.ts  the TypeScript SDK, likewise
#
# Then the head node is killed and a *new* one is started on the same bucket,
# and every check runs again. That is the part no in-process test can do: the
# LSM's recovery, the manifest, the writer lease, all through a real object
# store, with the first process gone.
#
# Nothing is mocked between the gRPC socket and the object store. What is *not*
# production-grade is the S3 implementation itself — see the note in
# `examples/s3_server.rs` and the `minio` job in CI, which covers that layer.
#
#   ./run.sh              start everything, load, check, restart, check, tear down
#   ./run.sh --keep       the same, but leave it running and print the address
#   ./run.sh --trips N    load only the first N trips, for a quick run
#   ./run.sh --python     only the Python check, for a quick pass
#   ./run.sh --no-restart skip the restart phase
#
# Every port is picked by the kernel, so this can run twice at once and cannot
# fail confusingly because something else holds 50051.

set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)

keep=
trips=
python_only=
restart=1
while [ $# -gt 0 ]; do
  case $1 in
    --keep) keep=1 ;;
    --trips) shift; trips=$1 ;;
    --python) python_only=1 ;;
    --no-restart) restart= ;;
    *)
      echo "usage: run.sh [--keep] [--trips N] [--python] [--no-restart]" >&2
      exit 2
      ;;
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
if [ -z "$python_only" ]; then
  # Built before anything starts, so a compile error is a compile error and
  # not a head node that came up and then had nothing to talk to it.
  (cd "$here/go" && go build -o "$work/go-check" .)
  # `@slate-orm/client` is a `file:` dependency on `clients/typescript`, and
  # what `node/src/check.ts` imports is that package's *built* `dist/`, which
  # an `npm install` does not produce -- it only symlinks the directory. So
  # the client is built here, first.
  #
  # Leaving this out passed locally for the worst possible reason: an earlier
  # run of `examples/explorer` had left a `dist/` behind, so the compile found
  # a client that this script never built and nothing said so. On a clean
  # checkout it is `TS2307: Cannot find module '@slate-orm/client'`, which is
  # how CI found it. `examples/explorer/run.sh` builds it for the neighbouring
  # reason -- there, a stale `dist/` means the node adapter runs yesterday's
  # client and the conformance runner reports three SDKs disagreeing.
  (cd "$root/clients/typescript" && npm install --silent && npm run build --silent)
  (cd "$here/node" && npm install --silent && npx tsc -p tsconfig.json)
fi

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
#
# A function, because it is started twice: once to load and check, and again
# after being killed, to prove the bucket is the system of record and the
# process is not.
start_head() {
  log=$1
  "$root/target/debug/slate-serverd" --config "$here/head.toml" > "$log" 2>&1 &
  head_pid=$!
  for _ in $(seq 300); do
    grep -q '^LISTENING ' "$log" 2>/dev/null && break
    sleep 0.2
  done
  if ! grep -q '^LISTENING ' "$log"; then
    echo "slate-serverd never said it was listening; its log:" >&2
    cat "$log" >&2
    exit 1
  fi
  address=$(grep '^LISTENING ' "$log" | head -1 | cut -d' ' -f2)
  # And it has to be the *leader*. A node that lost the campaign comes up
  # read-only — it did not open the writer, which is what makes starting at
  # all safe — and it would answer every read here perfectly while making the
  # restart prove nothing about the writer. Worse, `Freshness::Latest` has
  # nowhere to go on such a node and the pool refuses it outright, which is how
  # this was found.
  if ! grep -q 'leader$' "$log"; then
    echo "slate-serverd came up without the writer; its log:" >&2
    cat "$log" >&2
    exit 1
  fi
}

echo "starting the head node..."
start_head "$work/head.log"
echo "  serving on $address, with $(grep -c '^\[\[replicas\]\]' "$here/head.toml") replicas"

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

# --- the checks -----------------------------------------------------------
#
# Python first, and it writes the answers its fold produced; the other two read
# that file. The oracle is in one language on purpose — three decoders of one
# packed file would be three places to be wrong, and a common-mode error in
# them would agree with itself.
run_checks() {
  echo
  python3 "$here/check.py" --address "$address" ${trips:+--trips "$trips"} \
    --expect "$work/expected.json"
  if [ -z "$python_only" ]; then
    echo
    echo "the Go client, over the same socket:"
    "$work/go-check" --address "$address" --expect "$work/expected.json"
    echo
    echo "the TypeScript client, over the same socket:"
    (cd "$here/node" && node dist/check.js \
      --address "$address" --expect "$work/expected.json")
  fi
}

run_checks

if [ -n "$restart" ]; then
  echo
  echo "restarting the head node on the same bucket..."
  # SIGKILL rather than SIGTERM: a clean shutdown would flush, and what is
  # worth proving is that an acknowledged write is already in the bucket
  # without one.
  # One write, acknowledged, and then the kill on the next line. What that
  # proves is that an acknowledged write is in the bucket rather than in the
  # dead process's memory. It does *not* isolate `[storage] durability`: see
  # `probe.py`, which records the attempt to make it do that and why the claim
  # was withdrawn.
  python3 "$here/probe.py" --address "$address" --write
  kill -9 "$head_pid" 2>/dev/null || true
  wait "$head_pid" 2>/dev/null || true
  # The dead node's writer lease outlives it: nothing renews a lease for a
  # process that is gone, so a replacement has to wait the term out before it
  # can open the database. `[lease] term` in `head.toml` is three seconds for
  # exactly this, and four is that plus slack.
  #
  # Starting the new node immediately is what the first draft did. It came up
  # read-only, answered every read correctly, and failed the one check that
  # needs the writer — which is a much better failure than a restart that
  # silently proved nothing.
  echo "  waiting out the dead node's writer lease..."
  sleep 4
  start_head "$work/head-2.log"
  echo "  a new process, serving on $address"
  python3 "$here/probe.py" --address "$address" --verify
  # Every check again, not a subset: the recovered LSM has to answer the whole
  # query surface, not just a count. It costs a few seconds and proves the
  # planner, the grouper and the index still work over SSTs this process did
  # not write.
  run_checks
fi
