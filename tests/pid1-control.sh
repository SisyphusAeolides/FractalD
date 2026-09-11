#!/bin/sh
set -eu

root=/run/fractald-pid1
/usr/bin/printf '%s\n' started >"$root/control-started"
trap '/usr/bin/printf "%s\n" stopped >"$root/control-stopped"; exit 0' TERM INT
while :; do
    /usr/bin/sleep 1
done
