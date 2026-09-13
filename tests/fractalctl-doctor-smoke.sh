#!/bin/sh
set -eu

project_directory=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT HUP INT TERM

make -C "$project_directory" \
    DESTDIR="$root" \
    PREFIX=/usr \
    BINDIR=/usr/bin \
    LIBEXECDIR=/usr/lib \
    SERVICE_DIR=/usr/lib/fractald/services \
    INSTALL_ALPM_HOOK=0 \
    install >/dev/null
mkdir -p "$root/sbin"
ln -s ../usr/bin/fractald "$root/sbin/init"
mkdir -p "$root/etc/fractald"
cat >"$root/etc/fractald/boot.conf" <<'BOOT'
profile=boot
init=/usr/bin/fractald
BOOT

unset FRACTALD_SERVICE_DIR FRACTALD_DOCTOR_PROC_ROOT
output=$(FRACTALD_DOCTOR_ROOT="$root" \
    "$project_directory/target/debug/fractalctl" verify-pid1)
printf '%s\n' "$output"
printf '%s\n' "$output" | grep -F 'PASS boot:' >/dev/null
printf '%s\n' "$output" | grep -F 'PASS services:' >/dev/null
printf '%s\n' "$output" | grep -F 'FractalD doctor: PASS' >/dev/null

echo 'fractalctl doctor: PASS'
