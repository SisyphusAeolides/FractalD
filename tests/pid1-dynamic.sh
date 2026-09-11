#!/bin/sh
set -eu

runtime=${RUNTIME_DIRECTORY:?RUNTIME_DIRECTORY was not provided}
uid=$(/usr/bin/sed -n 's/^Uid:[[:space:]]*\([0-9][0-9]*\).*/\1/p' /proc/self/status | /usr/bin/sed -n '1p')
gid=$(/usr/bin/sed -n 's/^Gid:[[:space:]]*\([0-9][0-9]*\).*/\1/p' /proc/self/status | /usr/bin/sed -n '1p')
groups=$(/usr/bin/sed -n 's/^Groups:[[:space:]]*//p' /proc/self/status | /usr/bin/sed -n '1p')
[ -n "$uid" ]
[ -n "$gid" ]
[ -z "$groups" ]
/usr/bin/printf '%s:%s\n' "$uid" "$gid" >"$runtime/identity"
/usr/bin/printf '%s\n' "$groups" >"$runtime/supplementary-groups"
/usr/bin/printf '%s\n' ready >"$runtime/ready"
