#!/bin/sh
set -eu

runtime=/run/fractald-pid1
device=
for candidate in /dev/vd* /dev/xvd* /dev/sd* /dev/nvme*n*; do
    if [ -b "$candidate" ]; then
        device=$candidate
        break
    fi
done
[ -n "$device" ] || exit 1

# /dev/null is explicitly allowed. A block device is intentionally absent from
# the closed policy, so an attempted open must be rejected by the cgroup filter.
if /usr/bin/dd if="$device" of=/dev/null bs=512 count=1 status=none 2>/dev/null; then
    exit 1
fi
/usr/bin/printf '%s\n' "device-policy: blocked $device" >"$runtime/device-policy"
