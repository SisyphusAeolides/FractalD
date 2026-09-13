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

unpack_base
copy_binary "$fractald_binary" /usr/bin/fractald
copy_binary "$rustybox_binary" /usr/bin/rustybox
copy_binary "$ctl_binary" /usr/bin/fractalctl
copy_binary "$trigger_binary" /usr/bin/fractald-package-trigger
copy_binary "$udev_binary" /usr/bin/fractald-udevadm
copy_binary "$journald_binary" /usr/bin/fractald-journald
ln -s fractald-udevadm "$root/usr/bin/fractald-udevd"

mkdir -p "$root/usr/lib/fractald/services"
for service in "$project_directory"/packaging/arch/services/*.svc; do
    cp "$service" "$root/usr/lib/fractald/services/"
done
cp -L "$root/usr/bin/fractald" "$root/init"
chmod 0755 "$root/init"

mkdir -p "$(dirname -- "$output_image")"
(cd "$root" && find . -print0 | cpio --null -o -H newc 2>/dev/null | zstd -T0 -19 -q -f -o "$output_image")

printf 'initramfs image: %s\n' "$output_image"
if command -v file >/dev/null 2>&1; then
    file "$output_image"
fi
