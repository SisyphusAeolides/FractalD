#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "/tmp/fractald-udevadm.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

sysfs="$root/sys"
device="$sysfs/devices/virtual/block/vda"
mkdir -p "$device"
printf '%s\n' 'SUBSYSTEM=block' 'DEVNAME=vda' >"$device/uevent"
mkdir -p "$sysfs/class/block"
ln -s ../../devices/virtual/block/vda "$sysfs/class/block/vda"

binary="$project_dir/target/debug/udevadm"
test -x "$binary"

FRACTALD_SYSFS_ROOT="$sysfs" "$binary" trigger \
    --action=add --subsystem-match=block --sysname-match='vd?' \
    --property-match='SUBSYSTEM=block' --dry-run | grep -Fx "$device" >/dev/null

FRACTALD_SYSFS_ROOT="$sysfs" "$binary" trigger \
    --action=add --subsystem-match=block --sysname-match='vd?' \
    --property-match='SUBSYSTEM=block'
grep -Fx 'add' "$device/uevent" >/dev/null

printf '%s\n' 'SUBSYSTEM=block' 'DEVNAME=vda' >"$device/uevent"
properties=$(FRACTALD_SYSFS_ROOT="$sysfs" "$binary" info \
    --query=property --path=/devices/virtual/block/vda)
printf '%s\n' "$properties" | grep -Fx 'SUBSYSTEM=block' >/dev/null
printf '%s\n' "$properties" | grep -Fx 'DEVNAME=vda' >/dev/null
printf '%s\n' "$properties" | grep -Fx 'DEVPATH=/devices/virtual/block/vda' >/dev/null

FRACTALD_SYSFS_ROOT="$sysfs" "$binary" test-builtin \
    --query=property --path=/devices/virtual/block/vda >/dev/null
FRACTALD_SYSFS_ROOT="$sysfs" "$binary" control --reload-rules
FRACTALD_SYSFS_ROOT="$sysfs" "$binary" control --reload
FRACTALD_SYSFS_ROOT="$sysfs" FRACTALD_UDEV_ROOT="$root" "$binary" hwdb --update
FRACTALD_SYSFS_ROOT="$sysfs" "$binary" settle --timeout=0
"$binary" --version >/dev/null
"$binary" --help >/dev/null

runtime="$root/runtime"
daemon="$root/systemd-udevd"
ln -s "$binary" "$daemon"
"$daemon" --help >/dev/null
set +e
FRACTALD_UDEV_RUNTIME_DIR="$runtime" timeout --foreground 1s "$daemon" --foreground \
    >"$root/udevd.log" 2>&1
daemon_status=$?
set -e
test "$daemon_status" -eq 124
test -d "$runtime/queue"
test -d "$runtime/data"

if FRACTALD_SYSFS_ROOT="$sysfs" "$binary" trigger --action=invalid; then
    echo 'udevadm accepted an invalid action' >&2
    exit 1
fi
if FRACTALD_SYSFS_ROOT="$sysfs" "$binary" info --path=/devices/../escape >/dev/null 2>&1; then
    echo 'udevadm accepted a path traversal' >&2
    exit 1
fi

echo 'udevadm compatibility: PASS'
