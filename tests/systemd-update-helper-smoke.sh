#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-update-helper.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

binary="$project_dir/target/debug/systemd-update-helper"
test -x "$binary"

export FRACTALD_RUNTIME_DIR="$root/run"
export FRACTALD_STATE_DIR="$root/state"
export FRACTALD_PRESET_DIR="$root/presets"
export FRACTALD_SYSTEMCTL_BIN="$project_dir/target/debug/systemctl"
mkdir -p "$root/presets"

"$binary" mark-restart-system-units demo.service
"$binary" mark-reload-user-units demo.service
test -f "$root/run/markers/demo.service.needs-restart"
test -f "$root/run/markers/demo.service.needs-reload"

"$binary" install-system-units demo.service
test -f "$root/state/enabled/demo.service"

"$binary" remove-user-units demo.service
test ! -e "$root/state/enabled/demo.service"

cat >"$root/presets/00-disable-demo.preset" <<'PRESET'
disable demo.service
PRESET
"$binary" install-system-units demo.service
test ! -e "$root/state/enabled/demo.service"

export FRACTALD_SYSTEMCTL_BIN="$root/missing-systemctl"
rm "$root/presets/00-disable-demo.preset"
"$binary" install-system-units demo.service
test -f "$root/state/enabled/demo.service"

"$binary" user-reexec
"$project_dir/target/debug/systemctl" daemon-reexec

# Manager refresh verbs must remain harmless while FractalD is offline, as
# package post-transactions can run before PID1 has created its control socket.
for operation in system-reload-restart system-reload system-restart \
    user-reload-restart user-reload user-restart user-reexec; do
    "$binary" "$operation"
done

cat >"$root/fake-systemctl" <<'SCRIPT'
#!/bin/sh
printf '%s\n' "$*" >>"$FRACTALD_FAKE_LOG"
SCRIPT
chmod 755 "$root/fake-systemctl"
export FRACTALD_SYSTEMCTL_BIN="$root/fake-systemctl"
export FRACTALD_FAKE_LOG="$root/systemctl.log"
mkdir -p "$root/run"
touch "$root/run/control.sock"

for operation in system-reload-restart system-reload system-restart \
    user-reload-restart user-reload user-restart user-reexec; do
    "$binary" "$operation"
done
grep -Fqx 'daemon-reload' "$root/systemctl.log"
grep -Fqx 'reload-or-restart --marked' "$root/systemctl.log"
grep -Fqx -- '--user reload user@*.service' "$root/systemctl.log"
grep -Fqx -- '--user reload-or-restart --marked' "$root/systemctl.log"

for operation in system-reload-restart system-reload system-restart \
    user-reload-restart user-reload user-restart user-reexec; do
    if "$binary" "$operation" unexpected.service; then
        echo "$operation unexpectedly accepted unit arguments" >&2
        exit 1
    fi
done

if "$binary" mark-restart-system-units ../demo.service; then
    echo 'path-like unit name unexpectedly accepted' >&2
    exit 1
fi

echo 'systemd update-helper compatibility: PASS'
