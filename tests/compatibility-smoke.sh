#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
daemon_pid=
journal_pid=
cat_journal_pid=

cleanup() {
    if [ -n "$daemon_pid" ]; then
        FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    if [ -n "$journal_pid" ]; then
        kill "$journal_pid" 2>/dev/null || true
        wait "$journal_pid" 2>/dev/null || true
    fi
    if [ -n "$cat_journal_pid" ]; then
        kill "$cat_journal_pid" 2>/dev/null || true
        wait "$cat_journal_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$root/services" "$root/run" "$root/state"
export FRACTALD_JOURNAL_SOCKET="$root/native-journal.sock"
python3 - "$FRACTALD_JOURNAL_SOCKET" "$root/native-journal.log" >"$root/native-journal-server.log" 2>&1 <<'PY' &
import socket
import sys

socket_path, log_path = sys.argv[1:3]
server = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
server.bind(socket_path)
server.settimeout(15)
try:
    with open(log_path, "wb") as log:
        while True:
            try:
                payload = server.recv(65535)
            except socket.timeout:
                break
            log.write(payload)
            log.write(b"\n")
            log.flush()
            if b"MESSAGE=journal-compatibility-smoke" in payload:
                break
finally:
    server.close()
PY
journal_pid=$!
attempt=0
while [ ! -S "$FRACTALD_JOURNAL_SOCKET" ] && [ "$attempt" -lt 100 ]; do
    attempt=$((attempt + 1))
    sleep 0.01
done
test -S "$FRACTALD_JOURNAL_SOCKET"

cat > "$root/tmpfiles.conf" <<'RULES'
D /var/lib/fractald-smoke 0750 - - -
f /var/lib/fractald-smoke/state 0640 - - -
RULES

"$project_dir/target/debug/systemd-tmpfiles" \
    --create --root="$root" "$root/tmpfiles.conf"
test -f "$root/var/lib/fractald-smoke/state"

escaped_name=$(
    "$project_dir/target/debug/systemd-escape" 'package unit'
)
test "$escaped_name" = 'package\x20unit'
escaped_path=$(
    "$project_dir/target/debug/systemd-escape" --path /var/lib/fractald
)
test "$escaped_path" = 'var-lib-fractald'
template_name=$(
    "$project_dir/target/debug/systemd-escape" --template=worker@.service alpha
)
test "$template_name" = 'worker@alpha.service'

cat > "$root/services/notify-smoke.service" <<UNIT
[Service]
Type=notify
NotifyAccess=all
ExecStart=/usr/bin/python3 -c "import os, socket, time; socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).sendto(b'READY=1\\\\nSTATUS=compatibility-smoke\\\\n', os.environ['NOTIFY_SOCKET']); time.sleep(10)"
TimeoutStartSec=2s
UNIT

cat > "$root/services/notify-tool-smoke.service" <<UNIT
[Service]
Type=notify
ExecStart=$project_dir/target/debug/systemd-notify --ready --status=notify-tool
RemainAfterExit=yes
TimeoutStartSec=2s
UNIT

cat > "$root/services/journal-smoke.service" <<UNIT
[Service]
Type=oneshot
ExecStart=/bin/echo journal-compatibility-smoke
RemainAfterExit=yes
StandardOutput=journal
UNIT

cat > "$root/services/isolate-outside.service" <<UNIT
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT

cat > "$root/services/isolate-ignored.service" <<UNIT
[Unit]
IgnoreOnIsolate=yes

[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT

cat > "$root/services/isolate-kept.service" <<UNIT
[Service]
Type=oneshot
ExecStart=/bin/true
RemainAfterExit=yes
UNIT

cat > "$root/services/isolate.target" <<UNIT
[Unit]
AllowIsolate=yes
Wants=isolate-kept.service
UNIT

mkdir -p "$root/bin" "$root/mounts/data/cache"
mount_order="$root/mount-order.log"
cat > "$root/bin/mount" <<SCRIPT
#!/bin/sh
target=
for value in "\$@"; do
    target="\$value"
done
case "\$target" in
    */mounts/data) echo data >> "$mount_order" ;;
    */mounts/data/cache) echo cache >> "$mount_order" ;;
esac
SCRIPT
cat > "$root/bin/umount" <<'SCRIPT'
#!/bin/sh
exit 0
SCRIPT
chmod 755 "$root/bin/mount" "$root/bin/umount"
data_mount=$(
    "$project_dir/target/debug/systemd-escape" --path "$root/mounts/data"
).mount
cache_mount=$(
    "$project_dir/target/debug/systemd-escape" --path "$root/mounts/data/cache"
).mount
cat > "$root/services/$data_mount" <<UNIT
[Mount]
What=fractald-test-data
Where=$root/mounts/data
Type=tmpfs
UNIT
cat > "$root/services/$cache_mount" <<UNIT
[Mount]
What=fractald-test-cache
Where=$root/mounts/data/cache
Type=tmpfs
UNIT
cat > "$root/services/mount-requirement-smoke.service" <<UNIT
[Unit]
RequiresMountsFor=$root/mounts/data/cache/file

[Service]
Type=oneshot
ExecStart=/bin/sh -c "echo service >> $mount_order"
RemainAfterExit=yes
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
PATH="$root/bin:/usr/bin:/bin" \
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

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start isolate-outside.service >/dev/null
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start isolate-ignored.service >/dev/null
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" isolate isolate.target >/dev/null
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" is-active isolate.target >/dev/null
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" is-active isolate-kept.service >/dev/null
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" is-active isolate-ignored.service >/dev/null
if FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/systemctl" is-active isolate-outside.service >/dev/null 2>&1; then
    echo 'isolate did not stop an outside unit' >&2
    exit 1
fi

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start mount-requirement-smoke.service >/dev/null
test "$(sed -n '1p' "$mount_order")" = data
test "$(sed -n '2p' "$mount_order")" = cache
test "$(sed -n '3p' "$mount_order")" = service

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start notify-tool-smoke.service

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" stop notify-tool-smoke.service >/dev/null

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start notify-smoke.service

status=$(
    FRACTALD_RUNTIME_DIR="$root/run" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" status notify-smoke.service
)
printf '%s\n' "$status" | grep -q 'notify-smoke.service: running'

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/fractalctl" stop notify-smoke.service >/dev/null

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/systemctl" start journal-smoke.service >/dev/null

native_ready=false
local_ready=false
attempt=0
while [ "$attempt" -lt 100 ]; do
    if grep -q 'MESSAGE=journal-compatibility-smoke' "$root/native-journal.log" 2>/dev/null; then
        native_ready=true
    fi
    journal_output=$(FRACTALD_LOG_DIR="$root/state/logs" \
        "$project_dir/target/debug/journalctl" --no-pager --unit journal-smoke.service)
    case "$journal_output" in
        *"journal-compatibility-smoke"*) local_ready=true ;;
    esac
    if [ "$native_ready" = true ] && [ "$local_ready" = true ]; then
        break
    fi
    attempt=$((attempt + 1))
    sleep 0.02
done
test "$native_ready" = true
test "$local_ready" = true
grep -q '_SYSTEMD_UNIT=journal-smoke.service' "$root/native-journal.log"

cat_socket="$root/cat-native-journal.sock"
cat_log="$root/cat-native-journal.log"
python3 - "$cat_socket" "$cat_log" >"$root/cat-native-journal-server.log" 2>&1 <<'PY' &
import socket
import sys

socket_path, log_path = sys.argv[1:3]
server = socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM)
server.bind(socket_path)
server.settimeout(15)
try:
    with open(log_path, "wb") as log:
        while True:
            try:
                payload = server.recv(65535)
            except socket.timeout:
                break
            log.write(payload)
            log.write(b"\n")
            log.flush()
            if b"MESSAGE=cat-native" in payload:
                break
finally:
    server.close()
PY
cat_journal_pid=$!
attempt=0
while [ ! -S "$cat_socket" ] && [ "$attempt" -lt 100 ]; do
    attempt=$((attempt + 1))
    sleep 0.01
done
test -S "$cat_socket"
printf 'cat-native\n' | \
    FRACTALD_JOURNAL_SOCKET="$cat_socket" \
    "$project_dir/target/debug/systemd-cat" -t cat-smoke
attempt=0
while [ "$attempt" -lt 100 ] && [ ! -f "$cat_log" ]; do
    attempt=$((attempt + 1))
    sleep 0.02
done
grep -q 'MESSAGE=cat-native' "$cat_log"
grep -q '_SYSTEMD_UNIT=cat-smoke' "$cat_log"

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
"$project_dir/target/debug/fractalctl" stop journal-smoke.service >/dev/null
