# Roadmap

## Foundation

- [x] Define service lifecycle states and transition invariants.
- [x] Add pidfd based process identity and exit observation.
- [x] Exercise the Rust and C boundary with an end to end self check.
- [x] Add structured error types with source locations.

## Supervisor

- [x] Add a service registry and dependency graph.
- [x] Add process groups, ordered timeout escalation, and restart backoff.
- [x] Enforce `StartLimitIntervalSec=` and `StartLimitBurst=` across manual and automatic starts.
- [x] Enforce `RuntimeMaxSec=` with normal timeout, failure, and restart handling.
- [x] Enforce `JobTimeoutSec=` for pending start transactions and queue `JobTimeoutAction=` at PID1.
- [x] Add nonblocking dependency transactions with required failure rollback.
- [x] Enforce `Requisite=` as an already-active prerequisite without implicit start.
- [x] Start `OnSuccess=` completion units after successful service exits.
- [x] Apply `FailureAction=` and `SuccessAction=` through FractalD's ordered manager shutdown path.
- [x] Enforce `RefuseManualStart=` and `RefuseManualStop=` while preserving dependency and shutdown control.
- [x] Implement `AllowIsolate=` and `IgnoreOnIsolate=` target transactions.
- [x] Add readiness tracking for notify and forking services.
- [x] Resolve `RequiresMountsFor=` into parent-to-child mount dependencies.
- [x] Synthesize type-specific `DefaultDependencies=` ordering and shutdown relationships.
- [x] Reclaim `StopWhenUnneeded=` units after their active consumers stop.
- [x] Add a durable bounded state event log and parent-death cleanup.
- [x] Add crash recovery rules for restart and orphan reconciliation.
- [x] Add cgroup v2 resource ownership.
- [x] Apply `CPUQuota=` and `CPUQuotaPeriodSec=` to cgroup v2 `cpu.max` for services and slices.
- [x] Honor service `Slice=` placement, including template instances, in the cgroup v2 hierarchy.
- [x] Add notify watchdog intervals and expiry enforcement.
- [x] Enforce `NotifyAccess=` sender attribution for readiness, watchdog, and status notifications.
- [x] Honor common `KillMode` process-group semantics.
- [x] Honor `KillSignal=`, `RestartKillSignal=`, `SendSIGHUP=`, and `IgnoreSIGPIPE=` lifecycle behavior.
- [x] Add `PrivateTmp` mount namespace isolation.
- [x] Add `PrivateMounts` mount namespace isolation without changing the mount topology.
- [x] Add `PrivateIPC` namespace isolation.
- [x] Add `ConfigurationDirectory` provisioning and `CONFIGURATION_DIRECTORY` export.
- [x] Apply explicit `OOMPolicy` actions from cgroup v2 memory events.
- [x] Gate `Type=dbus` services on manager-observed `BusName=` ownership.
- [x] Add `ProtectSystem`, `ProtectHome`, and explicit writable, read-only, and inaccessible path controls.
- [x] Add cgroup, kernel module, and kernel log protection mounts.
- [x] Add private hostname protection with a UTS namespace and syscall filter.
- [x] Add personality locking through the child seccomp filter.
- [x] Allocate collision-checked transient identities and implied sandbox defaults for `DynamicUser=`.
- [x] Add private device namespaces with safe character devices and isolated `devpts`.
- [x] Enforce `DevicePolicy=` and `DeviceAllow=` with cgroup eBPF filters, including device groups from `/proc/devices`.
- [x] Implement cgroup `Delegate=` controller enablement, delegation ownership, and delegated subtree cleanup.
- [x] Implement `PrivateUsers=` user namespaces with self, identity, and full mappings.
- [x] Apply validated `CapabilityBoundingSet` masks to child capability sets.
- [x] Apply `AmbientCapabilities` after identity changes and before privilege locking.
- [x] Apply explicit `SupplementaryGroups` lists after primary identity setup.
- [x] Remove inherited environment variables with `UnsetEnvironment` for all lifecycle commands.
- [x] Restrict socket and socketpair address families with a child seccomp filter.
- [x] Apply systemd syscall groups, architecture restrictions, and errno actions with child seccomp filters.
- [x] Restrict chmod-family SUID and SGID changes with argument-aware seccomp rules.
- [x] Protect kernel tunables and clock mutation interfaces in the child namespace.
- [x] Apply `ProtectProc` and `ProcSubset` through per-unit procfs mount options.
- [x] Restrict namespace creation and switching with flag-aware seccomp rules.
- [x] Add common host conditions for paths, mounts, kernel arguments, virtualization, security, capabilities, power, firmware, and cgroup controllers.
- [x] Add `LoadCredential` file and credential-store forms, `SetCredential`, `CREDENTIALS_DIRECTORY`, and `%d` credential-directory expansion.
- [x] Import exact and trailing-prefix credentials from standard credential stores with precedence, renaming, and bounded size.
- [x] Add a `systemd-notify` compatibility sender for readiness, watchdog, lifecycle, and status messages.
- [x] Add a `systemd-tmpfiles` compatibility tool for package file and directory rules.
- [x] Add a `systemd-sysusers` compatibility tool for package account allocation and membership rules.
- [x] Add a `systemd-sysctl` compatibility tool for ordered kernel parameter rules and wildcard keys.
- [x] Provide `systemd-update-helper` package transaction operations and update markers.
- [x] Provide native machine-id setup, virtualization detection, and daemon-reexec compatibility tools.
- [x] Provide `systemd-analyze` configuration inspection, unit search-path, and typed verification compatibility for package scripts.
- [x] Provide native `kernel-install` BLS add, remove, list, and inspect operations for kernel transactions.
- [x] Rebuild and clean kernel module dependency indexes during native kernel transactions.
- [x] Provide native `udevadm` trigger, info, settle, rules-control, hwdb-update, and kernel-event listener compatibility paths without a systemd manager.
- [x] Add an isolated package compatibility smoke test covering tmpfiles, notify, systemctl, and shutdown.

## Control plane

- [x] Add a local Unix socket protocol.
- [x] Add `fractalctl` and stable exit codes.
- [x] Add status snapshots.
- [x] Add `--now` enablement operations and blocking `systemctl` compatibility waits.
- [x] Return transaction identifiers for accepted asynchronous service operations.
- [x] Add transaction status queries for accepted lifecycle operations.
- [x] Add subscription events.

## Resolver

- [x] Choose a same-package, separate-process resolver boundary.
- [x] Add `fractald-resolved` with a separately supervised lifecycle.
- [x] Add NSS forward and reverse lookup compatibility.
- [x] Add resolver service discovery.
- [x] Add UDP stub resolver, bounded cache, timeout, and upstream failover behavior.
- [x] Add resolver fault-injection tests.
- [x] Add resolver package compatibility tests.

## Compatibility

- [x] Parse systemd-style service definitions.
- [x] Load systemd search paths and drop-ins.
- [x] Run bounded systemd generators into an owned unit directory.
- [x] Provide the common `systemctl` service operations used by packages.
- [x] Provide `systemctl isolate` and native `fractalctl isolate` target transactions.
- [x] Provide lifecycle properties through the common `systemctl show` query form.
- [x] Add persistent `mask` and `unmask` operations with `/dev/null` mask recognition.
- [x] Add failed-state queries, wildcard unit listing, and reload-or-restart compatibility operations.
- [x] Apply systemd preset policy files with first-match rules, wildcard units, and preset modes.
- [x] Support package update markers through `set-property Markers=` and `reload-or-restart --marked`.
- [x] Add OpenRC init script execution and dependency extraction.
- [x] Add target units, `.wants`/`.requires` relationships, and template instances.
- [x] Add stream and datagram socket activation.
- [x] Add OpenRC runlevel startup plus `rc-service` and `rc-status` shims.
- [x] Add timer, path, mount, swap, slice, and eager automount compatibility unit support; native lazy autofs behavior remains.
- [x] Add native fstab and crypttab generation with Btrfs, mdraid, LVM, and LUKS preparation.
- [x] Resolve tagged root, fstab, crypttab, and toolbox sources, with common filesystem aliases in early boot.
- [x] Preserve arbitrary filesystem types, discover loaded kernel filesystems, and route helper-backed mounts through the distribution dispatcher.
- [x] Resolve `RequiresMountsFor=` and `WantsMountsFor=` into parent-ordered mount transactions.
- [x] Add persistent timer catch-up, randomized timer delay, and explicit accuracy support.

## Boot and operations

- [x] Add boot target and enablement startup sequencing.
- [x] Add ordered signal and control-socket shutdown transactions.
- [x] Add local journal-compatible output forwarding and one-generation log rotation.
- [x] Forward `StandardOutput=journal` and `StandardError=journal` records to a native journal socket with a local fallback.
- [x] Provide a native `systemd-journald` datagram receiver that writes the FractalD journal sink without a systemd manager.
- [x] Add package-manager integration tests on Fedora and Debian.
- [x] Add recovery and fault-injection tests.

## RustyBox

- [x] Add an independent Rust/C multi-call userland crate.
- [x] Add deterministic chaos applet coverage for Lorenz, Mandelbrot, Lyapunov, Rössler, the logistic map, and Duffing.
- [x] Use the toolbox for FractalD-owned mount and swap helpers through an explicit boundary.
- [x] Make toolbox mount and swap helpers resolve UUID, LABEL, PARTUUID, and PARTLABEL sources.
- [x] Add an isolated applet smoke test and package installation paths.
- [x] Add a pruned, independently assembled initramfs profile with FractalD and RustyBox.
- [x] Move the bounded early-boot primitives for kernel logs, mount discovery, text normalization, sorting, synchronization, module insertion, and root handoff into RustyBox.
- [ ] Complete the initramfs shell implementation and replace remaining external boot helpers.
- [ ] Expand applets against package-manager and boot-image conformance suites.

## Direct PID1 validation

- [x] Enter daemon mode when the FractalD binary is invoked without arguments as PID 1.
- [x] Reap orphaned children adopted by PID 1 while preserving pidfd-owned service exits.
- [x] Add an isolated Fedora boot smoke profile for direct kernel `init=` execution.
- [x] Exercise a multi-disk Fedora matrix with Btrfs RAID1, ext4, XFS, VFAT, exFAT, F2FS, LVM, mdraid, and LUKS.
- [x] Validate `DynamicUser=` identity allocation and managed-directory ownership in the direct Fedora PID1 boot.
- [ ] Complete Fedora boot inventory coverage for every enabled package unit and action path.
- [ ] Validate hardware-specific graphics, power, firmware, and suspend behavior on the physical ThinkPad.
