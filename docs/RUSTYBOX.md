# RustyBox

RustyBox is FractalD's optional multi-call initramfs userland for Arch based
images. It provides a small set of native applets for storage preparation,
filesystem handoff, diagnostics, and deterministic chaos samples.

```sh
rustybox --list
rustybox chaos list
rustybox chaos sample lorenz
rustybox chaos check
```

FractalD uses RustyBox only when configured with
`FRACTALD_RUSTYBOX=/absolute/path/to/rustybox` or
`FRACTALD_TOOLBOX_DIR=/usr/libexec/fractald/toolbox`. The toolbox applets cover
`mount`, `umount`, `swapon`, `swapoff`, `mkdir`, `mv`, `rm`, and related image
operations. The normal service command remains unchanged.

Build the image helpers with:

```sh
cargo build -p rustybox --release
make rustybox-profile
```

`make pid1-static-initramfs` creates an independently owned cpio/zstd image.
Set `FRACTALD_BASE_INITRAMFS` to an existing Arch initramfs when its files are
needed; the builder adds FractalD, RustyBox, the native package trigger, the
boot descriptor, and the libraries required by those binaries. The generated
`/init` is the FractalD executable itself, so it prepares the kernel interfaces
and runs the native boot graph as PID1.
