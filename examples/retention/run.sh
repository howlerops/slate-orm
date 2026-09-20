#!/bin/sh
# Start a head node, retire a row, sweep it, and prove it is gone.
#
# `purge.py` beside this file is the example a deployment copies. An example
# nothing runs is this repository's most familiar defect — a workflow that
# never fired, decoders that compiled and never ran — so this runs it, against
# a real server, and fails if the row survives.
#
# A `local` backend rather than the explorer's S3: this checks that the script
# calls the RPC correctly, which does not depend on where the bytes land, and
# it means the harness needs no container and no network.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(CDPATH= cd -- "$here/../.." && pwd)
work=$(mktemp -d)

# Wait for the node before removing its directory, and never let the cleanup
# decide the exit code. Both were real: `rm -rf` raced the dying process and
# failed with "Directory not empty", and because that was the trap's last
# command it became the script's status — a passing run reported failure.
cleanup() {
  status=$?
  if [ -n "${head_pid:-}" ]; then
    kill "$head_pid" 2>/dev/null || true
    wait "$head_pid" 2>/dev/null || true
  fi
  rm -rf "$work" 2>/dev/null || true
  exit "$status"
}
trap cleanup EXIT

serverd=${SLATE_SERVERD:-}
if [ -z "$serverd" ]; then
  echo "building slate-serverd..."
  cargo build -q -p slate-serverd --bin slate-serverd
  serverd="$root/target/debug/slate-serverd"
elif [ ! -x "$serverd" ]; then
  # A path that is set and missing is a hard error, never a silent fall back
  # to building: the point of the variable is to test *that* binary.
  echo "SLATE_SERVERD=$serverd is not executable" >&2
  exit 1
fi

mkdir -p "$work/store"
sed "s|{DIR}|$work/store|" "$here/head.toml" > "$work/head.toml"

"$serverd" --config "$work/head.toml" > "$work/log" 2>&1 &
head_pid=$!
for _ in $(seq 300); do
  grep -q '^LISTENING ' "$work/log" 2>/dev/null && break
  sleep 0.2
done
if ! grep -q '^LISTENING ' "$work/log"; then
  echo "slate-serverd never said it was listening; its log:" >&2
  cat "$work/log" >&2
  exit 1
fi
address=$(sed -n 's/^LISTENING //p' "$work/log" | head -1)
echo "head node on $address"

export PYTHONPATH="$root/clients/python/src:$here${PYTHONPATH:+:$PYTHONPATH}"
python3 "$here/seed.py" --address "$address"
python3 "$here/purge.py" \
  --address "$address" \
  --schema schema \
  --table notes \
  --older-than 0s \
  --role retention \
  --once
python3 "$here/seed.py" --address "$address" --verify

echo
echo "the retention sweep erased the retired row"
