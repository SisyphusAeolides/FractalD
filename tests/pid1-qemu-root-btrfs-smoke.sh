#!/bin/sh
set -eu

project_directory=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
qemu_binary=${FRACTALD_QEMU_BINARY:-qemu-system-x86_64}
kernel_image=${FRACTALD_QEMU_KERNEL:-/boot/vmlinuz-$(uname -r)}
qemu_cpu=${FRACTALD_QEMU_CPU:-max}
timeout_seconds=${FRACTALD_QEMU_TIMEOUT:-30}
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/fractald-qemu-root-btrfs.XXXXXX")
cleanup() {
    if [ "${FRACTALD_QEMU_KEEP_ARTIFACTS:-0}" = 1 ]; then
        printf 'PID1 root Btrfs smoke artifacts: %s\n' "$work_directory" >&2
    else
        rm -rf "$work_directory"
    fi
}
trap cleanup EXIT HUP INT TERM

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'PID1 root Btrfs smoke: missing required command: %s\n' "$1" >&2
        exit 1
    }
}

require_command mkfs.btrfs
require_command btrfs
require_command cpio
require_command timeout
require_command "$qemu_binary"
[ -r "$kernel_image" ] || {
    printf 'PID1 root Btrfs smoke: kernel image is missing: %s\n' "$kernel_image" >&2
    exit 1
}

root_tree="$work_directory/root-tree"
mkdir -p "$root_tree"/dev "$root_tree"/proc "$root_tree"/sys "$root_tree"/run \
    "$root_tree"/tmp "$root_tree"/srv/pool "$root_tree"/etc

make -C "$project_directory" \
    DESTDIR="$root_tree" \
    PREFIX=/usr \
    BINDIR=/usr/bin \
    LIBEXECDIR=/usr/lib \
    SERVICE_DIR=/usr/lib/fractald/services \
    INSTALL_ALPM_HOOK=0 \
    install >"$work_directory/install.log"

copy_library() {
    source=$1
    case "$source" in
        /lib/*|/lib64/*|/usr/lib/*|/usr/lib64/*) ;;
        *) return 0 ;;
    esac
    destination="$root_tree$source"
    [ -e "$destination" ] && return 0
    mkdir -p "$(dirname -- "$destination")"
    cp -L "$source" "$destination"
}

copy_binary() {
    source=$1
    destination=$2
    [ -x "$source" ] || {
        printf 'PID1 root Btrfs smoke: binary is missing: %s\n' "$source" >&2
        exit 1
    }
    mkdir -p "$root_tree$(dirname -- "$destination")"
    cp -L "$source" "$root_tree$destination"
    ldd "$source" 2>/dev/null | awk '
        /=> \/(lib|usr\/lib)/ { print $3 }
        $1 ~ /^\/(lib|usr\/lib)/ { print $1 }
    ' | while read -r library; do
        [ -n "$library" ] || continue
        copy_library "$library"
    done
}

for native_binary in "$project_directory"/target/release/*; do
    if [ -f "$native_binary" ] && [ -x "$native_binary" ]; then
        copy_binary "$native_binary" "/usr/bin/$(basename -- "$native_binary")"
    fi
done

shell_binary=${FRACTALD_INITRAMFS_SHELL_BINARY:-}
if [ -z "$shell_binary" ]; then
    for candidate in /bin/sh /usr/bin/sh; do
        if [ -x "$candidate" ]; then
            shell_binary=$candidate
            break
        fi
    done
fi
[ -n "$shell_binary" ] || {
    printf '%s\n' 'PID1 root Btrfs smoke: no POSIX shell is available' >&2
    exit 1
}
copy_binary "$shell_binary" /bin/sh
copy_binary "$(command -v btrfs)" /usr/bin/btrfs
for applet in cat chaos chroot dmesg echo env false findmnt init insmod kill ln ls mkdir modprobe mount mv pwd readlink rm rmdir sleep sort swapoff swapon switch_root sync tr true umount uname which; do
    ln -sfn /usr/bin/rustybox "$root_tree/bin/$applet"
done

cat >"$root_tree/etc/fstab" <<'FSTAB'
/dev/vda / btrfs defaults 0 0
tmpfs /srv/pool tmpfs nosuid,nodev 0 0
FSTAB
mkdir -p "$root_tree/etc/fractald/services" "$root_tree/etc/fractald"
cat >"$root_tree/etc/fractald/boot.conf" <<'BOOT'
profile=boot
init=/usr/bin/fractald
BOOT
cat >"$root_tree/etc/fractald/services/root-btrfs-check.svc" <<'SERVICE'
[service]
description=Installed-root Btrfs PID1 handoff check
kind=oneshot
exec=/bin/sh -c 'set -eu; i=0; while [ "$$i" -lt 200 ] && [ ! -S /run/fractald/control.sock ]; do i=$$((i + 1)); /usr/bin/rustybox sleep 0.01; done; [ -S /run/fractald/control.sock ]; [ -S /run/fractald/journal/socket ]; status=$$(/usr/bin/fractalctl status fractald-journald); case "$$status" in *": running "*|*": active "*) ;; *) echo "$$status" >&2; exit 1 ;; esac; info=$$(/usr/bin/btrfs filesystem show /); echo "$$info"; case "$$info" in *"Total devices 2"*) ;; *) echo "installed root did not retain its second Btrfs device" >&2; exit 1 ;; esac; /usr/bin/rustybox findmnt --mountpoint /srv/pool --output TARGET,SOURCE,FSTYPE; echo FRACTALD_ROOT_BTRFS_CHECK_PASS >&2'
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

root_disk="$work_directory/root.raw"
second_disk="$work_directory/root-second.raw"
truncate -s 768M "$root_disk"
truncate -s 256M "$second_disk"
mkfs.btrfs -f -L fractald-root -d single -m dup -r "$root_tree" "$root_disk" \
    >"$work_directory/mkfs.log" 2>&1

init_script="$work_directory/init"
cat >"$init_script" <<'INIT'
#!/bin/sh
set -eu
/bin/mkdir -p /proc /sys /dev /run /newroot
/bin/mount -t proc proc /proc
/bin/mount -t sysfs sysfs /sys
/bin/mount -t devtmpfs devtmpfs /dev
/bin/mount -t tmpfs -o mode=0755 tmpfs /run
/usr/bin/btrfs device scan /dev/vda
/bin/mount -t btrfs /dev/vda /newroot
/usr/bin/btrfs device add /dev/vdb /newroot
exec /bin/switch_root /newroot /usr/bin/fractald
INIT
chmod 0755 "$init_script"

image="$work_directory/fractald-pid1-root-btrfs.img"
FRACTALD_PID1_INITRAMFS_OUTPUT="$image" \
FRACTALD_INITRAMFS_INIT="$init_script" \
FRACTALD_INITRAMFS_BTRFS_BINARY="$(command -v btrfs)" \
    "$project_directory/tests/pid1-initramfs-build.sh" \
    >"$work_directory/initramfs.log"

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
    -drive "file=$root_disk,format=raw,if=virtio,id=root" \
    -drive "file=$second_disk,format=raw,if=virtio,id=root_second" \
    -nographic \
    -serial mon:stdio \
    -no-reboot \
    >"$qemu_log" 2>&1
qemu_status=$?
set -e

if [ "$qemu_status" -eq 124 ]; then
    printf '%s\n' 'PID1 root Btrfs smoke: the installed-root check did not complete' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
fi
if [ "$qemu_status" -ne 0 ]; then
    printf 'PID1 root Btrfs smoke: QEMU exited unexpectedly with status %s\n' "$qemu_status" >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
fi
grep -F 'Run /init as init process' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 root Btrfs smoke: the kernel did not start the initramfs entrypoint' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
}
if grep -E 'Kernel panic|Attempted to kill init|cannot prepare PID1|cannot start .*boot profile|service supervision failed|switch_root: cannot' "$qemu_log" >/dev/null; then
    printf '%s\n' 'PID1 root Btrfs smoke: boot reported a fatal PID1 or root handoff error' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
fi
grep -E 'BTRFS info \(device v[^)]*\): first mount' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 root Btrfs smoke: the installed root was not mounted' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
}
grep -F 'FRACTALD_ROOT_BTRFS_CHECK_PASS' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 root Btrfs smoke: the installed-root service check failed' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
}
grep -F 'Total devices 2' "$qemu_log" >/dev/null || {
    printf '%s\n' 'PID1 root Btrfs smoke: the second root filesystem device was not visible' >&2
    tail -n 160 "$qemu_log" >&2
    exit 1
}

printf '%s\n' 'PID1 installed-root Btrfs smoke: PASS (root handoff and two-device filesystem)'
