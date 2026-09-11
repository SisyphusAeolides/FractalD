#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "/tmp/fractald-sysusers.XXXXXX")
trap 'rm -r "$root"' EXIT HUP INT TERM

mkdir -p "$root/usr/lib/sysusers.d" "$root/usr/local/lib/sysusers.d" \
    "$root/run/sysusers.d" "$root/etc/sysusers.d" "$root/etc"
cat >"$root/etc/login.defs" <<'DEFS'
SYS_UID_MIN 201
SYS_UID_MAX 999
SYS_GID_MIN 201
SYS_GID_MAX 999
DEFS
: >"$root/etc/shadow"
: >"$root/etc/gshadow"

cat >"$root/usr/lib/sysusers.d/20-demo.conf" <<'CONFIG'
# The parser must preserve quoted GECOS fields.
g demo-group 450
u! demo-user - "Demo User" /var/lib/demo /usr/sbin/nologin
m demo-user demo-group
CONFIG
cat >"$root/usr/lib/sysusers.d/30-range.conf" <<'CONFIG'
r - 2000-2002
u dynamic-user -
CONFIG
cat >"$root/usr/lib/sysusers.d/40-override.conf" <<'CONFIG'
u low-priority-user 460
CONFIG
cat >"$root/etc/sysusers.d/40-override.conf" <<'CONFIG'
u high-priority-user 461
CONFIG
cat >"$root/usr/lib/sysusers.d/50-disabled.conf" <<'CONFIG'
u masked-user 470
CONFIG
ln -s /dev/null "$root/etc/sysusers.d/50-disabled.conf"

binary="$project_dir/target/debug/systemd-sysusers"
"$binary" --root="$root"

grep -Eq '^demo-user:x:[0-9]+:[0-9]+:Demo User:/var/lib/demo:/usr/sbin/nologin$' \
    "$root/etc/passwd"
grep -Eq '^demo-group:x:450:demo-user$' "$root/etc/group"
grep -Eq '^demo-user:!\*:' "$root/etc/shadow"
grep -Eq '^demo-group:!\*::' "$root/etc/gshadow"
grep -Eq '^dynamic-user:x:2002:2002:' "$root/etc/passwd"
grep -Eq '^high-priority-user:x:461:' "$root/etc/passwd"
! grep -q '^low-priority-user:' "$root/etc/passwd"
! grep -q '^masked-user:' "$root/etc/passwd"

before=$(sha256sum "$root/etc/passwd" "$root/etc/group" "$root/etc/shadow" "$root/etc/gshadow")
"$binary" --root="$root" >/dev/null
after=$(sha256sum "$root/etc/passwd" "$root/etc/group" "$root/etc/shadow" "$root/etc/gshadow")
[ "$before" = "$after" ]

dry_before=$(sha256sum "$root/etc/passwd")
dry_output=$("$binary" --root="$root" --dry-run --inline 'u dry-run-user 480')
echo "$dry_output" | grep -q "Would create user 'dry-run-user'"
! grep -q '^dry-run-user:' "$root/etc/passwd"
[ "$dry_before" = "$(sha256sum "$root/etc/passwd")" ]

replace_output=$(printf '%s\n' 'u replacement-user 482' | \
    "$binary" --root="$root" --replace=/usr/lib/sysusers.d/replacement.conf -)
echo "$replace_output" | grep -q "Creating user 'replacement-user'"
grep -Eq '^replacement-user:x:482:' "$root/etc/passwd"

config=$("$binary" --root="$root" --cat-config)
echo "$config" | grep -q '20-demo.conf'
echo "$config" | grep -q 'Demo User'

echo 'systemd sysusers compatibility: PASS'
