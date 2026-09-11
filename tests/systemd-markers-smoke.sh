#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-markers.XXXXXX")
daemon_pid=

cleanup() {
    if [ -n "$daemon_pid" ]; then
        FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$root/services" "$root/run" "$root/state"

cat >"$root/services/reloadable.service" <<'UNIT'
[Service]
Type=simple
ExecStart=/bin/sleep 30
ExecReload=/bin/true
UNIT
cat >"$root/services/restart-only.service" <<'UNIT'
[Service]
Type=simple
ExecStart=/bin/sleep 30
UNIT
cat >"$root/services/inactive.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
FRACTALD_STORAGE_ENABLE=0 \
FRACTALD_BOOT_TARGET= \
"$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!

ready=false
attempt=0
while [ "$attempt" -lt 100 ]; do
    if FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/fractalctl" status >/dev/null 2>&1; then
        ready=true
        break
    fi
    attempt=$((attempt + 1))
    sleep 0.02
done
test "$ready" = true

run_systemctl() {
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/systemctl" "$@"
}

run_systemctl start reloadable.service >/dev/null
run_systemctl set-property reloadable.service Markers=+needs-reload >/dev/null
test -f "$root/run/markers/reloadable.service.needs-reload"
run_systemctl reload-or-restart --marked >/dev/null
test ! -e "$root/run/markers/reloadable.service.needs-reload"
run_systemctl is-active reloadable.service >/dev/null

run_systemctl start restart-only.service >/dev/null
run_systemctl set-property restart-only.service Markers=+needs-reload >/dev/null
run_systemctl reload-or-restart --marked > /dev/null 2>"$root/reload-fallback.log"
test ! -s "$root/reload-fallback.log"
test ! -e "$root/run/markers/restart-only.service.needs-reload"
run_systemctl is-active restart-only.service >/dev/null

run_systemctl set-property inactive.service Markers=+needs-restart >/dev/null
run_systemctl try-reload-or-restart --marked >/dev/null
test -f "$root/run/markers/inactive.service.needs-restart"
if run_systemctl is-active inactive.service >/dev/null 2>&1; then
    echo 'try-reload-or-restart started an inactive marked unit' >&2
    exit 1
fi

echo 'systemd marker compatibility: PASS'
