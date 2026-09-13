# Native configuration

FractalD reads only `.svc` descriptors. A descriptor must contain a `[service]`
section and every section and key is checked against the native schema. Unknown
keys fail validation before a service enters the supervisor.

## Search order

For a root manager, FractalD searches these directories from lowest to highest
priority:

- `/usr/lib/fractald/services`
- `/usr/local/lib/fractald/services`
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

## Package detection

Arch packages place descriptors in one of the native service directories. The
pacman database records each installed file in a package `files` entry.
`fractald-package-trigger` scans those entries, validates the descriptor, and
records package ownership in `packages.index`. A descriptor is enabled when it
declares the profile selected by `FRACTALD_BOOT_PROFILE` or the native boot
configuration file; the default selected profile is `boot`. Duplicate service
names across package descriptors are rejected before state is changed.

Use these operations during image construction or package testing:

```sh
fractald-package-trigger verify
fractald-package-trigger sync
fractald-package-trigger reload
```

`FRACTALD_PACKAGE_DB` selects an alternate pacman database and
`FRACTALD_PACKAGE_ROOT` selects the filesystem root for fixture tests.

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
