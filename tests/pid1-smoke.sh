#!/bin/sh
set -eu

binary_directory=${FRACTALD_TEST_BIN_DIR:-/usr/local/libexec/fractald-pid1-test}
runtime=${FRACTALD_RUNTIME_DIR:-/run/fractald-pid1}
state=${FRACTALD_STATE_DIR:-/run/fractald-pid1-state}
ctl=${FRACTALCTL_BIN:-$binary_directory/fractalctl}
systemctl=${SYSTEMCTL_BIN:-$binary_directory/systemctl}
log=${FRACTALD_PID1_TEST_LOG:-/root/fractald-pid1-test/test.log}

/usr/bin/install -d -m 0755 "$runtime"
exec >"$log" 2>&1

fail() {
    echo "pid1 smoke: FAIL: $*" >&2
    exit 1
}

wait_for_file() {
    file=$1
    attempts=${2:-100}
    while [ "$attempts" -gt 0 ]; do
        [ -e "$file" ] && return 0
        /usr/bin/sleep 0.1
        attempts=$((attempts - 1))
    done
    fail "timed out waiting for $file"
}

wait_for_service_state() {
    service=$1
    expected=$2
    attempts=${3:-100}
    while [ "$attempts" -gt 0 ]; do
        if output=$(FRACTALD_RUNTIME_DIR="$runtime" "$ctl" status "$service" 2>/dev/null); then
            echo "$output"
            echo "$output" | /usr/bin/grep -Eq "^${service}: ${expected}([[:space:]]|$)" && return 0
        fi
        /usr/bin/sleep 0.1
        attempts=$((attempts - 1))
    done
    fail "$service did not reach state=$expected"
}

echo 'checking actual PID1'
[ "$(/usr/bin/cat /proc/1/comm)" = fractald ] || fail 'PID1 is not fractald'
pid1_exe=$(/usr/bin/readlink /proc/1/exe)
echo "PID1 executable: $pid1_exe"
echo "$pid1_exe" | /usr/bin/grep -q '/fractald$' || fail 'PID1 executable path is unexpected'

status=$(FRACTALD_RUNTIME_DIR="$runtime" "$ctl" status)
echo "$status"
echo "$status" | /usr/bin/grep -Eq 'pid(=|[[:space:]])1([,)]|$)' || fail 'control status did not report PID 1'

FRACTALD_RUNTIME_DIR="$runtime" "$systemctl" is-system-running
FRACTALD_RUNTIME_DIR="$runtime" "$ctl" list | /usr/bin/grep -Fx 'pid1.target'
FRACTALD_RUNTIME_DIR="$runtime" "$ctl" list | /usr/bin/grep -Fx 'pid1-ready.service'

wait_for_file /run/fractald-pid1/ready
wait_for_file /run/fractald-pid1/chaos-check
wait_for_file /run/fractald-pid1/orphan-complete 150
wait_for_file /run/fractald-pid1/fractald-pid1-dynamic/ready
wait_for_file /run/fractald-pid1/cpu-quota
wait_for_file /run/fractald-pid1/device-policy
wait_for_file /run/fractald-pid1/private-users
dynamic_identity=$(/usr/bin/cat /run/fractald-pid1/fractald-pid1-dynamic/identity)
dynamic_uid=${dynamic_identity%%:*}
dynamic_gid=${dynamic_identity#*:}
[ "$dynamic_uid" -ge 61184 ] && [ "$dynamic_uid" -le 65519 ] || fail 'DynamicUser UID is outside the transient range'
[ "$dynamic_gid" -ge 61184 ] && [ "$dynamic_gid" -le 65519 ] || fail 'DynamicUser GID is outside the transient range'
[ "$dynamic_uid" -eq "$dynamic_gid" ] || fail 'DynamicUser did not assign matching UID and GID'
dynamic_groups=$(/usr/bin/cat /run/fractald-pid1/fractald-pid1-dynamic/supplementary-groups)
[ -z "$dynamic_groups" ] || fail 'DynamicUser inherited supplementary groups'
echo "DynamicUser identity: $dynamic_uid:$dynamic_gid"
[ "$(/usr/bin/cat /run/fractald-pid1/chaos-check)" = 'chaos: ok' ] || fail 'Chaos self-check marker is wrong'
[ "$(/usr/bin/cat /run/fractald-pid1/cpu-quota)" = '50000 100000' ] || fail 'CPUQuota was not applied to cpu.max'
echo "$(/usr/bin/cat /run/fractald-pid1/device-policy)" | /usr/bin/grep -q '^device-policy: blocked /dev/' || fail 'DevicePolicy did not block a block device'
[ "$(/usr/bin/cat /run/fractald-pid1/private-users)" = 'private-users: ok' ] || fail 'PrivateUsers did not create a private user namespace'
[ -d /run/fractald-pid1/rustybox-created ] || fail 'RustyBox mkdir did not run'
[ "$(/usr/bin/cat /run/fractald-pid1/toolbox-output)" = 'pid1-toolbox-ready' ] || fail 'RustyBox echo did not run'

orphan_pid=$(/usr/bin/cat /run/fractald-pid1/orphan-pid)
attempts=100
while [ "$attempts" -gt 0 ] && [ -e "/proc/$orphan_pid" ]; do
    /usr/bin/sleep 0.1
    attempts=$((attempts - 1))
done
[ ! -e "/proc/$orphan_pid" ] || fail "orphan child $orphan_pid remains after exit"

FRACTALD_RUNTIME_DIR="$runtime" "$ctl" start pid1-control.service
wait_for_file /run/fractald-pid1/control-started
wait_for_service_state pid1-control.service running
FRACTALD_RUNTIME_DIR="$runtime" "$systemctl" show -p MainPID --value pid1-control.service | /usr/bin/grep -Eq '^[1-9][0-9]*$'
FRACTALD_RUNTIME_DIR="$runtime" "$ctl" stop pid1-control.service
wait_for_file /run/fractald-pid1/control-stopped
wait_for_service_state pid1-control.service exited

FRACTALD_RUNTIME_DIR="$runtime" FRACTALD_STATE_DIR="$state" "$ctl" events |
    /usr/bin/grep -q 'pid1-control.service'
echo 'pid1 smoke: PASS'
