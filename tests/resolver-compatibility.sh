#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-resolver-compat.XXXXXX")
resolved_pid=
upstream_pid=

cleanup() {
    if [ -n "$resolved_pid" ]; then
        kill "$resolved_pid" 2>/dev/null || true
        wait "$resolved_pid" 2>/dev/null || true
    fi
    if [ -n "$upstream_pid" ]; then
        kill "$upstream_pid" 2>/dev/null || true
        wait "$upstream_pid" 2>/dev/null || true
    fi
    rm -r "$root"
}
trap cleanup EXIT INT TERM

python3 - "$root/upstream.port" <<'PY' &
import socket
import struct
import sys

port_file = sys.argv[1]
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("127.0.0.1", 0))
with open(port_file, "w", encoding="ascii") as stream:
    stream.write(str(sock.getsockname()[1]))
while True:
    packet, peer = sock.recvfrom(4096)
    response = bytearray(packet)
    response[2] |= 0x80
    response[3] |= 0x80
    response[6:8] = struct.pack("!H", 1)
    response.extend(b"\xc0\x0c\x00\x01\x00\x01\x00\x00\x00\x1e\x00\x04\xc0\x00\x02\x0a")
    sock.sendto(response, peer)
PY
upstream_pid=$!

attempt=0
while [ ! -s "$root/upstream.port" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { echo "upstream did not start" >&2; exit 1; }
    sleep 0.01
done
upstream_port=$(cat "$root/upstream.port")
runtime="$root/runtime"
mkdir -p "$runtime"

FRACTALD_RUNTIME_DIR="$runtime" \
FRACTALD_RESOLVED_LISTEN="127.0.0.1:0" \
FRACTALD_RESOLVED_UPSTREAM="127.0.0.1:$upstream_port" \
FRACTALD_RESOLVED_CACHE_ENTRIES=8 \
    "$project_dir/target/debug/fractald-resolved" >"$root/resolved.log" 2>&1 &
resolved_pid=$!

attempt=0
while [ ! -s "$runtime/resolved.endpoint" ]; do
    attempt=$((attempt + 1))
    [ "$attempt" -lt 100 ] || { cat "$root/resolved.log" >&2; exit 1; }
    sleep 0.01
done

status=$(FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" status)
case "$status" in
    *"Stub Listener: 127.0.0.1:"*) ;;
    *) echo "$status" >&2; exit 1 ;;
esac

query=$(FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" query --type=A example.com)
case "$query" in
    *"A 192.0.2.10"*) ;;
    *) echo "$query" >&2; exit 1 ;;
esac

statistics=$(FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" statistics)
case "$statistics" in
    *"cache_entries=1"*"cache_limit=8"*) ;;
    *) echo "$statistics" >&2; exit 1 ;;
esac

FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" flush-caches >/dev/null
statistics=$(FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" statistics)
case "$statistics" in
    *"cache_entries=0"*) ;;
    *) echo "$statistics" >&2; exit 1 ;;
esac

FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" dns "127.0.0.1:$upstream_port" >/dev/null
status=$(FRACTALD_RUNTIME_DIR="$runtime" "$project_dir/target/debug/resolvectl" status)
case "$status" in
    *"DNS Servers: 127.0.0.1:$upstream_port"*) ;;
    *) echo "$status" >&2; exit 1 ;;
esac

kill "$resolved_pid"
wait "$resolved_pid"
resolved_pid=
[ ! -e "$runtime/resolved.endpoint" ]
[ ! -e "$runtime/resolved.control" ]

echo "resolver compatibility: ok"
