#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "/tmp/fractald-sysctl.XXXXXX")
trap 'rm -r "$root"' EXIT HUP INT TERM

mkdir -p "$root/usr/lib/sysctl.d" "$root/etc/sysctl.d" \
    "$root/proc/sys/net/ipv4/conf/all" "$root/proc/sys/net/ipv4/conf/default"
printf '0\n' >"$root/proc/sys/net/ipv4/ip_forward"
printf '0\n' >"$root/proc/sys/net/ipv4/conf/all/rp_filter"
printf '0\n' >"$root/proc/sys/net/ipv4/conf/default/rp_filter"

cat >"$root/usr/lib/sysctl.d/10-network.conf" <<'CONFIG'
# Vendor defaults.
net.ipv4.ip_forward = 1
net.ipv4.conf.*.rp_filter = 2
CONFIG
cat >"$root/etc/sysctl.d/10-network.conf" <<'CONFIG'
# The administrator file with the same name overrides the vendor file.
net.ipv4.ip_forward = 0
CONFIG
cat >"$root/usr/lib/sysctl.d/20-optional.conf" <<'CONFIG'
net.ipv4.missing_value = 1
CONFIG

binary="$project_dir/target/debug/systemd-sysctl"
"$binary" --root="$root"
test "$(cat "$root/proc/sys/net/ipv4/ip_forward")" = 0
test "$(cat "$root/proc/sys/net/ipv4/conf/all/rp_filter")" = 0
test "$(cat "$root/proc/sys/net/ipv4/conf/default/rp_filter")" = 0

dry_output=$("$binary" --root="$root" --dry-run --prefix=net.ipv4.conf \
    "$root/usr/lib/sysctl.d/10-network.conf")
echo "$dry_output" | grep -q 'Would set'

config=$("$binary" --root="$root" --cat-config)
echo "$config" | grep -q '10-network.conf'
echo "$config" | grep -q 'ip_forward'

if "$binary" --root="$root" --strict "$root/usr/lib/sysctl.d/20-optional.conf"; then
    echo 'strict sysctl mode unexpectedly succeeded' >&2
    exit 1
fi

echo 'systemd sysctl compatibility: PASS'
