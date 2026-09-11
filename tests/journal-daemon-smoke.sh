#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-journald.XXXXXX")
daemon_pid=
inherited_pid=

cleanup() {
    for pid in "$daemon_pid" "$inherited_pid"; do
        [ -n "$pid" ] || continue
        kill "$pid" 2>/dev/null || true
    done
    rm -r "$root"
}
trap cleanup EXIT INT TERM

daemon="$project_dir/target/debug/systemd-journald"
sender="$project_dir/target/debug/systemd-cat"
test -x "$daemon"
test -x "$sender"

socket="$root/run/systemd/journal/socket"
daemon_logs="$root/daemon-logs"
sender_logs="$root/sender-logs"
mkdir -p "$(dirname -- "$socket")"

FRACTALD_JOURNAL_SOCKET="$socket" FRACTALD_LOG_DIR="$daemon_logs" \
    "$daemon" --system >"$root/daemon.out" 2>&1 &
daemon_pid=$!

ready=0
for _ in $(seq 1 60); do
    if [ -S "$socket" ]; then
        ready=1
        break
    fi
    sleep 0.05
done
test "$ready" -eq 1

printf '%s\n' 'FractalD journal receiver' |
    FRACTALD_JOURNAL_SOCKET="$socket" FRACTALD_LOG_DIR="$sender_logs" \
        "$sender" --identifier=journal-daemon-smoke

kill "$daemon_pid" 2>/dev/null || true
set +e
wait "$daemon_pid"
set -e

grep -Fq 'FractalD journal receiver' "$daemon_logs/journal-daemon-smoke.stdout.log"
test -e "$sender_logs/journal-daemon-smoke.stdout.log"

inherited_first="$root/run/inherited-first.sock"
inherited_second="$root/run/inherited-second.sock"
inherited_logs="$root/inherited-logs"
inherited_pid=$(python3 - "$inherited_first" "$inherited_second" "$inherited_logs" "$daemon" <<'PY'
import os
import socket
import sys

first, second, log_directory, daemon = sys.argv[1:]
sockets = []
for path in (first, second):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
    sock.bind(path)
    sockets.append(sock)
pid = os.fork()
if pid == 0:
    for index, sock in enumerate(sockets):
        target = 3 + index
        if sock.fileno() != target:
            os.dup2(sock.fileno(), target)
        os.set_inheritable(3 + index, True)
    for index, sock in enumerate(sockets):
        if sock.fileno() != 3 + index:
            sock.close()
    null = os.open(os.devnull, os.O_RDWR)
    os.dup2(null, 1)
    os.dup2(null, 2)
    if null > 2:
        os.close(null)
    environment = os.environ.copy()
    environment["LISTEN_FDS"] = "2"
    environment["LISTEN_PID"] = str(os.getpid())
    environment["FRACTALD_LOG_DIR"] = log_directory
    os.execve(daemon, [daemon, "--system"], environment)
print(pid)
for sock in sockets:
    sock.close()
PY
)

printf '%s\n' 'inherited first' |
    FRACTALD_JOURNAL_SOCKET="$inherited_first" FRACTALD_LOG_DIR="$root/inherited-sender" \
        "$sender" --identifier=inherited-journal-smoke
printf '%s\n' 'inherited second' |
    FRACTALD_JOURNAL_SOCKET="$inherited_second" FRACTALD_LOG_DIR="$root/inherited-sender" \
        "$sender" --identifier=inherited-journal-smoke

for _ in $(seq 1 60); do
    if grep -Fq 'inherited first' "$inherited_logs/inherited-journal-smoke.stdout.log" 2>/dev/null &&
        grep -Fq 'inherited second' "$inherited_logs/inherited-journal-smoke.stdout.log" 2>/dev/null; then
        break
    fi
    sleep 0.05
done
grep -Fq 'inherited first' "$inherited_logs/inherited-journal-smoke.stdout.log"
grep -Fq 'inherited second' "$inherited_logs/inherited-journal-smoke.stdout.log"
kill "$inherited_pid" 2>/dev/null || true
echo 'journal daemon compatibility: PASS'
