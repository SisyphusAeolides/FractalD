#!/bin/sh
set -eu

/usr/bin/install -d -m 0755 /run/sshd
/usr/bin/printf '%s\n' 'sshd: start' >>/root/fractald-pid1-test/services.log
exec /usr/sbin/sshd -D -e
