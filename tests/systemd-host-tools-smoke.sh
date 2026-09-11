#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-host-tools.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

machine_id="$project_dir/target/debug/systemd-machine-id-setup"
detect_virt="$project_dir/target/debug/systemd-detect-virt"
systemctl="$project_dir/target/debug/systemctl"
for binary in "$machine_id" "$detect_virt" "$systemctl"; do
    test -x "$binary"
done

id=$($machine_id --root="$root" --print --commit)
case "$id" in
    ''|*[!0123456789abcdef]*) echo "machine ID is not lowercase hexadecimal: $id" >&2; exit 1 ;;
esac
[ "${#id}" -eq 32 ] || { echo "machine ID has unexpected length: $id" >&2; exit 1; }
test "$(cat "$root/etc/machine-id")" = "$id"
test "$($machine_id --root="$root" --print)" = "$id"

if "$machine_id" --root="$root" --print >/dev/null 2>&1; then
    :
else
    echo 'machine-id setup could not reuse an existing ID' >&2
    exit 1
fi

"$detect_virt" --list >/dev/null
"$detect_virt" --help >/dev/null
"$detect_virt" --version >/dev/null
"$systemctl" daemon-reexec

echo 'systemd host-tool compatibility: PASS'
