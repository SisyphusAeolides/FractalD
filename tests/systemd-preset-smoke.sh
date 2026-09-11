#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-preset.XXXXXX")
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

mkdir -p "$root/services" "$root/run" "$root/state" "$root/presets" "$root/empty"

cat >"$root/services/demo.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT
cat >"$root/services/disabled.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT
cat >"$root/services/ignored.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT

cat >"$root/presets/00-local.preset" <<'PRESET'
# The first matching rule wins.
enable demo.service
ignore ignored.service
PRESET
cat >"$root/presets/99-default.preset" <<'PRESET'
disable *
PRESET

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
    preset_directory=${FRACTALD_PRESET_DIR:-$root/presets}
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    FRACTALD_PRESET_DIR="$preset_directory" \
    "$project_dir/target/debug/systemctl" "$@"
}

run_systemctl preset demo.service disabled.service ignored.service
test "$(run_systemctl is-enabled demo.service)" = enabled
if run_systemctl is-enabled disabled.service >/dev/null 2>&1; then
    echo 'preset unexpectedly enabled disabled.service' >&2
    exit 1
fi
if run_systemctl is-enabled ignored.service >/dev/null 2>&1; then
    echo 'preset changed ignored.service' >&2
    exit 1
fi

run_systemctl enable disabled.service >/dev/null
run_systemctl --preset-mode=disable-only preset disabled.service
if run_systemctl is-enabled disabled.service >/dev/null 2>&1; then
    echo 'disable-only preset did not disable disabled.service' >&2
    exit 1
fi

run_systemctl --preset-mode=enable-only preset disabled.service
if run_systemctl is-enabled disabled.service >/dev/null 2>&1; then
    echo 'enable-only preset applied a disable rule' >&2
    exit 1
fi

FRACTALD_PRESET_DIR="$root/empty" run_systemctl preset disabled.service
test "$(run_systemctl is-enabled disabled.service)" = enabled

echo 'systemd preset compatibility: PASS'
