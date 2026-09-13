#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
for binary in \
    fractald-analyze fractald-detect-virt fractald-escape fractald-machine-id \
    fractald-notify fractald-sysctl fractald-sysusers fractald-tmpfiles \
    fractald-udevadm fractald-resolvectl fractald-journalctl; do
    "$project_dir/target/debug/$binary" --help >/dev/null
done

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/etc/fractald/services"
cat >"$root/etc/fractald/services/demo.svc" <<'SERVICE'
[service]
kind=group
SERVICE
FRACTALD_ANALYZE_ROOT="$root" \
    "$project_dir/target/debug/fractald-analyze" verify --root="$root" demo

echo 'native helper tools: PASS'
