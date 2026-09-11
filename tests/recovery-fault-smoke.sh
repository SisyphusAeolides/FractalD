#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-recovery.XXXXXX")
daemon_pid=
resolver_pid=

cleanup() {
    if [ -n "$daemon_pid" ]; then
        FRACTALD_RUNTIME_DIR="$root/run" \
        FRACTALD_STATE_DIR="$root/state" \
        FRACTALD_SERVICE_DIR="$root/services" \
            "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    if [ -n "$resolver_pid" ]; then
        kill "$resolver_pid" 2>/dev/null || true
        wait "$resolver_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT INT TERM

mkdir -p "$root/run" "$root/state" "$root/services"

cat >"$root/flaky.sh" <<'SCRIPT'
#!/bin/sh
count_file=$1
count=0
if [ -f "$count_file" ]; then
    count=$(cat "$count_file")
fi
count=$((count + 1))
printf '%s\n' "$count" >"$count_file"
if [ "$count" -lt 3 ]; then
    exit 42
fi
exec sleep 30
SCRIPT
chmod 0755 "$root/flaky.sh"
cat >"$root/services/flaky.service" <<UNIT
[Service]
ExecStart=$root/flaky.sh $root/flaky.count
Restart=on-failure
RestartSec=30ms
UNIT

cat >"$root/recovery.sh" <<'SCRIPT'
#!/bin/sh
printf 'recovered\n' >"$1"
SCRIPT
chmod 0755 "$root/recovery.sh"
cat >"$root/services/broken.service" <<UNIT
[Unit]
OnFailure=recovery.service

[Service]
ExecStart=/bin/false
Restart=no
UNIT
cat >"$root/services/recovery.service" <<UNIT
[Service]
Type=oneshot
ExecStart=$root/recovery.sh $root/recovered
RemainAfterExit=yes
UNIT

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!

attempt=0
while ! FRACTALD_RUNTIME_DIR="$root/run" FRACTALD_STATE_DIR="$root/state" FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" status >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" start flaky.service >/dev/null

attempt=0
while :; do
    count=0
    [ ! -f "$root/flaky.count" ] || count=$(cat "$root/flaky.count")
    status=$(FRACTALD_RUNTIME_DIR="$root/run" FRACTALD_STATE_DIR="$root/state" FRACTALD_SERVICE_DIR="$root/services" \
        "$project_dir/target/debug/fractalctl" status flaky.service)
    case "$status" in
        *": running ("*)
            if [ "$count" -ge 3 ]; then
                break
            fi
            ;;
    esac
    attempt=$((attempt + 1))
    [ "$attempt" -lt 200 ] || { echo "$status" >&2; exit 1; }
    sleep 0.02
done

FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" start broken.service >/dev/null
attempt=0
while [ ! -f "$root/recovered" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 200 ] || { cat "$root/daemon.log" >&2; exit 1; }
    sleep 0.02
done
broken_status=$(FRACTALD_RUNTIME_DIR="$root/run" FRACTALD_STATE_DIR="$root/state" FRACTALD_SERVICE_DIR="$root/services" \
    "$project_dir/target/debug/fractalctl" status broken.service)
case "$broken_status" in
    *": failed ("*) ;;
    *) echo "$broken_status" >&2; exit 1 ;;
esac

resolver_runtime="$root/resolver-run"
mkdir -p "$resolver_runtime"
FRACTALD_RUNTIME_DIR="$resolver_runtime" \
FRACTALD_RESOLVED_LISTEN="127.0.0.1:0" \
FRACTALD_RESOLVED_UPSTREAM="127.0.0.1:9" \
FRACTALD_RESOLVED_FAULT=servfail \
    "$project_dir/target/debug/fractald-resolved" >"$root/resolver.log" 2>&1 &
resolver_pid=$!
attempt=0
while [ ! -s "$resolver_runtime/resolved.endpoint" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { cat "$root/resolver.log" >&2; exit 1; }
    sleep 0.01
done

if FRACTALD_RUNTIME_DIR="$resolver_runtime" "$project_dir/target/debug/resolvectl" query example.com A >/dev/null 2>&1; then
    echo "SERVFAIL fault did not fail the DNS query" >&2
    exit 1
fi
FRACTALD_RUNTIME_DIR="$resolver_runtime" "$project_dir/target/debug/resolvectl" status >/dev/null
kill "$resolver_pid"
wait "$resolver_pid"
resolver_pid=
[ ! -e "$resolver_runtime/resolved.endpoint" ]
[ ! -e "$resolver_runtime/resolved.control" ]

echo "recovery and fault injection: ok"
