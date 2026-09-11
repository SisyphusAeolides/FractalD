# RustyBox

RustyBox is FractalD's independent multi-call userland. It is built as a
workspace binary and is suitable for a compact rescue or initramfs profile.
The executable dispatches an applet from its first argument or from the name
of the applet link that started it.

The implementation uses Rust for dispatch, argument handling, filesystem
policy, process launching, output, and the deterministic dynamics interface.
A small C layer owns the low-level operations that need direct POSIX or Linux
ABI calls: interrupt-safe file copying and writes, sleeping, directory and
link operations, signals, mount and unmount, swap activation, chroot,
`pivot_root` based root handoff, synchronization, and uname. The C boundary
returns negative errno values, which the Rust side turns into normal I/O
errors.

The initramfs profile also uses native bounded applets for `dmesg`, `findmnt`,
`sort`, `tr`, `sync`, and `switch_root`. `findmnt` reads the kernel mount
table, `sort` implements the field ordering used for `fstab`, and `tr`
implements the character classes and escapes used for early storage metadata.
`insmod` uses `finit_module(2)` for uncompressed modules, and `modprobe`
understands an uncompressed `modules.dep` tree when no distribution helper is
available.
The distribution `mount`, `blkid`, `cryptsetup`, RAID, LVM, module, and
filesystem helper tools remain available for operations whose on-disk or
kernel formats are owned by those projects.

The chaos applet is part of the binary rather than a separate diagnostic
program:

    rustybox chaos list
    rustybox chaos sample lorenz
    rustybox chaos sample mandelbrot
    rustybox chaos sample lyapunov
    rustybox chaos sample rossler
    rustybox chaos sample logistic-map
    rustybox chaos sample duffing
    rustybox chaos check

The samples use the shared FractalD dynamics crate and remain deterministic,
so they can be used in initramfs diagnostics and reproducible smoke tests.
They do not influence process scheduling, restart delays, or failure
handling. Operational behavior stays bounded and predictable.

FractalD selects the toolbox only when configured:

    FRACTALD_TOOLBOX_DIR=/usr/libexec/fractald/toolbox

When that directory is present, FractalD uses its mkdir, mount, umount,
swapon, swapoff, mv, rm, and rmdir applets for native mount and swap units
and places the directory first in the lifecycle helper PATH. An alternative
single binary can be selected with
FRACTALD_RUSTYBOX=/absolute/path/to/rustybox; FractalD sets the applet argv[0]
for that form. The mount applet resolves UUID, LABEL, PARTUUID, and PARTLABEL
sources through `/dev/disk` or the installed `blkid` helper before making the
mount syscall, and ignores fstab-only metadata such as `nofail`, `_netdev`,
and `x-systemd.*`. It normalizes `fat`, `msdos`, and `ext` to the kernel mount
types `vfat` and `ext4`. Service ExecStart commands remain untouched, so normal
Fedora and Debian packages keep their ordinary userland expectations.

Build and run the isolated check with:

    cargo build -p rustybox --release
    make rustybox-check

make install installs the binary and its applet links below
libexec/fractald/toolbox. It does not replace /sbin/init, /bin, or the host
service manager. The `pid1-static-initramfs` make target can assemble a
minimal, independently-owned initramfs from a Fedora base image. Its fixed
allowlist includes the FractalD and RustyBox binaries, the required storage
helpers, their shared libraries, and the selected kernel modules; it rejects
systemd manager paths before emitting the image. The profile is fixed-layout
and independently bootable; its ELF binaries remain dynamically linked so the
image can use the distribution's ordinary libraries. The init applet can explicitly exec FractalD with
RUSTYBOX_INIT=/usr/bin/fractald, or it can exec a command supplied after
rustybox init. The tested handoff then runs `/usr/bin/fractald` as the actual
PID 1 entrypoint after the root filesystem has been assembled.

The current profile is deliberately small. It is a real, buildable base
layer, while full BusyBox command and shell compatibility remains a separate
expansion track. Unsupported options fail clearly instead of silently
delegating to a different implementation.

During an independent static initramfs boot, the FractalD early mount path
uses RustyBox's mount applet for kernel mounted filesystems and keeps the
declared type and mount options intact. The selection consults the running
kernel's `/proc/filesystems` and falls back to a conservative built-in list;
helper and network filesystems remain on the distribution mount path. If the
direct attempt fails, or the filesystem is helper-backed, the distribution
mount program is retried when it is present; this covers forms such as SSHFS,
DAVFS, and GlusterFS. After root handoff, native mount units use the same
selection rule, so adding a kernel module or a mount helper extends filesystem
support without changing FractalD's unit graph.

The static image carries the matching kernel module tree from the Fedora base
image. It also carries the distribution `mount` and `umount` dispatchers as a
fallback for filesystems that need userspace helpers; RustyBox remains the
first attempt for kernel mounted types. When the image is built for another kernel or controller set, provide
the complete `/lib/modules/<release>` tree with
`FRACTALD_INITRAMFS_MODULE_DIR` and identify it with
`FRACTALD_INITRAMFS_KERNEL_RELEASE`; this keeps filesystem, storage
controller, device mapper, and RAID support aligned with the kernel that will
boot the image.
