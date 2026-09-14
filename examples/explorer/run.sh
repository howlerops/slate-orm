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
# `--conformance` and `--e2e`, which nobody reads and which must be able to run
# while a demo stack is already up -- and, more to the point, must not fail
# with "address already in use" and be mistaken for the SDKs disagreeing.
#
# Ports are chosen from *below* the kernel's ephemeral range, which is the
# whole point and took a CI failure to learn.
#
# The obvious implementation asks the kernel for a free port with
# `bind(("127.0.0.1", 0))`, reads the number and closes the socket. That draws
# from the ephemeral range -- 32768-60999 on Linux by default -- and so does
# every *outgoing connection* every service here makes. So between this script
# releasing a port and the service binding it, the Go adapter dialling the head
# node can be assigned that exact number, and the service dies with
# `EADDRINUSE` on a port the harness had just declared free.
#
# That is not a theory about what might happen: the demo job failed this way on
# port 38721, inside the ephemeral range, and it read as a broken demo rather
# than a broken runner.
#
# Ports below the range are never handed out automatically, so nothing can take
# one from under us. Each is still checked before being offered, because
# something else on the machine may be *listening* there already.
free_ports() {
  python3 - <<'PORTS'
import random
import socket

# The floor of the ephemeral range, read rather than assumed: a container can
# be configured with a different one, and picking "below 32768" on a machine
# whose range starts at 15000 would reintroduce exactly the bug.
try:
    with open("/proc/sys/net/ipv4/ip_local_port_range") as handle:
        ephemeral_low = int(handle.read().split()[0])
except (OSError, ValueError):
    ephemeral_low = 32768

high = max(ephemeral_low - 1, 10100)
low = 10000

chosen = []
while len(chosen) < 5:
    candidate = random.randint(low, min(high, ephemeral_low - 1))
    if candidate in chosen:
        continue
    probe = socket.socket()
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", candidate))
    except OSError:
        continue  # somebody is listening there; try another
    finally:
        probe.close()
    chosen.append(candidate)

print(" ".join(str(port) for port in chosen))
PORTS
}

if [ "$mode" = --conformance ] || [ "$mode" = --e2e ]; then
  # `read`, not `set --`. `set --` replaces the script's own positional
  # parameters, which are forwarded to the conformance runner further down --
  # so the five port numbers arrived as command-line arguments and it refused
  # them. Caught on the first run after the change.
  IFS=' ' read -r HEAD_PORT GO_PORT NODE_PORT PY_PORT WEB <<PICKED
$(free_ports)
PICKED
else
  HEAD_PORT=7421 GO_PORT=7431 NODE_PORT=7432 PY_PORT=7433 WEB=7440
fi

HEAD_ADDR="${SLATE_HEAD_ADDR:-127.0.0.1:$HEAD_PORT}"
GO_ADDR="${SLATE_GO_ADDR:-127.0.0.1:$GO_PORT}"
NODE_ADDR="${SLATE_NODE_ADDR:-127.0.0.1:$NODE_PORT}"
PY_ADDR="${SLATE_PY_ADDR:-127.0.0.1:$PY_PORT}"
WEB_PORT="${SLATE_WEB_PORT:-$WEB}"

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

# `SLATE_SERVERD` names a prebuilt daemon, as it does for the three client
# suites. CI builds it once and hands the same binary to every job that needs
# one; here it also means the demo starts without a Rust toolchain present.
if [ -n "${SLATE_SERVERD:-}" ]; then
  [ -x "$SLATE_SERVERD" ] || { echo "SLATE_SERVERD=$SLATE_SERVERD is not executable" >&2; exit 1; }
  serverd="$SLATE_SERVERD"
  echo "using the slate-serverd at $serverd"
else
  echo "building slate-serverd..."
  (cd "$root" && cargo build -q -p slate-serverd --bin slate-serverd)
  serverd="$root/target/debug/slate-serverd"
fi

echo "starting the head node on $HEAD_ADDR"
"$serverd" --config "$config" > "$run/head.log" 2>&1 &
pids+=($!)
await "$run/head.log" "the head node"

echo "seeding"
(cd "$here/backends/go" && go run . --head "$HEAD_ADDR" --seed)

echo "starting the go adapter on $GO_ADDR"
(cd "$here/backends/go" && go run . --head "$HEAD_ADDR" --listen "$GO_ADDR") > "$run/go.log" 2>&1 &
pids+=($!)

# The node adapter resolves `@slate-orm/client` to a symlink into
# `clients/typescript`, and imports its *built* `dist/`. Nothing rebuilds that
# on its own, so a change to the client reaches the adapter only if somebody
# remembers -- and the symptom is an adapter running yesterday's client, which
# the conformance runner reports as the three SDKs disagreeing. It did, once.
echo "building the typescript client the node adapter links to"
(cd "$root/clients/typescript" && npm run build --silent)

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
  # `--host 127.0.0.1`, explicitly. Vite binds `localhost` by default, and on
  # a machine where that resolves to `::1` first it listens on IPv6 only —
  # nothing answers on 127.0.0.1, which is the address the e2e and the poll
  # below both use. It printed `Local: http://localhost:60087/` and the poll
  # timed out against it for ninety seconds. Binding the address we then ask
  # for removes the question.
  (cd "$here/web" && npm run dev -- --host 127.0.0.1 --port "$WEB_PORT" --strictPort) \
    > "$run/web.log" 2>&1 &
  pids+=($!)

  # Asked over HTTP rather than read out of the log.
  #
  # This used to grep the log for `Local:`, which vite prints when it is
  # serving. It does not print that: it prints `Local` and `:` with an ANSI
  # reset between them, so the literal string never appears — on a terminal, in
  # a pipe, anywhere colour is on. It passed locally because colour was off
  # there and on in CI, which is the most annoying shape a bug can have and the
  # reason this only surfaced once CI existed.
  #
  # An HTTP poll has no such problem, and is a better signal besides: it asks
  # the question the browser is about to ask.
  for _ in $(seq 90); do
    curl -fsS -o /dev/null "http://127.0.0.1:$WEB_PORT/" 2>/dev/null && break
    sleep 1
  done
  curl -fsS -o /dev/null "http://127.0.0.1:$WEB_PORT/" 2>/dev/null || {
    echo "vite never served on $WEB_PORT:" >&2
    tail -20 "$run/web.log" >&2
    exit 1
  }

  status=0
  (cd "$here/web" && node e2e/explorer.mjs "http://127.0.0.1:$WEB_PORT") || status=$?
  exit "$status"
fi

echo "starting the frontend on 127.0.0.1:$WEB_PORT"
(cd "$here/web" && npm run dev -- --host 127.0.0.1 --port "$WEB_PORT" --strictPort)
