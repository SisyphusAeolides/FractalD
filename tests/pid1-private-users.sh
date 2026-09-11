#!/bin/sh
set -eu

runtime=/run/fractald-pid1
mapping=$(/usr/bin/sed -n 's/[[:space:]]\+/ /gp' /proc/self/uid_map | /usr/bin/sed -n '1p')
echo "$mapping" | /usr/bin/grep -Eq '^0 0 1$' || exit 1
/usr/bin/printf '%s\n' 'private-users: ok' >"$runtime/private-users"
