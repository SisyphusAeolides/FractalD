#!/bin/sh
set -eu

root=$(mktemp -d "/tmp/rustybox-smoke.XXXXXX")
trap 'rm -rf "$root"' EXIT HUP INT TERM

binary=$PWD/target/debug/rustybox
test -x "$binary"

applets=$("$binary" --list)
for applet in cat chaos chroot dmesg echo env false findmnt init insmod kill ln ls mkdir modprobe mount mv pwd readlink rm rmdir sleep sort swapoff swapon switch_root sync tr true umount uname which; do
    printf '%s\n' "$applets" | grep -Fx "$applet" >/dev/null
done

mkdir "$root/bin"
ln -s "$binary" "$root/bin/cat"
printf 'rustybox-c\n' >"$root/input"
"$root/bin/cat" "$root/input" >"$root/output"
test "$(cat "$root/output")" = rustybox-c

test "$("$binary" echo -n one two)" = 'one two'
"$binary" mkdir -p -m 755 "$root/tree/one/two"
test -d "$root/tree/one/two"
"$binary" ln -s "$root/input" "$root/link"
test "$("$binary" readlink "$root/link")" = "$root/input"
"$binary" mv "$root/link" "$root/moved-link"
test -L "$root/moved-link"
"$binary" rm "$root/moved-link"
test ! -e "$root/moved-link"
"$binary" rm -r "$root/tree"
test ! -e "$root/tree"

test "$("$binary" env -i RB_TEST=ok "$binary" env)" = 'RB_TEST=ok'
PATH="$root/bin:$PATH" "$binary" which cat | grep -Fx "$root/bin/cat" >/dev/null
"$binary" uname -s >/dev/null
"$binary" mount | grep -F '/proc' >/dev/null
"$binary" findmnt -M /proc >/dev/null
test "$(printf 'ABC\n' | "$binary" tr '[:upper:]' '[:lower:]')" = abc
printf 'b two\na one\n' | "$binary" sort -k2,2 | grep -Fx 'a one' >/dev/null
printf 'b two\na one\n' | "$binary" sort -k2,2 | tail -n 1 | grep -Fx 'b two' >/dev/null
"$binary" sync
"$binary" switch_root --help >/dev/null
"$binary" sleep 0.001
"$binary" true
RUSTYBOX_INIT=/bin/true "$binary" init
if "$binary" false; then
    exit 1
fi

for system in lorenz mandelbrot lyapunov rossler logistic-map duffing; do
    "$binary" chaos sample "$system" >/dev/null
done
"$binary" chaos check >/dev/null

echo "rustybox smoke: PASS"
