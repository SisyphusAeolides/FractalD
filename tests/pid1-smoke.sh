#!/bin/sh
set -eu

ctl=${FRACTALCTL_BIN:-/usr/bin/fractalctl}
runtime=${FRACTALD_RUNTIME_DIR:-/run/fractald}

fail() {
    echo "FractalD PID1 smoke: FAIL: $*" >&2
    exit 1
}

[ "$(cat /proc/1/comm)" = fractald ] || fail 'PID1 is not FractalD'
readlink /proc/1/exe | grep -q '/fractald$' || fail 'PID1 executable is not fractald'

status=$(FRACTALD_RUNTIME_DIR="$runtime" "$ctl" status) || fail 'control status failed'
echo "$status"
echo "$status" | grep -Eq 'pid(=|[[:space:]])1([,)]|$)' ||
    fail 'control status did not report PID 1'

FRACTALD_RUNTIME_DIR="$runtime" "$ctl" list | grep -Fx 'boot' >/dev/null ||
    fail 'native boot profile is not loaded'
FRACTALD_RUNTIME_DIR="$runtime" "$ctl" start boot >/dev/null ||
    fail 'native boot profile could not be started'

echo 'FractalD PID1 smoke: PASS'
