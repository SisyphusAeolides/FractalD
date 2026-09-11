#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "/tmp/fractald-kernel-install.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

binary="$project_dir/target/debug/kernel-install"
test -x "$binary"

mkdir -p "$root/etc/kernel" "$root/etc" "$root/usr/lib/modules/6.1.0-fractald/kernel" \
    "$root/etc/kernel/install.d" "$root/usr/lib/kernel/install.d" \
    "$root/boot/loader/entries" "$root/tmp" "$root/bin"
printf '%s\n' 'FractalD test kernel' >"$root/usr/lib/modules/6.1.0-fractald/vmlinuz"
printf '%s\n' 'FractalD test initrd' >"$root/tmp/initrd.img"
cat >"$root/bin/depmod" <<'EOF'
#!/bin/sh
printf '%s\n' "$@" >>"$FRACTALD_DEPMOD_LOG"
touch "$FRACTALD_MODULE_DIR/modules.dep"
EOF
chmod 755 "$root/bin/depmod"
printf '%s\n' 'ID=fedora' 'PRETTY_NAME="Fedora FractalD"' >"$root/etc/os-release"
printf '%s\n' 'root=UUID=test-root rw' >"$root/etc/kernel/cmdline"
printf '%s\n' '#!/bin/sh' 'printf "%s %s\n" "$1" "$2" >>"$FRACTALD_PLUGIN_LOG"' \
    >"$root/etc/kernel/install.d/10-record.install"
chmod 755 "$root/etc/kernel/install.d/10-record.install"
printf '%s\n' '#!/bin/sh' 'printf "%s %s\n" "vendor-$1" "$2" >>"$FRACTALD_PLUGIN_LOG"' \
    >"$root/usr/lib/kernel/install.d/21-vendor.install"
chmod 755 "$root/usr/lib/kernel/install.d/21-vendor.install"
printf '%s\n' '#!/bin/sh' 'printf "%s\n" masked >>"$FRACTALD_PLUGIN_LOG"' \
    >"$root/usr/lib/kernel/install.d/20-masked.install"
chmod 755 "$root/usr/lib/kernel/install.d/20-masked.install"
ln -s /dev/null "$root/etc/kernel/install.d/20-masked.install"
export FRACTALD_PLUGIN_LOG="$root/plugin.log"
export FRACTALD_DEPMOD_LOG="$root/depmod.log"
export FRACTALD_MODULE_DIR="$root/usr/lib/modules/6.1.0-fractald"
export FRACTALD_KERNEL_INSTALL_DEPMOD="$root/bin/depmod"

"$binary" --root="$root" --boot-path=/boot --entry-token=literal:test-token \
    --make-entry-directory=yes add 6.1.0-fractald \
    /usr/lib/modules/6.1.0-fractald/vmlinuz /tmp/initrd.img

test -f "$root/boot/test-token/6.1.0-fractald/linux"
test -f "$root/boot/test-token/6.1.0-fractald/initrd.img"
entry="$root/boot/loader/entries/test-token-6.1.0-fractald.conf"
test -f "$entry"
grep -Fqx 'title      Fedora FractalD' "$entry"
grep -Fqx 'options    root=UUID=test-root rw' "$entry"
grep -Fqx 'linux      /test-token/6.1.0-fractald/linux' "$entry"
grep -Fqx 'initrd     /test-token/6.1.0-fractald/initrd.img' "$entry"
grep -Fqx 'add 6.1.0-fractald' "$root/plugin.log"
grep -Fqx 'vendor-add 6.1.0-fractald' "$root/plugin.log"
if grep -Fqx masked "$root/plugin.log"; then
    echo 'kernel-install failed to honor a masked vendor hook' >&2
    exit 1
fi
grep -Fqx -- '-a' "$root/depmod.log"
grep -Fqx -- '-b' "$root/depmod.log"
grep -Fqx -- "$root" "$root/depmod.log"
grep -Fqx -- '6.1.0-fractald' "$root/depmod.log"

listing=$("$binary" --root="$root" list)
echo "$listing" | grep -Fqx '6.1.0-fractald installed'
inspect=$("$binary" --root="$root" --boot-path=/boot --entry-token=literal:test-token \
    inspect 6.1.0-fractald /usr/lib/modules/6.1.0-fractald/vmlinuz /tmp/initrd.img)
echo "$inspect" | grep -Fqx 'KERNEL_INSTALL_LAYOUT=bls'
echo "$inspect" | grep -Fqx 'KERNEL_INSTALL_ENTRY_TOKEN=test-token'

json=$("$binary" --root="$root" --boot-path=/boot --entry-token=literal:test-token \
    --json=short inspect)
echo "$json" | grep -Fq '"KERNEL_INSTALL_LAYOUT":"bls"'

ln -s "$binary" "$root/installkernel"
FRACTALD_KERNEL_INSTALL_GENERATE_INITRD=0 "$root/installkernel" \
    --root="$root" --boot-path=/boot --entry-token=literal:test-token \
    6.1.0-fractald /usr/lib/modules/6.1.0-fractald/vmlinuz

"$binary" --root="$root" --boot-path=/boot --entry-token=literal:test-token remove 6.1.0-fractald
test ! -e "$entry"
test ! -e "$root/boot/test-token/6.1.0-fractald"
test ! -e "$root/usr/lib/modules/6.1.0-fractald/modules.dep"
grep -Fqx 'remove 6.1.0-fractald' "$root/plugin.log"
grep -Fqx 'vendor-remove 6.1.0-fractald' "$root/plugin.log"

if "$binary" --root="$root" --entry-token=literal:../escape add 6.1.0-fractald \
    /usr/lib/modules/6.1.0-fractald/vmlinuz >/dev/null 2>&1; then
    echo 'kernel-install accepted a path traversal entry token' >&2
    exit 1
fi

"$binary" --version >/dev/null
"$binary" --help >/dev/null
echo 'kernel-install compatibility: PASS'
