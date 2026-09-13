#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'if [ -n "${daemon_pid:-}" ]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; fi; rm -rf "$root"' EXIT
cat >"$root/fstab" <<'FSTAB'
/dev/mapper/data /srv/data ext4 nofail,x-fractald.after=network-online 0 2
/swapfile none swap defaults,pri=5 0 0
FSTAB

export FRACTALD_STORAGE_FSTAB="$root/fstab"
export FRACTALD_STORAGE_SERVICE_DIR="$root/services"
export FRACTALD_SERVICE_DIR="$root/services"
export FRACTALD_RUNTIME_DIR="$root/runtime"
"$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!
sleep 0.1
kill "$daemon_pid" 2>/dev/null || true
wait "$daemon_pid" 2>/dev/null || true
unset daemon_pid

test -f "$root/services/storage.svc"
test -f "$root/services/mount-srv-data.svc"
echo 'native storage generation: PASS'
