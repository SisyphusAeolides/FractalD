#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-journal-activation.XXXXXX")
daemon_pid=

cleanup() {
    exit_status=$?
    if [ "$exit_status" -ne 0 ] && [ -f "$root/daemon.log" ]; then
        cat "$root/daemon.log" >&2
    fi
    if [ -n "$daemon_pid" ]; then
        FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/units" \
            "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
        kill "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    rm -r "$root"
    exit "$exit_status"
}
trap cleanup EXIT INT TERM

mkdir -p "$root/units" "$root/run" "$root/state"
first_socket="$root/journal.sock"
second_socket="$root/dev-log.sock"

cat >"$root/units/systemd-journald.service" <<UNIT
[Unit]
Sockets=systemd-journald.socket systemd-journald-dev-log.socket

[Service]
Type=simple
ExecStart=$project_dir/target/debug/systemd-journald --system
StandardOutput=null
StandardError=inherit
UNIT

cat >"$root/units/systemd-journald.socket" <<UNIT
[Socket]
Service=systemd-journald.service
ListenDatagram=$first_socket
FileDescriptorName=journal
RemoveOnStop=yes
UNIT

cat >"$root/units/systemd-journald-dev-log.socket" <<UNIT
[Socket]
Service=systemd-journald.service
ListenDatagram=$second_socket
FileDescriptorName=dev-log
RemoveOnStop=yes
UNIT

cat >"$root/units/journald.target" <<UNIT
[Unit]
Wants=systemd-journald.socket systemd-journald-dev-log.socket
After=systemd-journald.socket systemd-journald-dev-log.socket
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/units" \
FRACTALD_LOG_DIR="$root/logs" \
FRACTALD_BOOT_TARGET= \
FRACTALD_STORAGE_ENABLE=0 \
    "$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!

for _ in $(seq 1 100); do
    [ -S "$root/run/control.sock" ] && break
    sleep 0.05
done
test -S "$root/run/control.sock"

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/units" \
    "$project_dir/target/debug/fractalctl" start journald.target >/dev/null

for _ in $(seq 1 100); do
    if FRACTALD_RUNTIME_DIR="$root/run" \
        "$project_dir/target/debug/fractalctl" status systemd-journald.service 2>/dev/null |
        grep -q 'systemd-journald.service: running'; then
        break
    fi
    sleep 0.05
done

printf '%s\n' 'activation first' |
    FRACTALD_JOURNAL_SOCKET="$first_socket" FRACTALD_LOG_DIR="$root/sender-logs" \
    "$project_dir/target/debug/systemd-cat" --identifier=journald-activation
printf '%s\n' 'activation second' |
    FRACTALD_JOURNAL_SOCKET="$second_socket" FRACTALD_LOG_DIR="$root/sender-logs" \
    "$project_dir/target/debug/systemd-cat" --identifier=journald-activation

for _ in $(seq 1 100); do
    if grep -Fq 'activation first' "$root/logs/journald-activation.stdout.log" 2>/dev/null &&
        grep -Fq 'activation second' "$root/logs/journald-activation.stdout.log" 2>/dev/null; then
        break
    fi
    sleep 0.05
done
grep -Fq 'activation first' "$root/logs/journald-activation.stdout.log"
grep -Fq 'activation second' "$root/logs/journald-activation.stdout.log"
echo 'journal socket activation: PASS'
