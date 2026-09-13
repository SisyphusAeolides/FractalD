# Native configuration

FractalD reads only `.svc` descriptors. A descriptor must contain a `[service]`
section and every section and key is checked against the native schema. Unknown
keys fail validation before a service enters the supervisor.

## Search order

For a root manager, FractalD searches these directories from lowest to highest
priority:

- `/usr/lib/fractald/services`
- `/usr/libexec/fractald/services`
- `/usr/local/lib/fractald/services`
- `/usr/local/libexec/fractald/services`
- `/run/fractald/services`
- `/etc/fractald/services`

`FRACTALD_SERVICE_DIR` replaces the list with one controlled directory. A user
manager uses `$XDG_CONFIG_HOME/fractald/services` and its runtime directory.
A file named `example.svc` has the service name `example`.

## Descriptor example

```ini
[service]
description=HTTP worker
kind=notify
exec=/usr/bin/http-worker --foreground
restart=on-failure
restart_delay=250ms
start_timeout=10s
stop_timeout=10s
watchdog=30s
notify_access=main
stdout=journal

[dependencies]
requires=network
wants=log-ready
after=network
on_failure=http-recovery

[install]
profile=boot
```

Lists are whitespace separated and repeated directives append values. Command
lines support shell style quoting as parsed arguments; FractalD does not invoke
a shell unless the descriptor explicitly selects a shell executable.

The `kind` values are `simple`, `forking`, `oneshot`, `notify`, `dbus`, `idle`,
`group`, `listener`, `timer`, `watch`, `mount`, `swap`, and `device`. Listener,
timer, watch, mount, swap, and device details live in their matching sections.

## Package discovery

FractalD discovers every regular `.svc` file in the native service directories
at startup and reload. Package identity is not required: a package can install
its descriptor into a native directory and use `[install] profile=boot` to join
the boot profile. Later directories in the search order overlay earlier ones.

`fractald-package-trigger` is optional state maintenance. When a pacman file
database is available it validates the descriptors listed by that database and
records their owners in `packages.index`. Without that database it scans the
native service directories and records the owner as `filesystem`. In both modes
it enables descriptors for the selected profile and requests a native reload.
Duplicate package descriptors are rejected before state changes; native
directory overlays retain the documented search-order precedence.

Use these operations during image construction or package testing:

```sh
fractald-package-trigger verify
fractald-package-trigger sync
fractald-package-trigger reload
```

`FRACTALD_PACKAGE_DB` selects an alternate pacman database and
`FRACTALD_PACKAGE_ROOT` selects the filesystem root for fixture tests.
`FRACTALD_PACKAGE_DISCOVERY=native` forces package-neutral filesystem scanning.

## Profiles and PID1

Profiles are ordinary group services. `boot.svc` is the packaged default:

```ini
[service]
kind=group

[dependencies]
wants=storage

[install]
profile=boot
allow_isolate=true
```

`fractalctl enable` stores `profile=boot` in `/etc/fractald/boot.conf` and
creates a persistent `boot` marker. A different profile can be selected by
writing the profile name to that file or setting `FRACTALD_BOOT_PROFILE`.

Profile enablement is not bootloader installation. Before selecting FractalD as
the machine's init, run `fractalctl doctor`. It fails when the native binaries,
service tree, toolbox, kernel/initramfs `init=` selection, PID1 identity, or
cgroup v2 prerequisites are missing. The command is read-only; changing the
boot entry or `/sbin/init` remains an explicit administrator operation.

Mount and swap services become dependency-ready only after their helper exits;
services ordered after a mount therefore cannot race the mount operation.
