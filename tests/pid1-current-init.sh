#!/bin/sh
set -eu

export PATH=/usr/bin:/usr/sbin:/bin:/sbin
export FRACTALD_SERVICE_DIR=/etc/fractald/pid1-test
export FRACTALD_RUNTIME_DIR=/run/fractald-pid1
export FRACTALD_STATE_DIR=/run/fractald-pid1-state
export FRACTALD_BOOT_TARGET=pid1.target
export FRACTALD_STORAGE_ENABLE=1
export FRACTALD_STORAGE_FSTAB=/etc/fstab
export FRACTALD_RUSTYBOX=/usr/bin/rustybox

/usr/bin/printf '%s\n' 'FractalD current PID1 entrypoint entered' >/dev/console
/usr/bin/mkdir -p /root/fractald-pid1-current
/usr/bin/printf '%s\n' 'FractalD current PID1 entrypoint entered' \
    >/root/fractald-pid1-current/boot.log

(
    /usr/bin/sleep 8
    /usr/bin/printf '%s\n' 'FractalD current PID1 checks: worker started' >/dev/console
    /usr/bin/printf '%s\n' 'FractalD current PID1 checks: start' >/dev/console
    [ "$(/usr/bin/cat /proc/1/comm)" = fractald ]
    /usr/bin/fractald self-check >/dev/console 2>&1
    status=$(/usr/bin/fractalctl status 2>&1) || {
        /usr/bin/printf '%s\n' "FractalD current PID1 status failed: $status" >/dev/console
        exit 1
    }
    /usr/bin/printf '%s\n' "$status" >/dev/console
    /usr/bin/printf '%s\n' "$status" | /usr/bin/grep -Eq 'pid(=|[[:space:]])1([,)]|$)'
    FRACTALD_TEST_BIN_DIR=/usr/local/libexec/fractald-pid1-test \
        FRACTALCTL_BIN=/usr/bin/fractalctl \
        /usr/local/libexec/fractald-pid1-test/storage-matrix >/dev/console 2>&1
    FRACTALD_TEST_BIN_DIR=/usr/local/libexec/fractald-pid1-test \
        FRACTALCTL_BIN=/usr/bin/fractalctl \
        FRACTALD_STORAGE_MATRIX_CHECK=/usr/local/libexec/fractald-pid1-test/storage-matrix \
        /usr/local/libexec/fractald-pid1-test/storage-recovery >/dev/console 2>&1
    /usr/bin/printf '%s\n' 'FractalD current PID1 checks: PASS' >/dev/console
) >/dev/console 2>&1 &

/usr/bin/printf '%s\n' 'FractalD current PID1 manager launch' >/dev/console
exec /usr/bin/fractald
