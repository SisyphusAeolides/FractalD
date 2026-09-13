# FractalD

FractalD is a standalone Linux service manager and native PID1. It owns the
boot process, service graph, process supervision, storage activation, package
integration, and local control socket. It does not read, execute, or generate
configuration for another init system.

Fedora and RHEL-compatible distributions are the primary development targets.
The native service format uses `.svc` files stored below
`/usr/lib/fractald/services`, `/usr/libexec/fractald/services`,
`/usr/local/lib/fractald/services`, `/usr/local/libexec/fractald/services`,
`/run/fractald/services`, or `/etc/fractald/services`.

## Native service descriptors

A minimal service is explicit and easy to inspect:

```ini
[service]
description=Example worker
kind=simple
exec=/usr/bin/example-worker
restart=on-failure

[dependencies]
after=network-online

[install]
profile=boot
```

`[dependencies]` supports required, wanted, ordering, conflict, binding, and
recovery relationships. Native kinds cover processes, listeners, timers,
filesystem watches, mounts, swaps, devices, and grouping profiles. Hardening,
credentials, resource limits, conditions, environment setup, and lifecycle
hooks use the same descriptor.

Every regular `.svc` file in the native service directories is discovered and
validated by FractalD at startup and reload, regardless of which package
installed it. A descriptor with `profile=boot` is started by the native boot
profile. `fractald-package-trigger sync` is optional state maintenance: it uses
package-file metadata when available and otherwise scans the native directories
directly. The Arch package hook invokes it after transactions; the RPM package
does not depend on an Arch package database.

## Build on Fedora / RHEL / CentOS Stream

```sh
sudo dnf install @development-tools rust cargo clang
make check
make test
make native-check
```

Build an RPM package from the included spec:

```sh
dnf install rpm-build
rpmbuild -ba fractald.spec
```

Or install from COPR:

```sh
sudo dnf copr enable sisyphuscode/fractald
sudo dnf install fractald
```

Install the native tools into a staging root with `make install`. The package
places the PID1 binary at `/usr/bin/fractald`, a copy at
`/usr/lib/fractald/init`, native descriptors below `/usr/lib/fractald/services`,
and RustyBox helper links below the native FractalD toolbox directory. Arch
installs its optional package hook at `/usr/share/libalpm/hooks`.

## Control

`fractalctl` talks directly to FractalD through its Unix socket:

```sh
fractalctl enable
fractalctl start example
fractalctl status example
fractalctl stop example
fractalctl disable example
fractalctl mask example
fractalctl unmask example
fractalctl list
fractalctl events --follow
fractalctl doctor
```

`fractalctl enable` writes `/etc/fractald/boot.conf` and persists the `boot`
profile marker. `fractalctl start SERVICE` starts FractalD first when no daemon
is running, then submits the service transaction. Enabling the profile does not
rewrite a bootloader or replace `/sbin/init`; select `/usr/bin/fractald` with
`init=/usr/bin/fractald` in the boot entry, or point `/sbin/init` at the native
binary after an initramfs/root handoff has been installed. `fractalctl doctor`
checks the installed binaries, native descriptors, toolbox, boot selection,
PID1 identity, and cgroup/mount prerequisites before a reboot. When FractalD is
PID1 it prepares `/proc`, `/sys`, `/run`, `/dev`, and cgroup v2 before loading
every native descriptor. Package markers are optional and do not gate discovery.

Use `FRACTALD_SERVICE_DIR`, `FRACTALD_STATE_DIR`, and
`FRACTALD_RUNTIME_DIR` to run isolated development instances. Use
`FRACTALD_PACKAGE_DB` and `FRACTALD_PACKAGE_ROOT` test package-file discovery
against a fixture database. Set `FRACTALD_PACKAGE_DISCOVERY=native` to force
package-neutral filesystem discovery in a test or package script.

The production-oriented PID1 gate is:

```sh
make production-check
```

The gate includes a disposable two-device Btrfs initramfs boot and a separate
installed-root boot. The latter mounts a Btrfs root, adds a second virtual disk
to the same filesystem, switches root, and verifies that native journald,
udevd, storage, and `fractalctl` service state all work while FractalD is PID1.

## Project layout

- `fractald-core` defines service records, lifecycle states, and dependency
  planning.
- `fractald-config` parses the native `.svc` format and validates directives.
- `fractald-supervisor` owns process, listener, timer, mount, credential, and
  resource supervision.
- `fractald-storage` turns `fstab` and `crypttab` into native storage services.
- `fractald-control` defines the local control protocol and persistent state.
- `fractald-platform` contains the small C boundary for Linux kernel calls.
- `fractald-package-trigger` connects package contents to service state.
- `rustybox` supplies the optional native initramfs applets and chaos tools.

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for the boot and supervision
model and [docs/CONFIGURATION.md](docs/CONFIGURATION.md) for the descriptor
reference.
