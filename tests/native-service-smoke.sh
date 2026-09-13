#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'if [ -n "${daemon_pid:-}" ]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; fi; rm -rf "$root"' EXIT

mkdir -p "$root/services"
cat >"$root/services/demo.svc" <<'SERVICE'
[service]
description=Native smoke service
kind=oneshot
exec=/bin/sh -c "printf started > %t/native-service.marker"
remain_after_exit=true
SERVICE

export FRACTALD_SERVICE_DIR="$root/services"
export FRACTALD_STATE_DIR="$root/state"
export FRACTALD_RUNTIME_DIR="$root/runtime"
export FRACTALD_BIN="$project_dir/target/debug/fractald"

"$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!
for _ in $(seq 1 100); do
    if [ -S "$root/runtime/control.sock" ]; then
        break
    fi
    sleep 0.02
done
test -S "$root/runtime/control.sock"

"$project_dir/target/debug/fractalctl" enable demo
"$project_dir/target/debug/fractalctl" start demo
for _ in $(seq 1 100); do
    if [ -f "$root/runtime/native-service.marker" ]; then
        break
    fi
    sleep 0.02
done
test -f "$root/runtime/native-service.marker"
"$project_dir/target/debug/fractalctl" status demo | grep -F 'demo: active' >/dev/null
"$project_dir/target/debug/fractalctl" stop demo >/dev/null
"$project_dir/target/debug/fractalctl" disable demo >/dev/null
"$project_dir/target/debug/fractalctl" stop >/dev/null
unset daemon_pid

echo 'native service lifecycle: PASS'
