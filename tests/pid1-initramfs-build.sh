#!/bin/sh
set -eu

project_directory=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
base_image=${FRACTALD_BASE_INITRAMFS:?FRACTALD_BASE_INITRAMFS must name a Fedora initramfs}
output_image=${FRACTALD_PID1_INITRAMFS_OUTPUT:-$project_directory/target/fractald-pid1-initramfs.img}
switch_root_path=${FRACTALD_SWITCH_ROOT:-$(command -v switch_root)}
extra_module_directory=${FRACTALD_INITRAMFS_MODULE_DIR:-}
module_release=${FRACTALD_INITRAMFS_KERNEL_RELEASE:-}
profile=${FRACTALD_INITRAMFS_PROFILE:-base}
test_init=${FRACTALD_INITRAMFS_TEST_INIT:-}
fractald_binary=${FRACTALD_INITRAMFS_FRACTALD_BINARY:-$project_directory/target/release/fractald}
rustybox_binary=${FRACTALD_INITRAMFS_RUSTYBOX_BINARY:-$project_directory/target/release/rustybox}
work_directory=$(mktemp -d "${TMPDIR:-/tmp}/fractald-initramfs.XXXXXX")
trap 'rm -rf "$work_directory"' EXIT HUP INT TERM

case "$profile" in
    base|static) ;;
    *)
        printf 'unknown initramfs profile: %s\n' "$profile" >&2
        exit 1
        ;;
esac

mkdir -p "$work_directory/root"

copy_static_library() {
    library=$1
    case "$library" in
        /lib/*|/lib64/*|/usr/lib/*|/usr/lib64/*) ;;
        *) return 0 ;;
    esac
    source_path="$work_directory/source$library"
    [ -e "$source_path" ] || source_path=$library
    [ -e "$source_path" ] || return 0
    marker_name=$(printf '%s' "$library" | tr '/:' '__')
    marker="$work_directory/seen/$marker_name"
    [ -e "$marker" ] && return 0
    mkdir -p "$work_directory/seen"
    : >"$marker"
    mkdir -p "$work_directory/root$(dirname -- "$library")"
    cp -L "$source_path" "$work_directory/root$library"
    while read -r dependency; do
        [ -n "$dependency" ] || continue
        copy_static_library "$dependency"
    done <<EOF
$(ldd "$source_path" 2>/dev/null | awk '/=> \/(lib|usr\/lib)/ {print $3} $1 ~ /^\/(lib|usr\/lib)/ {print $1}')
EOF
}

copy_static_binary() {
    image_path=$1
    source_path="$work_directory/source$image_path"
    [ -e "$source_path" ] || source_path=$image_path
    [ -e "$source_path" ] || {
        printf 'static initramfs source is missing %s\n' "$image_path" >&2
        exit 1
    }
    mkdir -p "$work_directory/root$(dirname -- "$image_path")"
    cp -L "$source_path" "$work_directory/root$image_path"
    copy_static_library_dependencies="$source_path"
    while read -r dependency; do
        [ -n "$dependency" ] || continue
        copy_static_library "$dependency"
    done <<EOF
$(ldd "$copy_static_library_dependencies" 2>/dev/null | awk '/=> \/(lib|usr\/lib)/ {print $3} $1 ~ /^\/(lib|usr\/lib)/ {print $1}')
EOF
}

copy_external_binary() {
    source_path=$1
    image_path=$2
    [ -x "$source_path" ] || {
        printf 'static initramfs binary is missing: %s\n' "$source_path" >&2
        exit 1
    }
    mkdir -p "$work_directory/root$(dirname -- "$image_path")"
    cp -L "$source_path" "$work_directory/root$image_path"
    while read -r dependency; do
        [ -n "$dependency" ] || continue
        copy_static_library "$dependency"
    done <<EOF
$(ldd "$source_path" 2>/dev/null | awk '/=> \/(lib|usr\/lib)/ {print $3} $1 ~ /^\/(lib|usr\/lib)/ {print $1}')
EOF
}

if [ "$profile" = static ]; then
    mkdir -p "$work_directory/source"
    (cd "$work_directory/source" && lsinitrd --unpack "$base_image" >/dev/null || true)
    mkdir -p "$work_directory/root"/dev "$work_directory/root"/proc \
        "$work_directory/root"/sys "$work_directory/root"/run "$work_directory/root"/tmp

    for command in \
        /bin/sh /usr/bin/btrfs /usr/bin/blkid /usr/bin/cp /usr/bin/cryptsetup \
        /usr/bin/insmod /usr/bin/modprobe /usr/bin/mount /usr/bin/umount /usr/bin/sed \
        /usr/bin/mdadm /usr/bin/lvm; do
        copy_static_binary "$command"
    done

    for directory in /etc/crypto-policies /etc/depmod.d /etc/kmod /etc/lvm \
        /etc/modprobe.d /etc/cryptsetup; do
        if [ -e "$work_directory/source$directory" ]; then
            mkdir -p "$work_directory/root$(dirname -- "$directory")"
            cp -a "$work_directory/source$directory" "$work_directory/root$(dirname -- "$directory")/"
        fi
    done
    for file in /etc/group /etc/ld.so.conf /etc/nsswitch.conf /etc/passwd; do
        if [ -e "$work_directory/source$file" ]; then
            mkdir -p "$work_directory/root$(dirname -- "$file")"
            cp -a "$work_directory/source$file" "$work_directory/root$file"
        fi
    done
else
    (cd "$work_directory/root" && lsinitrd --unpack "$base_image" >/dev/null || true)
    test -f "$work_directory/root/init"
    test -x "$switch_root_path"
fi

copy_external_binary "$fractald_binary" /usr/bin/fractald
copy_external_binary "$rustybox_binary" /usr/bin/rustybox

if [ -n "$test_init" ]; then
    test_init_path=$test_init
    case "$test_init_path" in
        /*) ;;
        *) test_init_path="$project_directory/$test_init_path" ;;
    esac
    [ -f "$test_init_path" ] || {
        printf 'initramfs test entrypoint is missing: %s\n' "$test_init_path" >&2
        exit 1
    }
    mkdir -p "$work_directory/root/sbin"
    cp "$test_init_path" "$work_directory/root/sbin/fractald-current-pid1-test"
    chmod 0755 "$work_directory/root/sbin/fractald-current-pid1-test"
    copy_external_binary "$project_directory/target/release/fractalctl" /usr/bin/fractalctl
fi

# Keep the distribution kmod implementations behind RustyBox's native
# applet names. RustyBox can load uncompressed modules directly and delegates
# compressed modules, aliases, and dependency policy to these helpers.
mkdir -p "$work_directory/root/usr/libexec/fractald/kmod"
for applet in insmod modprobe; do
    cp -a "$work_directory/root/usr/bin/$applet" \
        "$work_directory/root/usr/libexec/fractald/kmod/$applet"
done

if [ "$profile" = static ]; then
    for command in btrfs blkid cryptsetup insmod mdadm lvm modprobe sed; do
        test -x "$work_directory/root/usr/bin/$command" || {
            printf 'static initramfs is missing /usr/bin/%s\n' "$command" >&2
            exit 1
        }
    done
    if find "$work_directory/root" -mindepth 1 \
        \( -path '*/etc/systemd*' -o -path '*/usr/lib/systemd*' \
        -o -name systemd -o -name 'systemd-*' \) -print -quit | grep -q .; then
        printf '%s\n' 'static initramfs contains a systemd manager path' >&2
        exit 1
    fi
    find "$work_directory/root/etc/modprobe.d" -name '*systemd*' -delete 2>/dev/null || true
fi

# These applets are part of the initramfs contract. Keep the distribution
# mount dispatcher and its storage helpers available for filesystems that
# need userspace support, while using RustyBox for the bounded primitives
# FractalD owns directly.
for applet in cat dmesg findmnt insmod mkdir modprobe sleep sort switch_root sync tr uname; do
    applet_path="$work_directory/root/usr/bin/$applet"
    if [ -e "$applet_path" ] || [ -L "$applet_path" ]; then
        rm "$applet_path"
    fi
    ln -s rustybox "$applet_path"
done
if [ "$profile" = static ]; then
    for applet in cat dmesg findmnt insmod mkdir modprobe sleep sort switch_root sync tr uname; do
        test "$(readlink "$work_directory/root/usr/bin/$applet")" = rustybox || {
            printf 'static initramfs applet is not owned by RustyBox: %s\n' "$applet" >&2
            exit 1
        }
    done
fi

rm -f "$work_directory/root/init"
cp "$project_directory/tests/pid1-initramfs-init.sh" "$work_directory/root/init"
chmod 0755 "$work_directory/root/init"

if [ "$profile" = static ] && [ -z "$module_release" ]; then
    for module_directory in "$work_directory/source/lib/modules" \
        "$work_directory/source/usr/lib/modules"; do
        if [ -d "$module_directory" ]; then
            for module_release_path in "$module_directory"/*; do
                [ -d "$module_release_path" ] || continue
                module_release=$(basename -- "$module_release_path")
                break 2
            done
        fi
    done
fi

if [ "$profile" = static ] && [ -n "$module_release" ]; then
    for module_directory in "$work_directory/source/lib/modules/$module_release" \
        "$work_directory/source/usr/lib/modules/$module_release"; do
        if [ -d "$module_directory" ]; then
            module_target="$work_directory/root/lib/modules/$module_release"
            mkdir -p "$module_target"
            cp -a "$module_directory"/. "$module_target/"
            break
        fi
    done
fi

if [ -n "$extra_module_directory" ]; then
    [ -n "$module_release" ] || {
        printf '%s\n' 'FRACTALD_INITRAMFS_KERNEL_RELEASE is required with FRACTALD_INITRAMFS_MODULE_DIR' >&2
        exit 1
    }
    test -d "$extra_module_directory"
    module_target="$work_directory/root/lib/modules/$module_release"
    mkdir -p "$module_target"
    cp -a "$extra_module_directory"/. "$module_target/"
    if command -v depmod >/dev/null 2>&1; then
        depmod -b "$work_directory/root" "$module_release"
    fi
elif [ "$profile" = static ] && [ -n "$module_release" ] \
    && [ -d "$work_directory/root/lib/modules/$module_release" ]; then
    if command -v depmod >/dev/null 2>&1; then
        depmod -b "$work_directory/root" "$module_release"
    fi
fi
mkdir -p "$(dirname -- "$output_image")"

(cd "$work_directory/root" &&
    find . -print0 |
    cpio --null -o -H newc 2>/dev/null |
    zstd -T0 -19 -q -f -o "$output_image")

if [ "$profile" = static ]; then
    image_listing=$(lsinitrd "$output_image")
    for path in 'bin/sh' 'init' 'usr/bin/fractald' 'usr/bin/rustybox'; do
        printf '%s\n' "$image_listing" | grep -Eq "[[:space:]]$path( |$)" || {
            printf 'static initramfs is missing %s\n' "$path" >&2
            exit 1
        }
    done
    if [ -n "$test_init" ]; then
        for path in 'sbin/fractald-current-pid1-test' 'usr/bin/fractalctl'; do
            printf '%s\n' "$image_listing" | grep -Eq "[[:space:]]$path( |$)" || {
                printf 'static initramfs test image is missing %s\n' "$path" >&2
                exit 1
            }
        done
    fi
    if printf '%s\n' "$image_listing" | grep -Eq \
        '(^|[[:space:]])(etc/systemd|usr/lib/systemd|usr/bin/systemd|usr/sbin/systemd)(/|[[:space:]]|$)'; then
        printf '%s\n' 'static initramfs contains a systemd manager path' >&2
        exit 1
    fi
fi

file "$output_image"
printf 'initramfs image: %s\n' "$output_image"
