#!/usr/bin/env bash
#
# N single writes against one batch of N, once per client, against one head
# node. See README.md for what this is for and what it deliberately does not
# do.
#
#   ./run.sh                 100 rows, five runs
#   ./run.sh --rows 200      a different N
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"

rows=100
runs=5
while [ $# -gt 0 ]; do
  case "$1" in
    --rows) rows="$2"; shift 2 ;;
    --runs) runs="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# A free port below the ephemeral range, for the reason `examples/explorer`
# spells out at length: a port drawn from the ephemeral range can be taken by
# an outgoing connection between this script releasing it and the head node
# binding it, and the failure reads as a broken benchmark.
port="$(python3 - <<'PORTS'
import random, socket
try:
    with open("/proc/sys/net/ipv4/ip_local_port_range") as handle:
        ephemeral_low = int(handle.read().split()[0])
except (OSError, ValueError):
    ephemeral_low = 32768
while True:
    candidate = random.randint(10000, min(max(ephemeral_low - 1, 10100), ephemeral_low - 1))
    probe = socket.socket()
    probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    try:
        probe.bind(("127.0.0.1", candidate))
    except OSError:
        continue
    finally:
        probe.close()
    print(candidate)
    break
PORTS
)"
addr="127.0.0.1:$port"

# The same rule the client harnesses use: a prebuilt binary if one is named,
# built from source otherwise, and a hard error if the path is set and missing
# — never a silent fall back to building, which would measure a different
# binary from the one the caller asked for.
if [ -n "${SLATE_SERVERD:-}" ]; then
  [ -x "$SLATE_SERVERD" ] || { echo "SLATE_SERVERD=$SLATE_SERVERD is not executable" >&2; exit 1; }
  serverd="$SLATE_SERVERD"
else
  (cd "$root" && cargo build -q -p slate-serverd)
  serverd="$root/target/debug/slate-serverd"
fi

run="${TMPDIR:-/tmp}/slate-batchbench-$port"
mkdir -p "$run"
config="$run/head.toml"
sed "s|^address = .*|address = \"$addr\"|" "$here/head.toml" > "$config"

set -m
pids=()
cleanup() {
  set +m
  for pid in "${pids[@]:-}"; do
    kill -TERM -- "-$pid" 2>/dev/null || kill -TERM "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

"$serverd" --config "$config" > "$run/head.log" 2>&1 &
pids+=($!)
for _ in $(seq 60); do
  grep -q LISTENING "$run/head.log" 2>/dev/null && break
  sleep 1
done
grep -q LISTENING "$run/head.log" || { echo "the head node did not start:"; cat "$run/head.log"; exit 1; }

printf 'client\trows\tsingle µs/row\tmin\tmax\tbatch µs/row\tmin\tmax\tratio\n'

(cd "$root/clients/python" && PYTHONPATH=src python3 "$here/bench.py" \
  --address "$addr" --rows "$rows" --runs "$runs")

(cd "$here/go" && go run . --address "$addr" --rows "$rows" --runs "$runs")

# The node arm resolves `@slate-orm/client` to a symlink into
# `clients/typescript` and imports its *built* `dist/`. Nothing rebuilds that
# on its own — `npm ci` in the client does not — so without this the arm fails
# to resolve the module at all on a clean checkout, which is exactly how it
# failed in CI while passing here on a tree that happened to have a `dist/`
# lying around. `examples/explorer/run.sh` does the same thing for the same
# reason and says so at length.
(cd "$root/clients/typescript" && npm run build --silent)
(cd "$here/node" && npm install --silent >/dev/null 2>&1 && npm run --silent bench -- \
  --address "$addr" --rows "$rows" --runs "$runs")
