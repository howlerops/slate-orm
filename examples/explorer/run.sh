#!/usr/bin/env bash
#
# Start the whole demo: one head node, three adapters, one frontend.
#
# The head node is shared on purpose. Three SDKs against three databases would
# prove nothing; three SDKs against one is the demo, and it is also what makes
# `conformance/` meaningful — a disagreement between adapters cannot be blamed
# on the data.
#
#   ./run.sh                 the stack and the frontend
#   ./run.sh --headless      the stack, no frontend; ctrl-c to stop
#   ./run.sh --conformance   the stack on free ports, the conformance suite
#                            against it, then tear it down. Exit status is the
#                            suite's.
#   ./run.sh --e2e           the same, plus the frontend, driven in a browser
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

mode="${1:-}"

# Fixed ports for the interactive modes, because the frontend is configured
# with them and a human reads them off the terminal. Free ports for
# `--conformance`, which nobody reads and which must be able to run while a
# demo stack is already up -- and, more to the point, must not fail with
# "address already in use" and be mistaken for the SDKs disagreeing.
port() {
  if [ "$mode" = --conformance ] || [ "$mode" = --e2e ]; then
    python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'
  else
    echo "$1"
  fi
}

HEAD_ADDR="${SLATE_HEAD_ADDR:-127.0.0.1:$(port 7421)}"
GO_ADDR="${SLATE_GO_ADDR:-127.0.0.1:$(port 7431)}"
NODE_ADDR="${SLATE_NODE_ADDR:-127.0.0.1:$(port 7432)}"
PY_ADDR="${SLATE_PY_ADDR:-127.0.0.1:$(port 7433)}"
WEB_PORT="${SLATE_WEB_PORT:-$(port 7440)}"

# The frontend reads its three adapter URLs from the environment, defaulting to
# the demo's fixed ports. Exported here rather than written into a file so that
# a run on free ports needs no `.env` to clean up afterwards -- and so that the
# UI and the conformance suite are pointed at one set of addresses by one line.
export VITE_GO_URL="http://$GO_ADDR"
export VITE_NODE_URL="http://$NODE_ADDR"
export VITE_PYTHON_URL="http://$PY_ADDR"

run="${TMPDIR:-/tmp}/slate-explorer-${HEAD_ADDR##*:}"
mkdir -p "$run"

# The head node's own config names its listen address, so a run on a free port
# needs a copy. Only that one line differs; everything else -- the tables, the
# grants, the two RLS policies and the comments explaining why there are two --
# is the file in the repository.
config="$run/head.toml"
sed "s|^address = .*|address = \"$HEAD_ADDR\"|" "$here/head.toml" > "$config"
grep -q "address = \"$HEAD_ADDR\"" "$config" ||
  { echo "head.toml has no [listen] address line to rewrite" >&2; exit 1; }

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
"$root/target/debug/slate-serverd" --config "$config" > "$run/head.log" 2>&1 &
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

if [ "$mode" = --conformance ]; then
  # `exec` would lose the trap, and the trap is what stops four processes.
  status=0
  python3 "$here/conformance/conformance.py" \
    --go "http://$GO_ADDR" --node "http://$NODE_ADDR" --python "http://$PY_ADDR" \
    "${@:2}" || status=$?
  exit "$status"
fi

if [ "$mode" = --headless ]; then
  echo "adapters are up; ctrl-c to stop"
  wait
fi

if [ "$mode" = --e2e ]; then
  echo "starting the frontend on 127.0.0.1:$WEB_PORT"
  (cd "$here/web" && npm run dev -- --port "$WEB_PORT" --strictPort) > "$run/web.log" 2>&1 &
  pids+=($!)
  # Vite prints `Local:` once it is serving. Same reasoning as `await`: a port
  # poll answers yes before the first module has been transformed.
  for _ in $(seq 90); do
    grep -q "Local:" "$run/web.log" 2>/dev/null && break
    sleep 1
  done
  grep -q "Local:" "$run/web.log" || { echo "vite never served:" >&2; tail -20 "$run/web.log" >&2; exit 1; }

  status=0
  (cd "$here/web" && node e2e/explorer.mjs "http://127.0.0.1:$WEB_PORT") || status=$?
  exit "$status"
fi

echo "starting the frontend on 127.0.0.1:$WEB_PORT"
(cd "$here/web" && npm run dev -- --port "$WEB_PORT" --strictPort)
