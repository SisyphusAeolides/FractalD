#!/bin/sh
set -eu

ip=/usr/sbin/ip
interface=${FRACTALD_TEST_INTERFACE:-}
log=/root/fractald-pid1-test/services.log
/usr/bin/printf '%s\n' 'network: start' >>"$log"

if [ -z "$interface" ]; then
    interface=$(
        "$ip" -o link show |
            /usr/bin/sed -n 's/^[0-9][0-9]*: \([^:]*\):.*/\1/p' |
            /usr/bin/grep -v '^lo$' |
            /usr/bin/head -n 1
    )
fi
[ -n "$interface" ] || {
    /usr/bin/printf '%s\n' 'network: no non-loopback interface found' >>"$log"
    exit 1
}

"$ip" link set lo up
"$ip" link set "$interface" up
if ! "$ip" -4 addr show dev "$interface" | /usr/bin/grep -q '10.0.2.15/24'; then
    "$ip" addr add 10.0.2.15/24 dev "$interface"
fi
"$ip" route replace default via 10.0.2.2 dev "$interface"
/usr/bin/printf '%s\n' 'network: complete' >>"$log"
