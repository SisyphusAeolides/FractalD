#!/bin/sh
set -eu

runtime=/run/fractald-pid1
cgroup_root=${FRACTALD_CGROUP_ROOT:-/sys/fs/cgroup}
cgroup_relative=$(/usr/bin/sed -n 's/^0:://p' /proc/self/cgroup)
[ -n "$cgroup_relative" ]
cpu_max=$(/usr/bin/cat "$cgroup_root$cgroup_relative/cpu.max")
[ "$cpu_max" = '50000 100000' ] || exit 1
subtree_control=$(/usr/bin/cat "$cgroup_root$cgroup_relative/cgroup.subtree_control")
echo "$subtree_control" | /usr/bin/grep -Eq '(^|[[:space:]])cpu([[:space:]]|$)' || exit 1
echo "$subtree_control" | /usr/bin/grep -Eq '(^|[[:space:]])memory([[:space:]]|$)' || exit 1
/usr/bin/printf '%s\n' "$cpu_max" >"$runtime/cpu-quota"
