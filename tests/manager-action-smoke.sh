#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-manager-action.XXXXXX")
daemon_pid=

cleanup() {
    if [ -n "$daemon_pid" ]; then
        FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
        kill "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT INT TERM

mkdir -p "$root/run" "$root/state" "$root/services"
cat >"$root/services/success-action.service" <<'UNIT'
[Unit]
SuccessAction=exit

[Service]
Type=oneshot
ExecStart=/bin/true
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!

attempt=0
while ! FRACTALD_RUNTIME_DIR="$root/run" FRACTALD_STATE_DIR="$root/state" FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" status >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done

set +e
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" start success-action.service >/dev/null 2>&1
set -e

attempt=0
while kill -0 "$daemon_pid" 2>/dev/null; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 200 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done
wait "$daemon_pid"
daemon_pid=

[ ! -e "$root/run/fractald.pid" ]
[ ! -e "$root/run/fractald.sock" ]
echo "manager actions: ok"
