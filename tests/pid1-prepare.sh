#!/bin/sh
set -eu

project=${FRACTALD_TEST_PROJECT:-/home/tester/FractalD}
unit_directory=/etc/fractald/pid1-test
script_directory=/usr/local/libexec/fractald-pid1-test

/usr/bin/install -d -m 0755 "$unit_directory" "$script_directory"
/usr/bin/install -m 0755 "$project/tests/pid1-boot-init.sh" /sbin/fractald-pid1-test
/usr/bin/install -m 0755 "$project/tests/pid1-network.sh" "$script_directory/network"
/usr/bin/install -m 0755 "$project/tests/pid1-sshd.sh" "$script_directory/sshd"
/usr/bin/install -m 0755 "$project/tests/pid1-ready.sh" "$script_directory/ready"
/usr/bin/install -m 0755 "$project/tests/pid1-dynamic.sh" "$script_directory/dynamic"
/usr/bin/install -m 0755 "$project/tests/pid1-cpu-quota.sh" "$script_directory/cpu-quota"
/usr/bin/install -m 0755 "$project/tests/pid1-device-policy.sh" "$script_directory/device-policy"
/usr/bin/install -m 0755 "$project/tests/pid1-private-users.sh" "$script_directory/private-users"
/usr/bin/install -m 0755 "$project/tests/pid1-control.sh" "$script_directory/control"
/usr/bin/install -m 0755 "$project/tests/pid1-smoke.sh" "$script_directory/smoke"
/usr/bin/install -m 0755 "$project/tests/storage-matrix-smoke.sh" "$script_directory/storage-matrix"
/usr/bin/install -m 0755 "$project/tests/storage-native-recovery.sh" "$script_directory/storage-recovery"
/usr/bin/install -m 0755 "$project/target/release/fractald" "$script_directory/fractald"
/usr/bin/install -m 0755 "$project/target/release/fractalctl" "$script_directory/fractalctl"
/usr/bin/install -m 0755 "$project/target/release/systemctl" "$script_directory/systemctl"
/usr/bin/install -m 0755 "$project/target/release/rustybox" "$script_directory/rustybox"
/usr/bin/install -m 0644 "$project/tests/pid1.target" "$unit_directory/pid1.target"
/usr/bin/install -m 0644 "$project/tests/pid1-network.service" "$unit_directory/pid1-network.service"
/usr/bin/install -m 0644 "$project/tests/pid1-sshd.service" "$unit_directory/pid1-sshd.service"
/usr/bin/install -m 0644 "$project/tests/pid1-ready.service" "$unit_directory/pid1-ready.service"
/usr/bin/install -m 0644 "$project/tests/pid1-dynamic.service" "$unit_directory/pid1-dynamic.service"
/usr/bin/install -m 0644 "$project/tests/pid1-cpu-quota.service" "$unit_directory/pid1-cpu-quota.service"
/usr/bin/install -m 0644 "$project/tests/pid1-device-policy.service" "$unit_directory/pid1-device-policy.service"
/usr/bin/install -m 0644 "$project/tests/pid1-private-users.service" "$unit_directory/pid1-private-users.service"
/usr/bin/install -m 0644 "$project/tests/pid1-control.service" "$unit_directory/pid1-control.service"
/usr/bin/rm -rf /run/fractald-pid1 /run/fractald-pid1-state
/usr/bin/rm -rf /var/lib/fractald-pid1 /root/fractald-pid1-test
