#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d)
cleanup() {
    "$project_dir/target/debug/fractalctl" stop >/dev/null 2>&1 || true
    rm -r "$root"
}
trap cleanup EXIT HUP INT TERM

mkdir -p "$root/run" "$root/state" "$root/services" "$root/units"
export FRACTALD_RUNTIME_DIR="$root/run"
export FRACTALD_STATE_DIR="$root/state"
export FRACTALD_SERVICE_DIR="$root/services"
export FRACTALD_UNIT_DIR="$root/units"
export FRACTALD_BIN="$project_dir/target/debug/fractald"

"$project_dir/target/debug/fractalctl" enable --now >/dev/null
test -L "$root/units/default.target.wants/fractald.service"
"$project_dir/target/debug/fractalctl" status >/dev/null

"$project_dir/target/debug/fractalctl" disable --now >/dev/null
test ! -e "$root/units/default.target.wants/fractald.service"
test ! -e "$root/run/fractald.pid"

echo "fractalctl --now: ok"
