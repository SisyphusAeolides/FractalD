#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT

printf 'hello from FractalD\n' | \
    FRACTALD_LOG_DIR="$root/logs" \
    "$project_dir/target/debug/fractald-cat" -t demo
FRACTALD_LOG_DIR="$root/logs" \
    "$project_dir/target/debug/fractald-journalctl" -u demo.svc | grep -F 'hello from FractalD' >/dev/null

echo 'native journal tools: PASS'
