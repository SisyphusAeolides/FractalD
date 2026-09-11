#!/bin/sh
set -u

export PATH=/usr/bin:/usr/sbin:/bin:/sbin

log() {
    message="FractalD initramfs: $*"
    printf '%s\n' "$message" >&2
    if [ -n "${persistent_log:-}" ]; then
        printf '%s\n' "$message" >>"$persistent_log"
    fi
}

fatal() {
    log "fatal: $*"
    /usr/bin/dmesg >&2 2>/dev/null || true
    exit 1
}

rustybox_path=${FRACTALD_RUSTYBOX:-/usr/bin/rustybox}

mount_filesystem() {
    requested_mount_type=
    expect_mount_type=0
    for mount_argument in "$@"; do
        if [ "$expect_mount_type" -eq 1 ]; then
            requested_mount_type=$mount_argument
            expect_mount_type=0
            continue
        fi
        case "$mount_argument" in
            -t|--types) expect_mount_type=1 ;;
            -t*) requested_mount_type=${mount_argument#-t} ;;
            --types=*) requested_mount_type=${mount_argument#--types=} ;;
            --) break ;;
        esac
    done
    if [ -n "$requested_mount_type" ]; then
        requested_mount_type=$(canonical_filesystem_type "$requested_mount_type")
    fi
    if [ -x "$rustybox_path" ] && {
        [ -z "$requested_mount_type" ] || kernel_filesystem_type "$requested_mount_type"
    }; then
        if "$rustybox_path" mount "$@"; then
            return 0
        fi
        # A source can be a helper-backed filesystem even when no explicit
        # type was supplied. Let the distribution mount program retry after
        # RustyBox's direct kernel attempt fails.
    fi
    mount_helper=/usr/bin/mount
    [ -x "$mount_helper" ] || mount_helper=/bin/mount
    if [ -x "$mount_helper" ]; then
        "$mount_helper" "$@"
    elif [ -x "$rustybox_path" ]; then
        # A minimal image may omit util-linux. RustyBox can still mount the
        # filesystem directly when the kernel exposes a compatible interface.
        "$rustybox_path" mount "$@"
    else
        return 127
    fi
}

mount_pseudo() {
    /usr/bin/mkdir -p /proc /sys /dev /run
    mount_filesystem -t proc proc /proc 2>/dev/null || true
    mount_filesystem -t sysfs sysfs /sys 2>/dev/null || true
    mount_filesystem -t devtmpfs -o mode=0755 devtmpfs /dev 2>/dev/null || true
}

load_drivers() {
    for module in \
        nvme nvme_core ahci libahci ata_generic sd_mod sr_mod scsi_mod \
        usb_storage uas mmc_block \
        virtio_pci virtio_net virtio_blk virtio_scsi \
        ext4 xfs vfat exfat f2fs btrfs overlay \
        dm_mod dm_crypt dm_integrity dm_mirror dm_snapshot dm_thin_pool \
        dm_cache dm_cache_mq dm_log dm_zero \
        md_mod linear raid0 raid1 raid10 raid456; do
        /usr/bin/modprobe -q "$module" 2>/dev/null || true
    done
}

load_filesystem_driver() {
    case "$1" in
        auto|none|swap|tmpfs|proc|sysfs|devtmpfs|devpts|cgroup|cgroup2|bpf|binder|binfmt_misc|configfs|debugfs|efivarfs|hugetlbfs|mqueue|pipefs|pstore|ramfs|securityfs|selinuxfs|sockfs|tracefs)
            return 0
            ;;
        ext|ext2|ext3|ext4)
            module=ext4
            ;;
        fat|msdos|vfat)
            module=vfat
            ;;
        exfat|f2fs|xfs|btrfs|jfs|nilfs2|ocfs2|reiserfs|squashfs|udf|erofs|hfs|hfsplus|jffs2|ubifs|ntfs|ntfs3|overlay|lustre)
            module=$1
            ;;
        nfs|nfs4)
            module=nfs
            ;;
        cifs|smb3|smbfs)
            module=cifs
            ;;
        9p)
            module=9p
            ;;
        iso9660)
            module=isofs
            ;;
        fuse|fuseblk|sshfs|fuse.*)
            module=fuse
            ;;
        fusectl)
            module=fuse
            ;;
        nfsd)
            module=nfsd
            ;;
        rpc_pipefs)
            module=sunrpc
            ;;
        davfs|glusterfs)
            return 0
            ;;
        *)
            module=$1
            ;;
    esac
    /usr/bin/modprobe -q "$module" 2>/dev/null || true
}

canonical_filesystem_type() {
    filesystem_type=$(printf '%s\n' "$1" | /usr/bin/tr '[:upper:]' '[:lower:]')
    case "$filesystem_type" in
        fat|msdos) printf '%s\n' vfat ;;
        ext) printf '%s\n' ext4 ;;
        *) printf '%s\n' "$filesystem_type" ;;
    esac
}

kernel_filesystem_type() {
    filesystem_type=$(canonical_filesystem_type "$1")
    case "$filesystem_type" in
        fuse|fuseblk|fuse.*|autofs|ceph|cifs|davfs|glusterfs|lustre|nfs|nfs4|smb3|smbfs|sshfs)
            return 1
            ;;
    esac
    if [ -r /proc/filesystems ]; then
        while read -r filesystem_first filesystem_second; do
            filesystem_name=${filesystem_second:-$filesystem_first}
            [ "$filesystem_name" = "$filesystem_type" ] && return 0
        done </proc/filesystems
    fi
    case "$filesystem_type" in
        9p|adfs|affs|befs|bfs|bpf|binder|binfmt_misc|btrfs|cgroup|cgroup2|configfs|cramfs|debugfs|devpts|devtmpfs|efivarfs|erofs|exfat|ext|ext2|ext3|ext4|f2fs|fat|fusectl|hfs|hfsplus|hugetlbfs|isofs|iso9660|jffs2|jfs|minix|msdos|mqueue|nfsd|nilfs2|ntfs|ntfs3|overlay|pipefs|proc|pstore|qnx4|qnx6|ramfs|reiserfs|romfs|rpc_pipefs|securityfs|selinuxfs|sockfs|squashfs|sysfs|tmpfs|tracefs|udf|ubifs|ufs|vfat|xfs|zonefs)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

activate_block_storage() {
    if [ -x /usr/bin/btrfs ]; then
        /usr/bin/btrfs device scan --all-devices 2>&1 ||
            log "Btrfs device scan returned a nonzero status"
    fi

    if [ -x /usr/sbin/mdadm ] || [ -x /usr/bin/mdadm ]; then
        mdadm_path=/usr/sbin/mdadm
        [ -x "$mdadm_path" ] || mdadm_path=/usr/bin/mdadm
        "$mdadm_path" --assemble --scan 2>&1 ||
            log "mdraid assembly returned a nonzero status"
    fi

    if [ -x /usr/bin/lvm ] || [ -x /usr/sbin/lvm ]; then
        lvm_path=/usr/bin/lvm
        [ -x "$lvm_path" ] || lvm_path=/usr/sbin/lvm
        "$lvm_path" pvscan --cache 2>&1 ||
            log "LVM PV scan returned a nonzero status"
        "$lvm_path" vgchange --activate y 2>&1 ||
            log "LVM volume-group activation returned a nonzero status"
    fi
}

open_crypt_mapping() {
    mapping_name=$1
    mapping_source=$2
    mapping_key=${3:-}
    mapping_key_root=${4:-}
    mapping_options=${5:-}
    mapping_nofail=0
    case ",$mapping_options," in
        *,nofail,*) mapping_nofail=1 ;;
    esac

    crypt_source=$(resolve_block_source "$mapping_source") || {
        if [ "$mapping_nofail" -eq 1 ]; then
            log "optional encrypted source is unavailable: $mapping_name from $mapping_source"
            return 0
        fi
        fatal "cannot resolve encrypted source $mapping_source for $mapping_name"
    }
    "$cryptsetup_path" status "$mapping_name" >/dev/null 2>&1 && return 0

    mapping_has_key=0
    if [ -n "$mapping_key" ] && [ "$mapping_key" != none ] && [ "$mapping_key" != - ]; then
        key_path=$mapping_key
        case "$key_path" in
            /*) [ -n "$mapping_key_root" ] && key_path="$mapping_key_root$key_path" ;;
        esac
        mapping_has_key=1
    fi
    mapping_discard=0
    mapping_readonly=0
    case ",$mapping_options," in
        *,discard,*) mapping_discard=1 ;;
    esac
    case ",$mapping_options," in
        *,readonly,*) mapping_readonly=1 ;;
    esac

    if [ "$mapping_has_key" -eq 1 ]; then
        if [ "$mapping_discard" -eq 1 ]; then
            "$cryptsetup_path" open --allow-discards --key-file "$key_path" "$crypt_source" "$mapping_name" || {
                [ "$mapping_nofail" -eq 1 ] && {
                    log "optional encrypted mapping failed: $mapping_name"
                    return 0
                }
                fatal "cannot open encrypted mapping $mapping_name"
            }
        elif [ "$mapping_readonly" -eq 1 ]; then
            "$cryptsetup_path" open --readonly --key-file "$key_path" "$crypt_source" "$mapping_name" || {
                [ "$mapping_nofail" -eq 1 ] && {
                    log "optional encrypted mapping failed: $mapping_name"
                    return 0
                }
                fatal "cannot open encrypted mapping $mapping_name"
            }
        else
            "$cryptsetup_path" open --key-file "$key_path" "$crypt_source" "$mapping_name" || {
                [ "$mapping_nofail" -eq 1 ] && {
                    log "optional encrypted mapping failed: $mapping_name"
                    return 0
                }
                fatal "cannot open encrypted mapping $mapping_name"
            }
        fi
    elif [ "$mapping_discard" -eq 1 ]; then
        "$cryptsetup_path" open --allow-discards "$crypt_source" "$mapping_name" || {
            [ "$mapping_nofail" -eq 1 ] && {
                log "optional encrypted mapping failed: $mapping_name"
                return 0
            }
            fatal "cannot open encrypted mapping $mapping_name"
        }
    elif [ "$mapping_readonly" -eq 1 ]; then
        "$cryptsetup_path" open --readonly "$crypt_source" "$mapping_name" || {
            [ "$mapping_nofail" -eq 1 ] && {
                log "optional encrypted mapping failed: $mapping_name"
                return 0
            }
            fatal "cannot open encrypted mapping $mapping_name"
        }
    else
        "$cryptsetup_path" open "$crypt_source" "$mapping_name" || {
            [ "$mapping_nofail" -eq 1 ] && {
                log "optional encrypted mapping failed: $mapping_name"
                return 0
            }
            fatal "cannot open encrypted mapping $mapping_name"
        }
    fi
}

commandline_luks_field() {
    requested_uuid=$1
    field=$2
    for word in $(/usr/bin/cat /proc/cmdline 2>/dev/null); do
        case "$word" in
            "$field"=*)
                mapping=${word#*=}
                mapping_uuid=${mapping%%=*}
                mapping_uuid=${mapping_uuid#luks-}
                [ "$mapping_uuid" = "$requested_uuid" ] || continue
                printf '%s\n' "${mapping#*=}"
                return 0
                ;;
        esac
    done
    return 1
}

activate_commandline_crypt() {
    [ -x /usr/sbin/cryptsetup ] || [ -x /usr/bin/cryptsetup ] || return 0

    cryptsetup_path=/usr/sbin/cryptsetup
    [ -x "$cryptsetup_path" ] || cryptsetup_path=/usr/bin/cryptsetup
    crypt_disabled=0
    for word in $(/usr/bin/cat /proc/cmdline 2>/dev/null); do
        case "$word" in
            rd.luks=0) crypt_disabled=1 ;;
        esac
    done
    [ "$crypt_disabled" -eq 0 ] || return 0

    for word in $(/usr/bin/cat /proc/cmdline 2>/dev/null); do
        case "$word" in
            fractald.crypt=*)
                mapping=${word#*=}
                mapping_name=${mapping%%:*}
                mapping_source=${mapping#*:}
                [ -n "$mapping_name" ] && [ "$mapping_source" != "$mapping" ] ||
                    fatal "fractald.crypt must be name:source"
                open_crypt_mapping "$mapping_name" "$mapping_source"
                ;;
            cryptdevice=*)
                mapping=${word#*=}
                mapping_source=${mapping%%:*}
                mapping_name=${mapping#*:}
                [ -n "$mapping_source" ] && [ -n "$mapping_name" ] &&
                    [ "$mapping_name" != "$mapping" ] ||
                    fatal "cryptdevice must be source:name"
                open_crypt_mapping "$mapping_name" "$mapping_source"
                ;;
            rd.luks.uuid=*)
                mapping_uuid=${word#*=}
                mapping_uuid=${mapping_uuid#luks-}
                [ -n "$mapping_uuid" ] || fatal "rd.luks.uuid has an empty UUID"
                mapping_key=$(commandline_luks_field "$mapping_uuid" rd.luks.key || true)
                mapping_options=$(commandline_luks_field "$mapping_uuid" rd.luks.options || true)
                open_crypt_mapping "luks-$mapping_uuid" "UUID=$mapping_uuid" "$mapping_key" "" "$mapping_options"
                ;;
            rd.luks.name=*)
                mapping=${word#*=}
                mapping_uuid=${mapping%%=*}
                mapping_name=${mapping#*=}
                mapping_uuid=${mapping_uuid#luks-}
                [ -n "$mapping_uuid" ] && [ -n "$mapping_name" ] &&
                    [ "$mapping_name" != "$mapping" ] ||
                    fatal "rd.luks.name must be UUID=name"
                mapping_key=$(commandline_luks_field "$mapping_uuid" rd.luks.key || true)
                mapping_options=$(commandline_luks_field "$mapping_uuid" rd.luks.options || true)
                open_crypt_mapping "$mapping_name" "UUID=$mapping_uuid" "$mapping_key" "" "$mapping_options"
                ;;
        esac
    done
}

activate_crypttab_file() {
    crypttab_path=$1
    crypttab_key_root=${2:-}
    [ -r "$crypttab_path" ] || return 0
    while read -r mapping_name mapping_source mapping_key mapping_options; do
        case "$mapping_name" in
            ''|\#*) continue ;;
        esac
        [ -n "$mapping_source" ] && [ "$mapping_source" != none ] || continue
        mapping_source=$(printf '%s\n' "$mapping_source" | /usr/bin/sed 's/\\040/ /g; s/\\011/\t/g; s/\\134/\\/g')
        mapping_key=$(printf '%s\n' "${mapping_key:-}" | /usr/bin/sed 's/\\040/ /g; s/\\011/\t/g; s/\\134/\\/g')
        case ",${mapping_options:-}," in
            *,noauto,*) continue ;;
        esac
        open_crypt_mapping "$mapping_name" "$mapping_source" "$mapping_key" "$crypttab_key_root" "${mapping_options:-}"
    done <"$crypttab_path"
}

activate_initramfs_crypttab() {
    [ "${crypt_disabled:-0}" -eq 0 ] || return 0
    [ -x /usr/sbin/cryptsetup ] || [ -x /usr/bin/cryptsetup ] || return 0
    cryptsetup_path=/usr/sbin/cryptsetup
    [ -x "$cryptsetup_path" ] || cryptsetup_path=/usr/bin/cryptsetup
    activate_crypttab_file /etc/crypttab
}

resolve_block_source() {
    source=$1
    case "$source" in
        UUID=*)
            value=${source#*=}
            if [ -e "/dev/disk/by-uuid/$value" ]; then
                printf '%s\n' "/dev/disk/by-uuid/$value"
                return 0
            fi
            if [ -x /usr/bin/blkid ]; then
                /usr/bin/blkid -U "$value" 2>/dev/null && return 0
            fi
            ;;
        LABEL=*)
            value=${source#*=}
            if [ -e "/dev/disk/by-label/$value" ]; then
                printf '%s\n' "/dev/disk/by-label/$value"
                return 0
            fi
            if [ -x /usr/bin/blkid ]; then
                /usr/bin/blkid -L "$value" 2>/dev/null && return 0
            fi
            ;;
        PARTUUID=*)
            value=${source#*=}
            if [ -e "/dev/disk/by-partuuid/$value" ]; then
                printf '%s\n' "/dev/disk/by-partuuid/$value"
                return 0
            fi
            if [ -x /usr/bin/blkid ]; then
                resolved=$(
                    /usr/bin/blkid -t "PARTUUID=$value" -o device 2>/dev/null |
                        /usr/bin/sed -n '1p'
                )
                [ -n "$resolved" ] && {
                    printf '%s\n' "$resolved"
                    return 0
                }
            fi
            ;;
        PARTLABEL=*)
            value=${source#*=}
            if [ -e "/dev/disk/by-partlabel/$value" ]; then
                printf '%s\n' "/dev/disk/by-partlabel/$value"
                return 0
            fi
            if [ -x /usr/bin/blkid ]; then
                resolved=$(
                    /usr/bin/blkid -t "PARTLABEL=$value" -o device 2>/dev/null |
                        /usr/bin/sed -n '1p'
                )
                [ -n "$resolved" ] && {
                    printf '%s\n' "$resolved"
                    return 0
                }
            fi
            ;;
        /dev/*)
            printf '%s\n' "$source"
            return 0
            ;;
        *)
            printf '%s\n' "$source"
            return 0
            ;;
    esac
    return 1
}

mount_pseudo

root_source=
root_fstype=
root_flags=
init_program=/usr/bin/fractald
for word in $(/usr/bin/cat /proc/cmdline 2>/dev/null); do
    case "$word" in
        root=*) root_source=${word#*=} ;;
        rootfstype=*) root_fstype=${word#*=} ;;
        rootflags=*) root_flags=${word#*=} ;;
        fractald.root=*) root_source=${word#*=} ;;
        fractald.rootfstype=*) root_fstype=${word#*=} ;;
        fractald.rootflags=*) root_flags=${word#*=} ;;
        fractald.init=*) init_program=${word#*=} ;;
    esac
done

load_drivers
activate_block_storage
activate_commandline_crypt
activate_initramfs_crypttab
activate_block_storage

[ -n "$root_source" ] || fatal "no root= or fractald.root= kernel parameter"
sysroot=/sysroot
/usr/bin/mkdir -p "$sysroot"

mount_root() {
    mount_type=$(canonical_filesystem_type "${root_fstype:-auto}")
    load_filesystem_driver "$mount_type"
    if [ -n "$root_fstype" ] && [ -n "$root_flags" ]; then
        root_device=$(resolve_block_source "$root_source") || return 1
        mount_filesystem -t "$mount_type" -o "$root_flags" -- "$root_device" "$sysroot"
    elif [ -n "$root_fstype" ]; then
        root_device=$(resolve_block_source "$root_source") || return 1
        mount_filesystem -t "$mount_type" -- "$root_device" "$sysroot"
    elif [ -n "$root_flags" ]; then
        root_device=$(resolve_block_source "$root_source") || return 1
        mount_filesystem -o "$root_flags" -- "$root_device" "$sysroot"
    else
        root_device=$(resolve_block_source "$root_source") || return 1
        mount_filesystem -- "$root_device" "$sysroot"
    fi
}

load_root_drivers() {
    # The base initramfs may omit network and less common storage modules.
    # Once the root filesystem is mounted, use its matching module tree so
    # early services can bring up hardware that was not needed to find root.
    root_release=$(/usr/bin/uname -r 2>/dev/null || /usr/bin/cat /proc/sys/kernel/osrelease)
    module_root="$sysroot/lib/modules/$root_release"
    if /usr/bin/modprobe -q -d "$sysroot" -S "$root_release" virtio_net 2>/dev/null; then
        log "loaded root driver virtio_net"
        return 0
    else
        log "modprobe could not load root driver virtio_net; trying direct module insertion"
        # kmod versions in minimal initramfs images do not all honor a root
        # prefix when /lib is a symlink. Fall back to the matching files on
        # the mounted root, in dependency order.
        for module_path in \
            "$module_root"/kernel/net/core/failover.ko* \
            "$module_root"/kernel/drivers/net/net_failover.ko* \
            "$module_root"/kernel/drivers/net/virtio_net.ko*; do
            [ -f "$module_path" ] || continue
            /usr/bin/insmod "$module_path" 2>/tmp/fractald-initramfs-module-error ||
                log "could not insert $module_path: $(/usr/bin/cat /tmp/fractald-initramfs-module-error 2>/dev/null)"
        done
    fi
}

attempts=120
while ! mount_root 2>/tmp/fractald-initramfs-mount-error; do
    if [ "$attempts" -le 0 ]; then
        log "cannot mount root $root_source"
        /usr/bin/cat /tmp/fractald-initramfs-mount-error >&2 2>/dev/null || true
        fatal "root storage did not become available"
    fi
    /usr/bin/sleep 0.1
    attempts=$((attempts - 1))
done

/usr/bin/mkdir -p "$sysroot/root/fractald-pid1-test"
persistent_log="$sysroot/root/fractald-pid1-test/initramfs.log"
log "mounted root $root_source"
load_root_drivers

activate_crypttab() {
    [ "${crypt_disabled:-0}" -eq 0 ] || return 0
    [ -x /usr/sbin/cryptsetup ] || [ -x /usr/bin/cryptsetup ] || return 0
    cryptsetup_path=/usr/sbin/cryptsetup
    [ -x "$cryptsetup_path" ] || cryptsetup_path=/usr/bin/cryptsetup
    activate_crypttab_file "$sysroot/etc/crypttab" "$sysroot"
}

activate_crypttab
activate_block_storage
log 'early storage activation complete'

mount_options() {
    raw_options=$1
    cleaned_options=
    mount_nofail=0
    mount_noauto=0
    mount_netdev=0
    for option in $(printf '%s\n' "$raw_options" | /usr/bin/tr ',' ' '); do
        case "$option" in
            ''|defaults|auto) continue ;;
            noauto) mount_noauto=1 ;;
            nofail) mount_nofail=1 ;;
            _netdev) mount_netdev=1 ;;
            x-*|comment=*) continue ;;
            *)
                if [ -n "$cleaned_options" ]; then
                    cleaned_options="$cleaned_options,$option"
                else
                    cleaned_options=$option
                fi
                ;;
        esac
    done
    case "$fstype" in
        9p|ceph|cifs|davfs|fuse.ceph|fuse.glusterfs|fuse.sshfs|glusterfs|lustre|nfs|nfs4|smb3|smbfs|sshfs)
            mount_netdev=1
            ;;
    esac
    [ -n "$cleaned_options" ] || cleaned_options=defaults
}

mount_fstab_entry() {
    source=$1
    target=$2
    fstype=$(canonical_filesystem_type "$3")
    options=$4
    source=$(printf '%s\n' "$source" | /usr/bin/sed 's/\\040/ /g; s/\\011/\t/g; s/\\134/\\/g')
    target=$(printf '%s\n' "$target" | /usr/bin/sed 's/\\040/ /g; s/\\011/\t/g; s/\\134/\\/g')
    case "$target" in
        /|/proc|/sys|/dev|/run) return 0 ;;
    esac
    [ "$fstype" != swap ] || return 0
    mount_options "$options"
    [ "$mount_noauto" -eq 0 ] || return 0
    if [ "$mount_netdev" -eq 1 ]; then
        log "deferring network mount $target until FractalD networking is ready"
        return 0
    fi
    resolved_source=$(resolve_block_source "$source") || {
        if [ "$mount_nofail" -eq 1 ]; then
            log "optional mount source is unavailable: $target from $source"
            return 0
        fi
        fatal "cannot resolve mount source $source for $target"
    }
    mountpoint="$sysroot$target"
    /usr/bin/mkdir -p "$mountpoint"
    /usr/bin/findmnt -M "$mountpoint" >/dev/null 2>&1 && return 0
    load_filesystem_driver "$fstype"
    if [ "$fstype" = btrfs ] && [ -x /usr/bin/btrfs ]; then
        /usr/bin/btrfs device scan --all-devices 2>&1 || true
    fi
    if mount_filesystem -t "$fstype" -o "$cleaned_options" -- "$resolved_source" "$mountpoint"; then
        log "mounted $target from $resolved_source ($fstype)"
    elif [ "$mount_nofail" -eq 1 ]; then
        log "optional mount failed: $target from $resolved_source"
    else
        fatal "required mount failed: $target from $resolved_source"
    fi
}

fstab="$sysroot/etc/fstab"
if [ -r "$fstab" ]; then
    fstab_entries=/run/fractald-initramfs-fstab
    /usr/bin/sed -e 's/[[:space:]]*#.*$//' -e '/^[[:space:]]*$/d' "$fstab" |
        /usr/bin/sort -k2,2 >"$fstab_entries"
    while read -r source target fstype options dump pass; do
        [ -n "${source:-}" ] || continue
        mount_fstab_entry "$source" "$target" "$fstype" "${options:-defaults}"
    done <"$fstab_entries"
fi
log 'fstab mount pass complete'

# A static test image can carry the current release binaries without
# changing the cloned Fedora root. Stage them into the root before the
# handoff so the PID1 test executes the exact build under test.
if [ -x /sbin/fractald-current-pid1-test ]; then
    /usr/bin/mkdir -p "$sysroot/usr/bin"
    if [ ! -e "$sysroot/sbin" ]; then
        /usr/bin/mkdir -p "$sysroot/sbin"
    fi
    /usr/bin/cp -a /usr/bin/fractald "$sysroot/usr/bin/fractald"
    /usr/bin/cp -a /usr/bin/fractalctl "$sysroot/usr/bin/fractalctl"
    /usr/bin/cp -a /usr/bin/rustybox "$sysroot/usr/bin/rustybox"
    /usr/bin/cp -a /sbin/fractald-current-pid1-test "$sysroot/sbin/fractald-current-pid1-test"
    log 'staged current release PID1 test entrypoint'
fi

# Commit the root handoff markers before switch_root. Btrfs may delay both
# file data and directory metadata long enough for a test VM to be stopped
# immediately after a failed handoff.
if [ -x /usr/bin/sync ]; then
    /usr/bin/sync
else
    log 'sync is unavailable in the initramfs; handoff markers may be delayed'
fi

/usr/bin/mkdir -p "$sysroot/proc" "$sysroot/sys" "$sysroot/dev" "$sysroot/run"

# switch_root moves these mounts from the initramfs root into sysroot. Keep
# them mounted on the old root and leave matching directories at the target.
mount_filesystem -t tmpfs -o mode=0755,nosuid,nodev tmpfs /run 2>/dev/null || true
/usr/bin/mkdir -p /sys/fs/cgroup
mount_filesystem -t cgroup2 -o nsdelegate cgroup2 /sys/fs/cgroup 2>&1 ||
    log "cgroup v2 mount unavailable during initramfs handoff"

[ "${init_program#/}" = "$init_program" ] && fatal "fractald.init must be absolute"
log "switching root to FractalD entry point $init_program"
exec /usr/bin/switch_root "$sysroot" "$init_program"
