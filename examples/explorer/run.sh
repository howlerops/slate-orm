#!/usr/bin/env bash
#
# Start the whole demo: one head node, three adapters, one frontend.
#
# The head node is shared on purpose. Three SDKs against three databases would
# prove nothing; three SDKs against one is the demo, and it is also what makes
# `conformance/` meaningful — a disagreement between adapters cannot be blamed
# on the data.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
run="${TMPDIR:-/tmp}/slate-explorer"
mkdir -p "$run"

HEAD_ADDR=127.0.0.1:7421
GO_ADDR=127.0.0.1:7431
NODE_ADDR=127.0.0.1:7432
PY_ADDR=127.0.0.1:7433

pids=()
cleanup() {
  for pid in "${pids[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# Wait for a line on a log, rather than polling a port: the daemon prints
# `LISTENING <addr>` once its listener is bound and before it serves, which
# closes the race where a connection arrives between bind and accept.
await() {
  local log="$1" what="$2" limit="${3:-60}"
  for _ in $(seq "$limit"); do
    grep -q "LISTENING" "$log" 2>/dev/null && return 0
    sleep 1
  done
  echo "$what never said it was listening; its log:" >&2
  tail -20 "$log" >&2
  return 1
}

echo "building slate-serverd..."
(cd "$root" && cargo build -q -p slate-serverd --bin slate-serverd)

echo "starting the head node on $HEAD_ADDR"
"$root/target/debug/slate-serverd" --config "$here/head.toml" > "$run/head.log" 2>&1 &
pids+=($!)
await "$run/head.log" "the head node"

echo "seeding"
(cd "$here/backends/go" && go run . --head "$HEAD_ADDR" --seed)

echo "starting the go adapter on $GO_ADDR"
(cd "$here/backends/go" && go run . --head "$HEAD_ADDR" --listen "$GO_ADDR") > "$run/go.log" 2>&1 &
pids+=($!)

echo "starting the node adapter on $NODE_ADDR"
(cd "$here/backends/node" && npm start --silent -- --head "$HEAD_ADDR" --listen "$NODE_ADDR") > "$run/node.log" 2>&1 &
pids+=($!)

echo "starting the python adapter on $PY_ADDR"
(cd "$here/backends/python" && python3 -m adapter --head "$HEAD_ADDR" --listen "$PY_ADDR") > "$run/python.log" 2>&1 &
pids+=($!)

await "$run/go.log" "the go adapter"
await "$run/node.log" "the node adapter"
await "$run/python.log" "the python adapter"

echo
echo "  go         http://$GO_ADDR"
echo "  node       http://$NODE_ADDR"
echo "  python     http://$PY_ADDR"
echo "  logs       $run"
echo

if [ "${1:-}" = "--headless" ]; then
  echo "adapters are up; ctrl-c to stop"
  wait
fi

echo "starting the frontend"
(cd "$here/web" && npm run dev)
