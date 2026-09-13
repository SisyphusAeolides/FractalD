#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'if [ -n "${daemon_pid:-}" ]; then kill "$daemon_pid" 2>/dev/null || true; wait "$daemon_pid" 2>/dev/null || true; fi; rm -rf "$root"' EXIT

mkdir -p "$root/db/demo-1" "$root/usr/lib/fractald/services"
cat >"$root/usr/lib/fractald/services/demo.svc" <<'SERVICE'
[service]
description=Package owned demo service
kind=oneshot
exec=/bin/sh -c "printf started > %t/package-trigger.marker"
remain_after_exit=true

[install]
profile=boot
SERVICE
cat >"$root/db/demo-1/files" <<'FILES'
%FILES%
usr/lib/fractald/services/demo.svc
FILES

env \
    FRACTALD_PACKAGE_DB="$root/db" \
    FRACTALD_PACKAGE_ROOT="$root" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_RUNTIME_DIR="$root/runtime" \
    "$project_dir/target/debug/fractald-package-trigger" verify
env \
    FRACTALD_PACKAGE_DB="$root/db" \
    FRACTALD_PACKAGE_ROOT="$root" \
    FRACTALD_STATE_DIR="$root/state" \
    FRACTALD_RUNTIME_DIR="$root/runtime" \
    "$project_dir/target/debug/fractald-package-trigger" sync
test -f "$root/state/enabled/demo"
test -f "$root/state/packages.index"

export FRACTALD_SERVICE_DIR="$root/usr/lib/fractald/services"
export FRACTALD_PACKAGE_DB="$root/db"
export FRACTALD_PACKAGE_ROOT="$root"
export FRACTALD_STATE_DIR="$root/state"
export FRACTALD_RUNTIME_DIR="$root/runtime"
"$project_dir/target/debug/fractald" daemon >"$root/daemon.log" 2>&1 &
daemon_pid=$!
for _ in $(seq 1 100); do
    if [ -S "$root/runtime/control.sock" ]; then
        break
    fi
    sleep 0.02
done
test -S "$root/runtime/control.sock"
for _ in $(seq 1 100); do
    if [ -f "$root/runtime/package-trigger.marker" ]; then
        break
    fi
    sleep 0.02
done
test -f "$root/runtime/package-trigger.marker"

"$project_dir/target/debug/fractalctl" status demo | grep -F 'demo: active' >/dev/null

cat >"$root/usr/lib/fractald/services/demo.svc" <<'SERVICE'
[service]
description=Package owned demo service
kind=group
SERVICE
"$project_dir/target/debug/fractald-package-trigger" sync
test ! -e "$root/state/enabled/demo"
sleep 0.1
! "$project_dir/target/debug/fractalctl" status demo | grep -F 'demo: active' >/dev/null

"$project_dir/target/debug/fractalctl" stop >/dev/null
unset daemon_pid

cat >"$root/usr/lib/fractald/services/server.svc" <<'SERVICE'
[service]
description=Package owned server profile
kind=group

[install]
profile=server
SERVICE
mkdir -p "$root/db/server-1"
cat >"$root/db/server-1/files" <<'FILES'
%FILES%
usr/lib/fractald/services/server.svc
FILES
FRACTALD_BOOT_PROFILE=server \
    "$project_dir/target/debug/fractald-package-trigger" sync
test -f "$root/state/enabled/server"
test ! -e "$root/state/enabled/demo"

mkdir -p "$root/etc/fractald/services" "$root/db/duplicate-1"
cp "$root/usr/lib/fractald/services/server.svc" \
    "$root/etc/fractald/services/server.svc"
cat >"$root/db/duplicate-1/files" <<'FILES'
%FILES%
etc/fractald/services/server.svc
FILES
if FRACTALD_BOOT_PROFILE=server \
    "$project_dir/target/debug/fractald-package-trigger" verify 2>"$root/duplicate-error"; then
    echo 'duplicate package descriptors were accepted' >&2
    exit 1
fi
grep -F 'declared more than once' "$root/duplicate-error" >/dev/null

echo 'native package trigger: PASS'
