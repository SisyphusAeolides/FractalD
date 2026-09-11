#!/bin/sh
set -eu

binary_directory=${FRACTALD_TEST_BIN_DIR:-/usr/local/libexec/fractald-pid1-test}
runtime=${FRACTALD_RUNTIME_DIR:-/run/fractald-pid1}
state=${FRACTALD_STATE_DIR:-/run/fractald-pid1-state}
ctl=${FRACTALCTL_BIN:-$binary_directory/fractalctl}
initramfs_log=${FRACTALD_INITRAMFS_LOG:-/root/fractald-pid1-test/initramfs.log}

fail() {
    echo "storage matrix: FAIL: $*" >&2
    exit 1
}

check_mount() {
    mountpoint=$1
    expected_type=$2
    actual_type=$(/usr/bin/findmnt --target "$mountpoint" --noheadings --output FSTYPE 2>/dev/null || true)
    [ "$actual_type" = "$expected_type" ] ||
        fail "$mountpoint is $actual_type, expected $expected_type"
    [ -f "$mountpoint/normal-marker" ] || fail "$mountpoint is missing its persistence marker"
    echo "$mountpoint: $actual_type"
}

[ "$(/usr/bin/cat /proc/1/comm)" = fractald ] || fail 'PID1 is not fractald'

export FRACTALD_RUNTIME_DIR="$runtime"
export FRACTALD_STATE_DIR="$state"

storage_target=$($ctl status fractald-storage.target 2>/dev/null || true)
echo "$storage_target"
echo "$storage_target" | /usr/bin/grep -Eq ': active([[:space:]]|$)' ||
    fail 'fractald-storage.target is not active'
prepare=$($ctl status fractald-storage-prepare.service 2>/dev/null || true)
echo "$prepare"
echo "$prepare" | /usr/bin/grep -Eq ': active([[:space:]]|$)' ||
    fail 'fractald-storage-prepare.service is not active'

check_mount /srv/fractald-btrfs-pool btrfs
check_mount /srv/fractald-ext4 ext4
check_mount /srv/fractald-xfs xfs
check_mount /srv/fractald-vfat vfat
check_mount /srv/fractald-exfat exfat
check_mount /srv/fractald-f2fs f2fs
check_mount /srv/fractald-lvm ext4
check_mount /srv/fractald-md xfs
check_mount /srv/fractald-luks ext4

if [ -r "$initramfs_log" ]; then
    for mountpoint in \
        /srv/fractald-btrfs-pool \
        /srv/fractald-ext4 \
        /srv/fractald-xfs \
        /srv/fractald-vfat \
        /srv/fractald-exfat \
        /srv/fractald-f2fs \
        /srv/fractald-lvm \
        /srv/fractald-md \
        /srv/fractald-luks; do
        /usr/bin/grep -Fq "mounted $mountpoint" "$initramfs_log" ||
            fail "initramfs did not record $mountpoint"
    done
fi

btrfs_devices=$(/usr/bin/btrfs filesystem show /srv/fractald-btrfs-pool |
    /usr/bin/grep -c '^[[:space:]]*devid ')
[ "$btrfs_devices" -ge 2 ] || fail "Btrfs pool reports $btrfs_devices devices"

md_source=$(/usr/bin/findmnt --target /srv/fractald-md --noheadings --output SOURCE)
md_detail=$(/usr/sbin/mdadm --detail "$md_source") || fail "cannot inspect mdraid source $md_source"
echo "$md_detail"
echo "$md_detail" | /usr/bin/grep -Eq 'Active Devices[[:space:]]*:[[:space:]]*2' ||
    fail 'mdraid does not report two active devices'

lvm_attrs=$(/usr/sbin/lvs --noheadings --options lv_attr fractalvg/data | /usr/bin/tr -d '[:space:]')
echo "LVM attributes: $lvm_attrs"
echo "$lvm_attrs" | /usr/bin/grep -q 'a' || fail 'LVM data volume is not active'

crypt_status=$(/usr/sbin/cryptsetup status fractal-crypt) || fail 'LUKS mapping is not active'
echo "$crypt_status"
echo "$crypt_status" | /usr/bin/grep -q 'is active' || fail 'LUKS mapping status is not active'

units=$($ctl list)
for unit in \
    fractald-storage.target \
    fractald-storage-prepare.service \
    'srv-fractald\x2dbtrfs\x2dpool.mount' \
    'srv-fractald\x2dext4.mount' \
    'srv-fractald\x2dxfs.mount' \
    'srv-fractald\x2dvfat.mount' \
    'srv-fractald\x2dexfat.mount' \
    'srv-fractald\x2df2fs.mount' \
    'srv-fractald\x2dlvm.mount' \
    'srv-fractald\x2dmd.mount' \
    'srv-fractald\x2dluks.mount'; do
    echo "$units" | /usr/bin/grep -Fxq "$unit" || fail "generated unit $unit is missing"
done

echo 'storage matrix: PASS'
