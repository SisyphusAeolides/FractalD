#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-device.XXXXXX")
daemon_pid=
marker="/dev/shm/fractald-device-$$"

cleanup() {
    rm -f "$marker"
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

mkdir -p "$root/run" "$root/state" "$root/services"
rm -f "$marker"

device_stem=$(
    "$project_dir/target/debug/systemd-escape" --path "$marker"
)
device_unit="$device_stem.device"
cat >"$root/services/device-dependent.service" <<UNIT
[Unit]
BindsTo=$device_unit
After=$device_unit

[Service]
Type=oneshot
ExecStart=/bin/sh -c "test -e '$marker' && printf ready > '$root/ready'"
RemainAfterExit=yes
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!

attempt=0
while ! FRACTALD_RUNTIME_DIR="$root/run" FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" status >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done

start_status="$root/start.status"
(
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/systemctl" start device-dependent.service >"$root/start.log" 2>&1
    printf '%s\n' "$?" >"$start_status"
) &
start_pid=$!

sleep 0.15
[ ! -e "$root/ready" ]
printf present >"$marker"
set +e
wait "$start_pid"
start_exit=$?
set -e
[ "$start_exit" -eq 0 ] || {
    cat "$root/start.log" >&2
    cat "$root/daemon.log" >&2
    exit "$start_exit"
}
[ "$(cat "$start_status")" = 0 ]
[ -f "$root/ready" ]

rm -f "$marker"
attempt=0
while FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/systemctl" is-active device-dependent.service \
    >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || {
        "$project_dir/target/debug/fractalctl" status device-dependent.service >&2 || true
        exit 1
    }
    sleep 0.02
done

echo "device unit: ok"
