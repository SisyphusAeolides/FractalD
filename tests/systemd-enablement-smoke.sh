#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-enablement.XXXXXX")
daemon_pid=

cleanup() {
    if [ -n "$daemon_pid" ]; then
        run_fractalctl stop >/dev/null 2>&1 || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT HUP INT TERM

mkdir -p \
    "$root/services/multi-user.target.wants" \
    "$root/services/multi-user.target.requires" \
    "$root/run" \
    "$root/state"

cat > "$root/services/demo.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT
cat > "$root/services/required.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT
cat > "$root/services/disabled.service" <<'UNIT'
[Service]
Type=oneshot
ExecStart=/bin/true
UNIT
ln -s ../demo.service "$root/services/multi-user.target.wants/demo.service"
ln -s ../required.service "$root/services/multi-user.target.requires/required.service"
ln -s /dev/null "$root/services/multi-user.target.wants/disabled.service"
ln -s /dev/null "$root/services/masked.service"

run_systemctl() {
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/systemctl" "$@"
}

run_fractalctl() {
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" "$@"
}

list=$(run_systemctl --no-legend list-unit-files demo.service required.service disabled.service masked.service)
printf '%s\n' "$list" | grep -F 'demo.service	enabled' >/dev/null
printf '%s\n' "$list" | grep -F 'required.service	enabled' >/dev/null
printf '%s\n' "$list" | grep -F 'disabled.service	disabled' >/dev/null
printf '%s\n' "$list" | grep -F 'masked.service	masked' >/dev/null

test "$(run_systemctl is-enabled demo.service)" = enabled
test "$(run_systemctl is-enabled demo)" = enabled
test "$(run_systemctl is-enabled required.service)" = enabled
test "$(run_fractalctl is-enabled demo.service)" = enabled

if run_systemctl is-enabled disabled.service >/dev/null 2>&1; then
    echo 'disabled relationship was reported as enabled' >&2
    exit 1
fi

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!
ready=false
attempt=0
while [ "$attempt" -lt 100 ]; do
    if run_fractalctl status >/dev/null 2>&1; then
        ready=true
        break
    fi
    attempt=$((attempt + 1))
    sleep 0.02
done
test "$ready" = true
run_fractalctl status demo.service | grep -Eq ': (active|running)' >/dev/null
run_fractalctl status required.service | grep -Eq ': (active|running)' >/dev/null

echo 'standard enablement: PASS'
