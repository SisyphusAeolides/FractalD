#!/bin/sh
set -eu

binary=${FRACTALD_UNIT_INVENTORY_BINARY:-target/release/fractald}
directories=${FRACTALD_UNIT_INVENTORY_DIRS:-/usr/lib/systemd/system:/usr/local/lib/systemd/system:/etc/systemd/system:/run/systemd/system}
unit_list=$(mktemp)
failure_log=$(mktemp)

cleanup() {
    /usr/bin/rm -f "$unit_list" "$failure_log"
}
trap cleanup EXIT HUP INT TERM

[ -x "$binary" ] || {
    echo "unit inventory: inspector is not executable: $binary" >&2
    exit 1
}

old_ifs=$IFS
IFS=:
for directory in $directories; do
    [ -d "$directory" ] || continue
    rg --files "$directory" 2>/dev/null || true
done | sort -u >"$unit_list"
IFS=$old_ifs

unit_count=0
failure_count=0
while IFS= read -r path; do
    case "$path" in
        *.service|*.socket|*.target|*.timer|*.path|*.mount|*.swap|*.automount|*.slice|*.scope|*.device|*.busname)
            unit_count=$((unit_count + 1))
            if ! output=$("$binary" inspect-unit "$path" 2>&1); then
                failure_count=$((failure_count + 1))
                {
                    printf '%s\n' "$path"
                    printf '%s\n' "$output"
                } >>"$failure_log"
            fi
            ;;
    esac
done <"$unit_list"

printf 'unit inventory: %s files\n' "$unit_count"
printf 'parse failures: %s\n' "$failure_count"
if [ "$failure_count" -ne 0 ]; then
    /usr/bin/sed -n '1,240p' "$failure_log" >&2
    exit 1
fi
[ "$unit_count" -gt 0 ] || {
    echo 'unit inventory: no unit files found' >&2
    exit 1
}
