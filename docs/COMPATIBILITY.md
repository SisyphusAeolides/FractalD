# Compatibility contract

Compatibility is added in layers so that each behavior can be tested independently of the daemon.

## Systemd units

| Surface | Initial contract | Later contract |
| --- | --- | --- |
| Unit discovery | `FRACTALD_SERVICE_DIR`, standard system/user systemd paths, bounded executable generators, `.d/*.conf` drop-ins, target relationship directories, standard `*.wants` and `*.requires` enablement links, dynamic template instances, synthetic `.device` units, filesystem aliases, `[Install] Alias=` declarations, persistent or `/dev/null` masks, and systemd preset policy files | additional install-time alias semantics |
| Service execution | `ExecStart`, `ExecStartPre`, `ExecStartPost`, `ExecStop`, `ExecStopPost`, `ExecReload`, `ExecCondition`, `Environment`, `EnvironmentFile`, `LoadCredential` file and credential-store forms, `ImportCredential`, `SetCredential`, `CREDENTIALS_DIRECTORY`, `%d`, `UnsetEnvironment`, path and host conditions, `ConfigurationDirectory`, `RuntimeDirectory`, `StateDirectory`, `CacheDirectory`, `LogsDirectory`, `WorkingDirectory`, `User`, `Group`, `DynamicUser`, `SupplementaryGroups`, `PIDFile`, `Slice`, `StartLimitIntervalSec`, `StartLimitBurst`, `RuntimeMaxSec`, `OOMPolicy`, `FailureAction`, `SuccessAction`, `Type=dbus` with `BusName=` readiness probing, common `Type` and `Restart` values, `RemainAfterExit`, `StandardOutput`, `StandardError`, native journal datagram forwarding with `FRACTALD_JOURNAL_SOCKET`, `LimitNOFILE`, `LimitMEMLOCK`, `LimitNPROC`, `CapabilityBoundingSet`, `AmbientCapabilities`, `RestrictAddressFamilies`, `SystemCallFilter`, `SystemCallArchitectures`, `SystemCallErrorNumber`, `RestrictSUIDSGID`, `RestrictNamespaces`, `ProtectKernelTunables`, `ProtectClock`, `ProtectProc`, `ProcSubset`, `KillMode`, `PrivateTmp`, `PrivateDevices`, `DevicePolicy`, `DeviceAllow`, `PrivateMounts`, `PrivateIPC`, `PrivateNetwork`, `ProtectSystem`, `ProtectHome`, `ProtectControlGroups`, `ProtectKernelModules`, `ProtectKernelLogs`, `ProtectHostname`, `LockPersonality`, `ReadWritePaths`, `ReadOnlyPaths`, `InaccessiblePaths`, `KillSignal`, `RestartKillSignal`, `SendSIGHUP`, `IgnoreSIGPIPE`, common process hardening settings, watchdog notifications, local output logs with one-generation rotation, and cgroup limits including `CPUQuota` and `CPUQuotaPeriodSec` | broader namespace and sandbox settings, native journal querying and retention |
| Dependencies | `Requires`, `Wants`, `After`, `Before`, `Conflicts`, `PartOf`, `BindsTo` with requirement and loss propagation, `Requisite` active prerequisite checks, synthetic device wait units, `OnSuccess`, `OnFailure`, `RequiresMountsFor`, `WantsMountsFor`, `DefaultDependencies`, `StopWhenUnneeded`, `RefuseManualStart`, `RefuseManualStop`, `AllowIsolate`, `IgnoreOnIsolate`, target transactions, readiness ordering, and required failure rollback | isolation domains and full job reporting |
| Control | `start`, `stop`, `restart`, `isolate`, `status`, `is-active`, `is-failed`, `enable`, `disable`, `preset`, `preset-all`, `mask`, `unmask`, `set-property Markers=`, `reload-or-restart --marked`, `reset-failed`, `daemon-reload`, `events [SINCE]`, `events [SINCE] --follow`, `--now`, `is-system-running`, `poweroff`, `reboot`, `halt`, wildcard listing, state-aware `list-unit-files`, accepted transaction identifiers, transaction status queries, a lifecycle-property subset of `systemctl show`, a blocking `systemctl` compatibility path, `systemd-notify` readiness, status, watchdog, reload, stop, main-PID, and custom field messages, and `systemd-tmpfiles` creation, cleaning, removal, purge, alternate-root, prefix, and dry-run operations | full property output |
| Activation | long running services, targets, stream/datagram/sequential-packet socket units with `RemoveOnStop` and `FileDescriptorName`, abstract Unix sockets, FIFO, netlink, and special listeners, per-connection stream sockets, `LISTEN_FDS` and `LISTEN_FDNAMES` descriptor passing, monotonic/common calendar timer units with persistent catch-up, randomized delay, and `AccuracySec=` coalescing, polled path units including glob existence watches, mount units, swap units, slice lifecycle nodes, and eager automount compatibility transactions | native lazy autofs automount behavior and full multi-instance socket activation |

Parser errors include source lines. A unit that cannot be represented safely by the current service model fails validation before a process is started.

`JobTimeoutSec=` bounds the pending start transaction for the requested unit,
including dependency and conflict waits. On expiry FractalD rolls back services
started by that transaction, records the request as failed, and queues the
configured `JobTimeoutAction=` for the PID1 manager.

`NotifyAccess=none`, `main`, `exec`, and `all` are parsed and enforced for service status notifications. FractalD enables sender credentials on the notification socket, attributes `main` to the recorded service PID, `exec` to manager-launched lifecycle commands, and `all` to the service cgroup or its process group when no cgroup is available. `Type=notify` and `WatchdogSec=` implicitly use `main` when the directive is omitted or set to `none`.

`DevicePolicy=strict|closed|auto` and `DeviceAllow=` are enforced before service launch with a cgroup eBPF device filter. Strict policies admit only resolved path or `/proc/devices` group rules, closed policies also admit the standard `/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`, and `/dev/urandom` nodes, and an explicit allow list changes `auto` to an allow list. Missing device groups follow systemd's start-time resolution rule and are omitted until a unit arranges for the device provider to load. The service directive inventory includes these controls alongside the existing private device namespace support.

`Delegate=` enables available cgroup v2 controllers below the unit and exposes the unit cgroup's delegation controls to its configured `User=` and `Group=`. `Delegate=yes` selects all controllers available on the host; a controller list selects only those names. Service membership and cleanup include delegated child cgroups.

`PrivateUsers=yes|self|identity|full` creates a Linux user namespace before other service sandbox setup and installs root, service, and supplementary group mappings where required. `identity` and `full` use the first 65,536 identity mappings; `self` uses a minimal mapping. The PID1 boot profile checks both user namespace creation and cgroup delegation.

`KillSignal=` controls ordinary service termination, while `RestartKillSignal=` is used for an explicit restart transaction. `SendSIGHUP=` sends SIGHUP immediately after the selected initial termination signal. Lifecycle children receive the configured `IgnoreSIGPIPE=` disposition, which defaults to the systemd-compatible ignored state.

`DynamicUser=yes` allocates a collision-checked transient numeric UID and GID from FractalD's reserved service range without editing passwd, group, subuid, or subgid databases. The identity is reused when the same loaded unit is reloaded and is released when the unit leaves the manager registry. Unless the unit specifies otherwise, DynamicUser also implies `ProtectSystem=strict`, `ProtectHome=read-only`, and `PrivateTmp=yes`; FractalD-managed directories are created and owned by the allocated identity before launch. When `SupplementaryGroups=` is omitted, inherited supplementary groups are cleared before the service starts.

`Type=dbus` requires `BusName=` and becomes ready after the manager confirms that name is owned on the selected D-Bus. A non-root manager with `DBUS_SESSION_BUS_ADDRESS` probes the user bus; other managers probe the system bus. The probe is outside the service namespace and `ExecStartPost` waits for the readiness transition.

`RequiresMountsFor=` and `WantsMountsFor=` expand the supported unit path specifiers, discover matching `.mount` units, and add the corresponding required or wanted start ordering for every loaded mount point that contains the requested path. Nested mount units are ordered from parent to child, and units started on demand load matching mount dependencies before the transaction begins.

When `FRACTALD_STORAGE_ENABLE=1` or `FRACTALD_STORAGE_FSTAB` is set, FractalD owns the `fstab` mount and swap generation path and suppresses the matching systemd fstab generator. The storage preparation unit scans multi-device Btrfs filesystems, assembles mdraid, activates LVM, and opens keyed `crypttab` mappings. Mount units pass `Type`, `Options`, and the source reference to the installed mount helper; any filesystem understood by that kernel and helper is accepted. RustyBox is selected for declared kernel mounted filesystem types after checking the running kernel's `/proc/filesystems`, while helper based filesystems retain the distribution mount program. The early boot path resolves UUID, LABEL, PARTUUID, and PARTLABEL sources for root, fstab, and crypttab, and normalizes `fat` and `ext` aliases. The parser pass-through coverage includes ext2, ext3, ext4, minix, NTFS/NTFS3, NTFS-3G, UDF, SquashFS, overlay, EROFS, ComposeFS, Bcachefs, ZFS, Ceph, CIFS, FUSE, and SSHFS. The direct Fedora storage matrix covers Btrfs RAID1, ext4, XFS, VFAT, exFAT, F2FS, LVM, mdraid, and LUKS, including a pre-mounted parent chain; an actual mount still requires the selected kernel driver or distribution helper.

`tests/storage-native-recovery.sh` unmounts the matrix, closes the LUKS mapping, deactivates the LVM volume group, stops mdraid, invokes FractalD’s storage preparation service, and remounts every filesystem through generated native mount units. This verifies provider activation and mount execution after the initramfs handoff rather than only checking mounts prepared by early boot.

`DefaultDependencies=` is enabled by default. FractalD synthesizes the standard `sysinit.target`, `basic.target`, `sockets.target`, `timers.target`, `paths.target`, `local-fs-pre.target`, `swap.target`, `umount.target`, and `shutdown.target` relationships supported by each loaded unit type. The relationships are added only when the referenced target is loaded, which keeps isolated unit trees usable; `DefaultDependencies=no` disables the synthesized relationships for a unit.

`systemctl isolate TARGET` and `fractalctl isolate TARGET` require `AllowIsolate=yes` on the target. FractalD starts the target's required and wanted closure, cancels pending starts outside that closure, and stops other loaded units through their normal teardown path. Units marked `IgnoreOnIsolate=yes`, together with their activation dependencies, remain available during the transaction.

`systemctl preset` and `preset-all` read the standard system and user preset directories, with administrator directories overriding a vendor file of the same name. Rules are evaluated in filename order and the first matching `enable`, `disable`, or `ignore` rule wins. Wildcards, template instances, and `--preset-mode=full`, `enable-only`, and `disable-only` are supported; when no preset file exists, the default is to enable the requested unit as systemd does.

`systemctl set-property UNIT Markers=+needs-restart` and `Markers=+needs-reload` retain package update markers in the manager runtime directory. `reload-or-restart --marked` reloads marked active units when they provide `ExecReload=`, falls back to restart when reload is unsupported or fails, clears completed markers, and leaves failed markers for a later retry. `try-reload-or-restart --marked` skips inactive marked units.

`StopWhenUnneeded=yes` reclaims a loaded unit when no active `Requires=` or `Wants=` consumer remains. Dependency chains are reclaimed from the leaf toward their parents, while enabled boot units remain active through their target relationships.

`FailureAction=` and `SuccessAction=` are carried by the native unit specification. A newly failed unit or a service that exits successfully can request `exit`, `halt`, `poweroff`, or `reboot` families, including force and immediate spellings. FractalD queues the strongest request, completes its own dependency ordered stop transaction, and invokes the Linux power operation directly when it is PID 1; action units are not handed to systemd.

`make compatibility-check` runs isolated package-style smoke tests. It exercises `fractalctl enable --now` and `disable --now` in a temporary boot tree, consumes standard target enablement links, creates tmpfiles below a separate temporary root, starts a `Type=notify` unit through `systemctl`, sends readiness through `systemd-notify`, checks the running state through `fractalctl`, and stops the service without touching host service state.

`make unit-inventory-check` scans the standard systemd unit directories on the test host and validates every discovered manager unit through FractalD’s unit inspector. It covers service, target, socket, timer, path, mount, swap, automount, slice, scope, device, and bus-name filenames and fails on any parser error.

`make unit-load-check` loads the complete system unit directory into an isolated FractalD registry with boot targets, enablement, and storage generation disabled. It verifies that package units do not create duplicate service names or alias collisions and that the manager can remain alive after the full registry is assembled. An empty `FRACTALD_BOOT_TARGET` is treated as unset.

`make device-unit-check` starts a service bound to a dynamically resolved `.device` unit, verifies that the transaction waits for the `/dev` path, and removes the path to confirm loss propagation stops the dependent service.

The resolver compatibility check runs a local UDP upstream and an isolated `fractald-resolved`, verifies discovery through `resolvectl`, performs a DNS query, inspects and flushes the cache, changes the upstream set, and confirms graceful cleanup of the endpoint and control records.

`make recovery-check` starts a service that fails twice before becoming healthy, verifies bounded restart recovery, exercises `OnFailure=` recovery, injects resolver `SERVFAIL`, and checks that both daemons remain controllable and clean up their runtime records.

`make package-check` builds and inspects a Fedora RPM when `rpmbuild` is available and builds and inspects a Debian package layout when `dpkg-deb` is available. The repository contains native Debian metadata under `debian/` and a Fedora spec under `packaging/fedora/`; the check is designed to run on both distribution families without host installation.

`journalctl` reads FractalD’s local per-unit journal sink with unit, tail, follow, grep, rotation, and disk-usage operations. `systemd-cat` writes stdin or a command’s captured output into that sink; `systemd-journald` is a native receiver for the configured datagram endpoint and writes the same sink, consuming all inherited `LISTEN_FDS` datagram sockets when socket activated. Service output can forward native datagrams to it.

`systemd-escape` supports unit-safe escaping, path escaping, template instance insertion, suffixes, mangling, and unescaping for package scripts and generators.

`systemd-sysusers` processes the standard `sysusers.d` search path with vendor, runtime, and administrator precedence, `/dev/null` masks, `u`, `u!`, `g`, `m`, and `r` records, quoted fields, dynamic system UID/GID allocation, `--root`, `--replace`, `--inline`, `--dry-run`, `--cat-config`, and `--tldr`. Account database writes are serialized and replaced atomically so package installation scripts can run concurrently without leaving partial passwd or group files.

`systemd-sysctl` processes the standard `sysctl.d` search path with the same precedence and masking rules, applies key/value rules below `/proc/sys`, expands wildcard components such as `net.ipv4.conf.*.rp_filter`, supports ignored errors, `--prefix`, `--strict`, `--dry-run`, `--root`, `--cat-config`, and `--tldr`, and never delegates the operation to a systemd manager.

`systemd-update-helper` handles package transaction requests to persist or remove unit enablement, mark system and user units for restart or reload, and perform the systemd package refresh verbs (`system-reload-restart`, `system-reload`, `system-restart`, `user-reload-restart`, `user-reload`, `user-restart`, and `user-reexec`). It accepts both system and user operation names, records markers in the FractalD runtime directory, safely no-ops before FractalD creates its control socket, and routes active-manager refreshes through FractalD's own control client.

`systemd-machine-id-setup` initializes or reuses a 32-character machine ID below the requested root and supports `--print` and `--commit`. `systemd-detect-virt` reports common container, VM, chroot, private-user, and confidential-VM environments for package conditionals. `systemctl daemon-reexec` is accepted as a successful manager-independent refresh point.

`systemd-analyze cat-config` reads the effective systemd configuration search path, including ordered `.d` drop-ins, and supports `--tldr` for package scripts that need to inspect a setting without comments. `unit-paths` reports the native system unit search directories. `verify` parses a named unit below the selected root and validates its typed directive set using the same configuration parser as the FractalD loader before a service is started.

`kernel-install` provides native kernel transaction handling for Fedora-style Boot Loader Specification type 1 layouts. It copies kernel and initrd images below the selected entry token, writes and removes loader entries, supports `add`, `add-all`, `remove`, `inspect`, and `list`, honors alternate roots and entry options, rebuilds module dependency indexes with `depmod` when a kernel module tree is present, removes those indexes during kernel removal, can invoke `dracut` for a missing initrd on the live root, and runs ordered `.install` hooks from the standard administrator, runtime, local, and vendor directories with `/dev/null` masking and the standard environment. Set `FRACTALD_KERNEL_INSTALL_DEPMOD=0` to suppress the index operation for an image-building environment. Disk-image mode and bootloader-specific firmware mutation remain outside this filesystem-scoped contract.

`udevadm` provides the package-facing device maintenance boundary without a
systemd manager. `trigger` walks sysfs and writes filtered actions to kernel
`uevent` files, `info` reads device properties, `settle` waits for the device
event queue, and `control --reload-rules`, `control --reload`, and
`hwdb --update` accept the common package transaction operations. The same
native binary is installed at `systemd-udevd`; in that invocation it listens
to the kernel uevent netlink channel, records bounded event data, and drains
queue markers for `settle`. `FRACTALD_SYSFS_ROOT`, `FRACTALD_UDEV_ROOT`,
`FRACTALD_UDEV_QUEUE_DIR`, and `FRACTALD_UDEV_RUNTIME_DIR` isolate the paths
for image and conformance tests.

The device-event command does not claim the complete udev rule engine or
binary hardware-database format; FractalD storage resolves tagged block
devices directly and synthetic `.device` units observe kernel paths. Full
udev rule execution and persistent `/dev` naming remain a separate device
subsystem expansion.

## OpenRC

| Surface | Initial contract | Later contract |
| --- | --- | --- |
| Runlevels | `FRACTALD_OPENRC_RUNLEVEL` startup from a runlevel directory with ordered dependencies | stacked and hotplug runlevels |
| Init scripts | `start`, `stop`, and `reload` actions through the shell adapter; `depend()` relationship extraction | `openrc-run` helpers and service supervision |
| Control | `rc-service`, `rc-status` compatible commands | richer OpenRC status fields and helper semantics |

The native FractalD format remains the smallest configuration surface. Compatibility formats are parsed into the same Rust service specification and lifecycle state machine.

## RustyBox toolbox

The package includes an optional multi-call RustyBox userland for rescue and
initramfs profiles. Set `FRACTALD_TOOLBOX_DIR` to its applet-link directory or
set `FRACTALD_RUSTYBOX` to the binary. FractalD uses it for its own native
mount and swap helpers and leaves service `ExecStart` paths unchanged. The
`chaos` applet exposes deterministic Lorenz, Mandelbrot, Lyapunov, Rössler,
logistic-map, and Duffing samples. Its mount applet resolves UUID, LABEL,
PARTUUID, and PARTLABEL sources and handles fstab-only mount metadata before
calling the kernel. Common `fat`, `msdos`, and `ext` aliases are normalized
before the mount call. Initramfs profiles use native `findmnt`, `sort`, `tr`,
`sync`, `switch_root`, `insmod`, and `modprobe` applets and retain the
distribution helpers behind an explicit fallback directory for compressed
modules and helper-backed storage. See `docs/RUSTYBOX.md` for the applet and
build contract.

The distribution also includes `fractald-resolved.service`. It is optional and separately supervised; enabling it does not alter the host resolver configuration automatically.

## Resolver

The optional `libnss_fractald.so.2` module provides glibc forward A/AAAA and reverse PTR lookups through the nameserver path configured for the host. It is installed by `make install` and enabled only when the operator adds `fractald` to the `hosts` entry in `nsswitch.conf`; resolver unavailability returns a fallthrough status where libc supports it.

The resolver process publishes an atomic endpoint record below its runtime directory and serves a private Unix control socket. The `resolvectl` compatibility client discovers the actual UDP listener, reports status and statistics, flushes the bounded cache, performs A/AAAA and other supported DNS queries, and updates upstream servers. `systemd-resolve` is installed as an alias. Discovery and control remain local to the package and do not rewrite `/etc/resolv.conf`.
