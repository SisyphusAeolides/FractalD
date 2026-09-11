#!/bin/sh
set -eu

binary_directory=${FRACTALD_TEST_BIN_DIR:-/usr/local/libexec/fractald-pid1-test}
runtime=${FRACTALD_RUNTIME_DIR:-/run/fractald-pid1}
storage_check=${FRACTALD_STORAGE_MATRIX_CHECK:-$binary_directory/storage-matrix}
ctl=${FRACTALCTL_BIN:-$binary_directory/fractalctl}

fail() {
    echo "storage recovery: FAIL: $*" >&2
    exit 1
}

wait_for_state() {
    unit=$1
    expected=$2
    attempts=${3:-300}
    while [ "$attempts" -gt 0 ]; do
        status=$($ctl status "$unit" 2>/dev/null || true)
        if echo "$status" | /usr/bin/grep -Eq ": $expected([[:space:]]|$)"; then
            return 0
        fi
        /usr/bin/sleep 0.1
        attempts=$((attempts - 1))
    done
    echo "$status" >&2
    fail "$unit did not reach state=$expected"
}

[ "$(/usr/bin/cat /proc/1/comm)" = fractald ] || fail 'PID1 is not fractald'
[ -x "$storage_check" ] || fail "storage matrix check is unavailable: $storage_check"

md_source=$(/usr/bin/findmnt --target /srv/fractald-md --noheadings --output SOURCE) ||
    fail 'mdraid mount is unavailable before recovery'

for mountpoint in \
    /srv/fractald-luks \
    /srv/fractald-lvm \
    /srv/fractald-md \
    /srv/fractald-f2fs \
    /srv/fractald-exfat \
    /srv/fractald-vfat \
    /srv/fractald-xfs \
    /srv/fractald-ext4 \
    /srv/fractald-btrfs-pool; do
    /usr/bin/umount -- "$mountpoint" || fail "cannot unmount $mountpoint"
done

/usr/sbin/cryptsetup close fractal-crypt || fail 'cannot close LUKS mapping'
/usr/sbin/vgchange --activate n fractalvg || fail 'cannot deactivate LVM volume group'
/usr/sbin/mdadm --stop "$md_source" || fail "cannot stop mdraid $md_source"

export FRACTALD_RUNTIME_DIR="$runtime"
"$ctl" restart fractald-storage-prepare.service
wait_for_state fractald-storage-prepare.service active

for unit in \
    'srv-fractald\x2dbtrfs\x2dpool.mount' \
    'srv-fractald\x2dext4.mount' \
    'srv-fractald\x2dxfs.mount' \
    'srv-fractald\x2dvfat.mount' \
    'srv-fractald\x2dexfat.mount' \
    'srv-fractald\x2df2fs.mount' \
    'srv-fractald\x2dlvm.mount' \
    'srv-fractald\x2dmd.mount' \
    'srv-fractald\x2dluks.mount'; do
    "$ctl" start "$unit"
    wait_for_state "$unit" active
done

"$storage_check"
echo 'storage recovery: PASS'
