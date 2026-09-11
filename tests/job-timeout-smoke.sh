#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-job-timeout.XXXXXX")
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
cat >"$root/services/job-timeout-dependency.service" <<'UNIT'
[Service]
Type=notify
ExecStart=/bin/sh -c 'sleep 30'
TimeoutStartSec=5s
UNIT
cat >"$root/services/job-timeout.target" <<'UNIT'
[Unit]
Requires=job-timeout-dependency.service
JobTimeoutSec=50ms
JobTimeoutAction=exit

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

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" start job-timeout.target >/dev/null

attempt=0
while kill -0 "$daemon_pid" 2>/dev/null; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 200 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done
wait "$daemon_pid"
daemon_pid=

grep -Fq 'start job for job-timeout.target timed out' "$root/daemon.log"
[ ! -e "$root/run/fractald.pid" ]
[ ! -e "$root/run/fractald.sock" ]
echo 'job timeouts: ok'
