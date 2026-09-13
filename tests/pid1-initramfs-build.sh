#!/bin/sh
set -eu

project_directory=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
base_image=${FRACTALD_BASE_INITRAMFS:-}
output_image=${FRACTALD_PID1_INITRAMFS_OUTPUT:-$project_directory/target/fractald-pid1-initramfs.img}
fractald_binary=${FRACTALD_INITRAMFS_FRACTALD_BINARY:-$project_directory/target/release/fractald}
rustybox_binary=${FRACTALD_INITRAMFS_RUSTYBOX_BINARY:-$project_directory/target/release/rustybox}
ctl_binary=${FRACTALD_INITRAMFS_FRACTALCTL_BINARY:-$project_directory/target/release/fractalctl}
trigger_binary=${FRACTALD_INITRAMFS_TRIGGER_BINARY:-$project_directory/target/release/fractald-package-trigger}
udev_binary=${FRACTALD_INITRAMFS_UDEV_BINARY:-$project_directory/target/release/fractald-udevadm}
journald_binary=${FRACTALD_INITRAMFS_JOURNALD_BINARY:-$project_directory/target/release/fractald-journald}
service_source_directory=${FRACTALD_INITRAMFS_SERVICE_DIR:-$project_directory/packaging/arch/services}
fstab_file=${FRACTALD_INITRAMFS_FSTAB:-}
blkid_binary=${FRACTALD_INITRAMFS_BLKID_BINARY:-}
btrfs_binary=${FRACTALD_INITRAMFS_BTRFS_BINARY:-}
shell_binary=${FRACTALD_INITRAMFS_SHELL_BINARY:-}
init_binary_or_script=${FRACTALD_INITRAMFS_INIT:-}
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/fractald-initramfs.XXXXXX")
trap 'rm -rf "$work_directory"' EXIT HUP INT TERM

root="$work_directory/root"
mkdir -p "$root"/dev "$root"/proc "$root"/sys "$root"/run "$root"/tmp

unpack_base() {
    [ -n "$base_image" ] || return 0
    [ -f "$base_image" ] || {
        printf 'initramfs image is missing: %s\n' "$base_image" >&2
        exit 1
    }
    if command -v lsinitcpio >/dev/null 2>&1; then
        lsinitcpio -x "$base_image" -d "$root"
    elif command -v bsdtar >/dev/null 2>&1; then
        bsdtar -xf "$base_image" -C "$root"
    else
        printf '%s\n' 'lsinitcpio or bsdtar is required to unpack the base image' >&2
        exit 1
    fi
}

copy_library() {
    source=$1
    [ -e "$source" ] || return 0
    case "$source" in
        /lib/*|/lib64/*|/usr/lib/*|/usr/lib64/*) ;;
        *) return 0 ;;
    esac
    destination="$root$source"
    [ -e "$destination" ] && return 0
    mkdir -p "$(dirname -- "$destination")"
    cp -L "$source" "$destination"
}

copy_binary() {
    source=$1
    destination=$2
    [ -x "$source" ] || {
        printf 'initramfs binary is missing: %s\n' "$source" >&2
        exit 1
    }
    mkdir -p "$root$(dirname -- "$destination")"
    cp -L "$source" "$root$destination"
    ldd "$source" 2>/dev/null | awk '
        /=> \/(lib|usr\/lib)/ { print $3 }
        $1 ~ /^\/(lib|usr\/lib)/ { print $1 }
    ' | while read -r library; do
        [ -n "$library" ] || continue
        copy_library "$library"
    done
}

find_host_binary() {
    for candidate in "$@"; do
        if [ -x "$candidate" ]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    return 1
}

unpack_base
copy_binary "$fractald_binary" /usr/bin/fractald
copy_binary "$rustybox_binary" /usr/bin/rustybox
copy_binary "$ctl_binary" /usr/bin/fractalctl
copy_binary "$trigger_binary" /usr/bin/fractald-package-trigger
copy_binary "$udev_binary" /usr/bin/fractald-udevadm
copy_binary "$journald_binary" /usr/bin/fractald-journald
ln -s fractald-udevadm "$root/usr/bin/fractald-udevd"

if [ ! -x "$root/bin/sh" ]; then
    if [ -z "$shell_binary" ]; then
        shell_binary=$(find_host_binary /bin/sh /usr/bin/sh || true)
    fi
    [ -n "$shell_binary" ] || {
        printf '%s\n' 'initramfs requires a POSIX shell or a base initramfs that provides /bin/sh' >&2
        exit 1
    }
    copy_binary "$shell_binary" /bin/sh
fi

if [ -z "$blkid_binary" ]; then
    blkid_binary=$(find_host_binary /usr/bin/blkid /usr/sbin/blkid || true)
fi
if [ -n "$blkid_binary" ]; then
    copy_binary "$blkid_binary" /usr/bin/blkid
fi

if [ -z "$btrfs_binary" ]; then
    btrfs_binary=$(find_host_binary /usr/bin/btrfs /usr/sbin/btrfs || true)
fi
if [ -n "$btrfs_binary" ]; then
    copy_binary "$btrfs_binary" /usr/bin/btrfs
fi

if [ -n "$fstab_file" ]; then
    [ -f "$fstab_file" ] || {
        printf 'initramfs fstab is missing: %s\n' "$fstab_file" >&2
        exit 1
    }
    install -Dm644 "$fstab_file" "$root/etc/fstab"
fi

mkdir -p "$root/usr/lib/fractald/services" "$root/usr/lib/fractald/toolbox"
[ -d "$service_source_directory" ] || {
    printf 'initramfs service directory is missing: %s\n' "$service_source_directory" >&2
    exit 1
}
find "$service_source_directory" -maxdepth 1 -type f -name '*.svc' -exec cp '{}' "$root/usr/lib/fractald/services/" \;
for applet in mount umount swapon swapoff mkdir mv rm rmdir; do
    ln -sfn /usr/bin/rustybox "$root/usr/lib/fractald/toolbox/$applet"
done
for applet in cat dmesg echo env false findmnt init kill ln ls mkdir mount mv pwd readlink rm rmdir sleep sort swapoff swapon switch_root sync tr true umount uname which; do
    ln -sfn /usr/bin/rustybox "$root/bin/$applet"
done
if [ -n "$init_binary_or_script" ]; then
    [ -f "$init_binary_or_script" ] || {
        printf 'initramfs init entry is missing: %s\n' "$init_binary_or_script" >&2
        exit 1
    }
    install -Dm755 "$init_binary_or_script" "$root/init"
else
    cp -L "$root/usr/bin/fractald" "$root/init"
    chmod 0755 "$root/init"
fi

mkdir -p "$(dirname -- "$output_image")"
(cd "$root" && find . -print0 | cpio --null -o -H newc 2>/dev/null | zstd -T0 -19 -q -f -o "$output_image")

printf 'initramfs image: %s\n' "$output_image"
if command -v file >/dev/null 2>&1; then
    file "$output_image"
fi
