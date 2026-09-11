#!/bin/sh
set -eu

toolbox=${FRACTALD_TEST_TOOLBOX:-/usr/local/libexec/fractald-pid1-test/rustybox}
root=/run/fractald-pid1
/usr/bin/printf '%s\n' 'ready: start' >>/root/fractald-pid1-test/services.log
/usr/bin/install -d -m 0755 "$root"

"$toolbox" chaos check >"$root/chaos-check"
"$toolbox" chaos list >"$root/chaos-list"
"$toolbox" mkdir -p "$root/rustybox-created"
"$toolbox" echo pid1-toolbox-ready >"$root/toolbox-output"

(
    (
        /usr/bin/sleep 2
        /usr/bin/printf '%s\n' orphan-complete >"$root/orphan-complete"
    ) &
    /usr/bin/printf '%s\n' "$!" >"$root/orphan-pid"
) &

/usr/bin/printf '%s\n' ready >"$root/ready"
/usr/bin/printf '%s\n' 'ready: complete' >>/root/fractald-pid1-test/services.log
