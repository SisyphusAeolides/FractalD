#!/bin/sh
set -eu

binary_directory=${FRACTALD_TEST_BIN_DIR:-/usr/local/libexec/fractald-pid1-test}
log_directory=${FRACTALD_TEST_LOG_DIR:-/root/fractald-pid1-test}
/usr/bin/install -d -m 0755 "$log_directory"
/usr/bin/printf '%s\n' 'FractalD PID1 wrapper entered' >"$log_directory/boot.log"
/usr/bin/sync
export FRACTALD_SERVICE_DIR=/etc/fractald/pid1-test
export PATH=/usr/local/libexec/fractald-pid1-test:/usr/local/bin:/usr/bin:/usr/sbin:/bin:/sbin
export FRACTALD_RUNTIME_DIR=/run/fractald-pid1
export FRACTALD_STATE_DIR=/run/fractald-pid1-state
export FRACTALD_BOOT_TARGET=pid1.target
export FRACTALD_STORAGE_ENABLE=1
export FRACTALD_STORAGE_FSTAB=/etc/fstab
export FRACTALD_RUSTYBOX="$binary_directory/rustybox"
auto_check=0
for word in $(/usr/bin/cat /proc/cmdline 2>/dev/null); do
    [ "$word" = fractald.auto-check ] && auto_check=1
done
if [ "$auto_check" -eq 1 ]; then
    (
        /usr/bin/sleep 8
        /usr/bin/printf '%s\n' 'FractalD automatic PID1 checks: start' >/dev/console
        "$binary_directory/smoke"
        "$binary_directory/storage-matrix"
        "$binary_directory/storage-recovery"
        /usr/bin/printf '%s\n' 'FractalD automatic PID1 checks: PASS' >/dev/console
    ) >/dev/console 2>&1 &
fi
exec "$binary_directory/fractald" >>"$log_directory/daemon.log" 2>&1
