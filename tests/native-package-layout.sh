#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

make -C "$project_dir" DESTDIR="$root" PREFIX=/usr BINDIR=/usr/bin \
    LIBEXECDIR=/usr/lib SERVICE_DIR=/usr/lib/fractald/services install >/dev/null
test -x "$root/usr/bin/fractald"
test -x "$root/usr/bin/fractald-package-trigger"
test -f "$root/usr/lib/fractald/services/boot.svc"
test -f "$root/usr/lib/fractald/services/fractald-journald.svc"
test -f "$root/usr/lib/fractald/services/fractald-udevd.svc"
test -f "$root/usr/share/libalpm/hooks/90-fractald-package.hook"
test -L "$root/usr/lib/fractald/toolbox/mount"
test "$(readlink "$root/usr/lib/fractald/toolbox/mount")" = ../../../bin/rustybox
test -x "$root/usr/lib/fractald/init"
test -L "$root/usr/bin/fractald-udevd"
test "$(readlink "$root/usr/bin/fractald-udevd")" = fractald-udevadm

echo 'native Arch package layout: PASS'
