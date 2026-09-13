#!/bin/sh
set -eu

project_directory=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
qemu_binary=${FRACTALD_QEMU_BINARY:-qemu-system-x86_64}
kernel_image=${FRACTALD_QEMU_KERNEL:-/boot/vmlinuz-$(uname -r)}
qemu_cpu=${FRACTALD_QEMU_CPU:-max}
timeout_seconds=${FRACTALD_QEMU_TIMEOUT:-20}
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/fractald-qemu-btrfs.XXXXXX")
cleanup() {
    if [ "${FRACTALD_QEMU_KEEP_ARTIFACTS:-0}" = 1 ]; then
        printf 'PID1 Btrfs smoke artifacts: %s\n' "$work_directory" >&2
    else
        rm -rf "$work_directory"
    fi
}
trap cleanup EXIT HUP INT TERM

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'PID1 Btrfs smoke: missing required command: %s\n' "$1" >&2
        exit 1
    }
}

require_command mkfs.btrfs
require_command blkid
require_command btrfs
require_command timeout
require_command "$qemu_binary"
[ -r "$kernel_image" ] || {
    printf 'PID1 Btrfs smoke: kernel image is missing: %s\n' "$kernel_image" >&2
    exit 1
}

disk_a="$work_directory/pool-a.raw"
disk_b="$work_directory/pool-b.raw"
truncate -s 256M "$disk_a"
truncate -s 256M "$disk_b"
mkfs.btrfs -f -L fractald-test -d single -m raid1 "$disk_a" "$disk_b" \
    >"$work_directory/mkfs.log" 2>&1
pool_uuid=$(blkid -s UUID -o value "$disk_a")
[ -n "$pool_uuid" ] || {
    printf '%s\n' 'PID1 Btrfs smoke: could not read the test filesystem UUID' >&2
    exit 1
}

fstab_file="$work_directory/fstab"
cat >"$fstab_file" <<FSTAB
UUID=$pool_uuid /srv/pool btrfs defaults,compress=zstd:1 0 0
FSTAB

service_directory="$work_directory/services"
mkdir -p "$service_directory"
cp "$project_directory"/packaging/arch/services/*.svc "$service_directory/"
cat >"$service_directory/pool-check.svc" <<'SERVICE'
[service]
description=Two-device Btrfs pool boot check
kind=oneshot
exec=/bin/sh -c 'set -eu; echo FRACTALD_BTRFS_CHECK_START >&2; i=0; while [ "$$i" -lt 100 ] && [ ! -S /run/fractald/control.sock ]; do i=$$((i + 1)); sleep 0.01; done; [ -S /run/fractald/control.sock ]; [ -S /run/fractald/journal/socket ]; status=$$(/usr/bin/fractalctl status fractald-journald); case "$$status" in *": running "*|*": active "*) ;; *) echo "$$status" >&2; exit 1 ;; esac; status=$$(/usr/bin/fractalctl status fractald-udevd); case "$$status" in *": running "*|*": active "*) ;; *) echo "$$status" >&2; exit 1 ;; esac; /usr/bin/rustybox findmnt --mountpoint /srv/pool --output TARGET,SOURCE,FSTYPE; /usr/bin/btrfs filesystem show /srv/pool; echo FRACTALD_BTRFS_CHECK_PASS >&2'
remain_after_exit=true
stdout=inherit
stderr=inherit

[dependencies]
requires=mount-srv-pool fractald-journald fractald-udevd
after=mount-srv-pool fractald-journald fractald-udevd

[manager]
success_action=poweroff
failure_action=poweroff

[install]
profile=boot
SERVICE

image="$work_directory/fractald-pid1-btrfs.img"
FRACTALD_PID1_INITRAMFS_OUTPUT="$image" \
FRACTALD_INITRAMFS_SERVICE_DIR="$service_directory" \
FRACTALD_INITRAMFS_FSTAB="$fstab_file" \
FRACTALD_INITRAMFS_BLKID_BINARY="$(command -v blkid)" \
FRACTALD_INITRAMFS_BTRFS_BINARY="$(command -v btrfs)" \
    "$project_directory/tests/pid1-initramfs-build.sh" \
    >"$work_directory/build.log"

qemu_log="$work_directory/qemu.log"
set +e
timeout --signal=TERM --kill-after=3s "${timeout_seconds}s" \
    "$qemu_binary" \
    -M pc,accel=tcg \
    -cpu "$qemu_cpu" \
    -m 512M \
    -smp 1 \
    -kernel "$kernel_image" \
    -initrd "$image" \
    -append 'console=ttyS0,115200 earlycon=uart,io,0x3f8,115200 init=/init panic=-1 nokaslr loglevel=7' \
    -drive "file=$disk_a,format=raw,if=virtio,id=pool_a" \
    -drive "file=$disk_b,format=raw,if=virtio,id=pool_b" \
    -nographic \
    -serial mon:stdio \
    -no-reboot \
    >"$qemu_log" 2>&1
qemu_status=$?
set -e

if [ "$qemu_status" -eq 124 ]; then
    printf '%s\n' 'PID1 Btrfs smoke: the pool-check service did not complete' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
fi
if [ "$qemu_status" -ne 0 ]; then
    printf 'PID1 Btrfs smoke: QEMU exited unexpectedly with status %s\n' "$qemu_status" >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
fi
grep -F 'Run /init as init process' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 Btrfs smoke: the kernel did not start the FractalD init entrypoint' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
}
if grep -E 'Kernel panic|Attempted to kill init|cannot prepare PID1|cannot start .*boot profile|service supervision failed' "$qemu_log" >/dev/null; then
    printf '%s\n' 'PID1 Btrfs smoke: boot reported a fatal PID1 or service error' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
fi
grep -E 'BTRFS info \(device v[^)]*\): first mount' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 Btrfs smoke: the guest did not mount the Btrfs pool' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
}
grep -F 'FRACTALD_BTRFS_CHECK_PASS' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 Btrfs smoke: the native pool-check service failed' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
}
grep -F 'devid 2' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 Btrfs smoke: the guest did not report both pool devices' >&2
    tail -n 120 "$qemu_log" >&2
    exit 1
}

printf 'PID1 Btrfs smoke: PASS (UUID %s, two guest devices)\n' "$pool_uuid"
