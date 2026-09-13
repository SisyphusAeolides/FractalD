# RustyBox

RustyBox is FractalD's optional multi-call initramfs userland. It provides a
small set of native applets for storage preparation, filesystem handoff,
diagnostics, and deterministic chaos samples.

```sh
rustybox --list
rustybox chaos list
rustybox chaos sample lorenz
rustybox chaos check
```

FractalD honors an explicit `FRACTALD_RUSTYBOX` or
`FRACTALD_TOOLBOX_DIR`. When neither is set it automatically searches the
native toolbox directories `/usr/lib/fractald/toolbox`,
`/usr/libexec/fractald/toolbox`, and their `/usr/local` counterparts. The
toolbox applets cover `mount`, `umount`, `swapon`, `swapoff`, `mkdir`, `mv`,
`rm`, and related image operations. The normal service command remains
unchanged.

Build the image helpers with:

```sh
cargo build -p rustybox --release
make rustybox-profile
```

`make pid1-static-initramfs` creates an independently owned cpio/zstd image.
Set `FRACTALD_BASE_INITRAMFS` to an existing initramfs when distribution files
or package services are needed; the builder adds FractalD, RustyBox, a POSIX
shell, `blkid`, `btrfs` when available, the native package trigger, the native
toolbox links, and the libraries required by those binaries. The generated
`/init` is the FractalD executable itself by default, and PID1 dispatch is
independent of boot-loader arguments. Tests can provide a controlled init
handoff with `FRACTALD_INITRAMFS_INIT`; it is copied as `/init` and can invoke
`switch_root NEW_ROOT /usr/bin/fractald`. `FRACTALD_INITRAMFS_FSTAB` and
`FRACTALD_INITRAMFS_SERVICE_DIR` provide controlled test fixtures.

`make pid1-qemu-btrfs-check` is the storage and boot integration test. It uses
two temporary virtual disks and a native `.svc` check to verify the mounted
pool and both Btrfs members.

`make pid1-qemu-root-btrfs-check` verifies the complete handoff path: a
temporary installed root is mounted from Btrfs, a second device is added to
that filesystem, RustyBox switches root, and FractalD starts the native boot
services as PID1.
