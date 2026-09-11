#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-analyze.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

binary="$project_dir/target/debug/systemd-analyze"
test -x "$binary"

mkdir -p "$root"/etc/systemd/resolved.conf.d \
    "$root"/run/systemd/resolved.conf.d \
    "$root"/usr/lib/systemd/resolved.conf.d \
    "$root"/etc/systemd/system
cat >"$root/usr/lib/systemd/resolved.conf" <<'CONFIG'
[Resolve]
DNS=192.0.2.1
CONFIG
cat >"$root/usr/lib/systemd/resolved.conf.d/20-vendor.conf" <<'CONFIG'
[Resolve]
FallbackDNS=192.0.2.2
CONFIG
cat >"$root/run/systemd/resolved.conf.d/10-runtime.conf" <<'CONFIG'
[Resolve]
DNSStubListener=yes
CONFIG
cat >"$root/etc/systemd/resolved.conf" <<'CONFIG'
[Resolve]
DNSStubListener=no
CONFIG
cat >"$root/etc/systemd/resolved.conf.d/90-local.conf" <<'CONFIG'
# local policy
Domains=example.test
CONFIG
cat >"$root/etc/systemd/system/verify.service" <<'UNIT'
[Unit]
Description=verify smoke unit
[Service]
Type=oneshot
ExecStart=/bin/true
UNIT
cat >"$root/etc/systemd/system/invalid.service" <<'UNIT'
[Service]
Type=definitely-invalid
ExecStart=/bin/true
UNIT

config=$(
    FRACTALD_ANALYZE_ROOT="$root" "$binary" cat-config systemd/resolved.conf
)
echo "$config" | grep -Fq 'DNSStubListener=yes'
echo "$config" | grep -Fq 'DNSStubListener=no'
echo "$config" | grep -Fq 'FallbackDNS=192.0.2.2'
echo "$config" | grep -Fq '# local policy'

tldr=$(
    FRACTALD_ANALYZE_ROOT="$root" "$binary" --tldr cat-config systemd/resolved.conf
)
echo "$tldr" | grep -Fq 'Domains=example.test'
if echo "$tldr" | grep -Fq '# local policy'; then
    echo 'systemd-analyze --tldr retained a comment' >&2
    exit 1
fi

paths=$(FRACTALD_ANALYZE_ROOT="$root" "$binary" unit-paths)
echo "$paths" | grep -Fxq /etc/systemd/system
FRACTALD_ANALYZE_ROOT="$root" "$binary" verify verify.service
if FRACTALD_ANALYZE_ROOT="$root" "$binary" verify invalid.service >/dev/null 2>&1; then
    echo 'systemd-analyze verify accepted an invalid service type' >&2
    exit 1
fi
"$binary" --version >/dev/null
"$binary" --help >/dev/null

if FRACTALD_ANALYZE_ROOT="$root" "$binary" cat-config ../resolved.conf >/dev/null 2>&1; then
    echo 'systemd-analyze accepted a path escaping the configuration roots' >&2
    exit 1
fi

echo 'systemd-analyze compatibility: PASS'
