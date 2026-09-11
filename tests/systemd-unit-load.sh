#!/bin/sh
set -eu

binary=${FRACTALD_UNIT_LOAD_BINARY:-target/debug/fractald}
directory=${FRACTALD_UNIT_LOAD_DIR:-/usr/lib/systemd/system}
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-unit-load.XXXXXX")

cleanup() {
    rm -r "$root"
}
trap cleanup EXIT HUP INT TERM

[ -x "$binary" ] || {
    echo "unit load: manager is not executable: $binary" >&2
    exit 1
}
[ -d "$directory" ] || {
    echo "unit load: unit directory is unavailable: $directory" >&2
    exit 1
}

set +e
FRACTALD_RUNTIME_DIR="$root/run" \
FRACTALD_STATE_DIR="$root/state" \
FRACTALD_SERVICE_DIR="$directory" \
FRACTALD_BOOT_TARGET= \
FRACTALD_STORAGE_ENABLE=0 \
    timeout --foreground 5s "$binary" daemon >"$root/daemon.log" 2>&1
status=$?
set -e

if [ "$status" -ne 124 ]; then
    cat "$root/daemon.log" >&2
    echo "unit load: manager exited unexpectedly with status $status" >&2
    exit 1
fi

if grep -Eq 'cannot load service|defined more than once|alias .* conflicts with an existing service|unknown service \.service' "$root/daemon.log"; then
    cat "$root/daemon.log" >&2
    echo 'unit load: package unit registry could not be loaded cleanly' >&2
    exit 1
fi

echo "unit load: loaded $directory without registry errors"
