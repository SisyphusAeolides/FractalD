# Architecture

## Runtime layers

```text
native policy / systemd units / OpenRC scripts
                    |
          compatibility and config layer
                    |
              Rust supervisor core
             /          |          \
    service processes  listeners  fractald-resolved
                    |
          Linux platform boundary in C
                    |
              pidfds, signals,
            waitid, groups, fds
```

The Rust core is the authority for service state. Adapters translate external descriptions and commands into core operations; they do not maintain a second lifecycle model.

Service stdin, stdout, and stderr are selected before a child is spawned. File and tty descriptors are opened by the supervisor, while socket activated stdin is mapped from fd 3 after descriptor preparation. Per-connection stream sockets are accepted by the supervisor and passed to the service as fd 0, with `StandardOutput=socket` or `StandardError=socket` receiving a duplicate. The default journal-compatible mode writes each output stream to a mode 0600 per-unit file in the manager log directory and forwards line records to the native journal datagram socket when available; explicit null, inherited, tty, socket, and file modes are also supported. Append logs rotate one backup when a new writer opens a file that has reached the configured threshold. `journalctl` and `systemd-cat` operate on this local sink, while `FRACTALD_JOURNAL_SOCKET` selects a test or alternate native endpoint.

## Resolver boundary

FractalD distributes its resolver in the same package as the manager, with `fractald-resolved` as a separately supervised process. The package boundary keeps installation and versioning simple; the process boundary keeps a DNS parser, cache, and network event loop from becoming part of the service manager’s failure domain.

The resolver exposes a standard UDP stub listener, reads upstream nameservers from its own configuration, and keeps a bounded TTL cache. FractalD owns its startup ordering, restart policy, resource limits, and shutdown, while the resolver owns name resolution state. The manager must remain usable when the resolver is disabled or unavailable, including for local service control. `fractald-resolved` atomically publishes its actual listener, control socket, PID, upstreams, and cache limits in a runtime discovery record; its private Unix control socket serves status, statistics, cache flush, and upstream update requests. `resolvectl` consumes this record without changing host resolver configuration. `FRACTALD_RESOLVED_FAULT` injects deterministic drop, `SERVFAIL`, or delay behavior for outage tests. The optional `libnss_fractald.so.2` module uses the configured resolver path for forward A/AAAA and reverse PTR lookups.

## Storage boundary

The native storage adapter reads `fstab` and `crypttab` into the same unit graph used by services. One preparation unit runs the available topology tools in a stable order: Btrfs device discovery, mdraid assembly, LVM activation, and encrypted mapping setup. Mount and swap units pass the declared filesystem type and options to the installed helpers, so filesystem support follows the kernel and userspace toolset instead of a FractalD filesystem allowlist. RustyBox handles declared kernel mounted filesystem types through direct mount syscalls; helper based filesystems keep the distribution mount program. Device references use UUID, LABEL, PARTUUID, PARTLABEL, or explicit paths; early boot resolves tagged root, fstab, and encrypted devices directly when udev has not populated `/dev/disk` yet. Common `fat` and `ext` fstab aliases are normalized to `vfat` and `ext4`. A pre-existing parent mount is treated as available when its mount condition skips the parent unit, allowing nested Btrfs, bind, and ordinary filesystem mounts to proceed.

When launched without a subcommand as process 1, `fractald` enters its daemon loop directly. The PID1 path installs a child subreaper, retains pidfd ownership for managed service processes, and sweeps unrelated adopted children so they do not remain as zombies. A shutdown request stops services in dependency order before invoking the requested kernel power action. The isolated `tests/pid1-*` profile boots this path with the kernel `init=` parameter and checks the control socket, service lifecycle, RustyBox workload, orphan reaping, and ordered shutdown.

Resource limits and `Slice=` placement use the cgroup v2 filesystem from Rust. When enabled, each service receives a stable cgroup below `fractald`; slice names create their parent hierarchy and template instances expand before placement. The supervisor attaches the launched process before publishing it as active and removes empty managed groups during teardown. Explicit `OOMPolicy` values create a cgroup even without other limits, snapshot `memory.events` after attachment, and apply stop or group kill when the `oom` counter advances. Stale groups are reconciled recursively at manager startup.

## Lifecycle invariants

- A service can have at most one owned process at a time.
- A process event is accepted only for the current process identity and generation.
- An explicit stop cancels pending restarts.
- Starting a unit stops loaded units that conflict with it, including reverse `Conflicts=` declarations, before the start transaction proceeds.
- `PartOf=` dependents follow explicit owner stops and restarts; `BindsTo=` pulls in its bound units and stops dependents when a bound unit leaves the active state; `Requisite=` checks that a named prerequisite is already active without pulling it into the transaction; synthetic `.device` units wait for escaped `/dev` and `/sys` paths and report hot unplug; `OnFailure=` starts configured recovery units once when a service enters `Failed`, and `OnSuccess=` starts configured completion units after a successful exit.
- Persistent masks and systemd `/dev/null` masks prevent a unit from entering the supervisor registry; masking a loaded unit stops it before configuration is reloaded.
- Restart policy is evaluated only after an observed exit.
- Restart attempts are bounded by the service policy, and configured `StartLimitIntervalSec=`/`StartLimitBurst=` values add a sliding-window limit across manual and automatic starts; `RuntimeMaxSec=` turns an overlong running service into a supervised failure.
- A timeout produces an ordered escalation: configured signal first, `SIGKILL` after the stop deadline, with `KillMode` deciding whether each signal targets the main process or its process group; a start timeout remains a failed start after the process exits.
- `JobTimeoutSec=` bounds a pending start transaction, including dependency and conflict waits; expiry rolls back that transaction and queues `JobTimeoutAction=` for the PID1 manager.
- Start and stop hooks run as separate asynchronous helper processes and cannot block the daemon accept loop.
- A successful `RemainAfterExit` service is active with no main PID until explicitly stopped.
- A false start condition becomes `Skipped` without spawning a process.
- `DynamicUser=yes` assigns a transient numeric UID and GID before directory provisioning and identity setup, while preserving an existing assignment across reloads of the same loaded unit; its implied filesystem protections are applied only when the unit has not explicitly selected those settings, and inherited supplementary groups are cleared unless the unit supplies an explicit list.
- A notify service becomes dependency-ready only after `READY=1`; a forking service becomes ready only after its daemon child is adopted.
- A `Type=dbus` service becomes dependency-ready only after the manager confirms its configured `BusName=` is owned; its readiness probe remains outside the service namespace.
- Notification datagrams carry Linux sender credentials. `NotifyAccess=main` accepts only the recorded service PID, `exec` also accepts manager-launched lifecycle helpers, and `all` accepts processes in the service cgroup or its process group fallback; `Type=notify` and `WatchdogSec=` implicitly use `main`.
- `RequiresMountsFor=` and `WantsMountsFor=` are resolved against the loaded mount registry at transaction planning time; every matching parent mount is ordered before nested mounts and the requesting unit, with the former retaining required failure semantics.
- `DefaultDependencies=` adds type-specific startup, synchronization-target, and shutdown relationships for loaded standard targets, while `DefaultDependencies=no` suppresses those synthesized edges.
- `StopWhenUnneeded=yes` reclaims active units only after no active consumer retains a required, wanted, or socket-activation relationship.
- A socket unit owns its listeners until stop; ordinary listeners pass `LISTEN_FDS`, `LISTEN_PID`, and `LISTEN_FDNAMES`, while an `Accept=yes` stream listener passes one accepted connection to a service template. Filesystem listener paths persist across stop by default and are unlinked when `RemoveOnStop=yes` is configured.
- A timer or path unit owns its trigger state until stop and starts its associated service only when a schedule or watch becomes due. Timer jitter and explicit `AccuracySec=` coalescing delays are applied to scheduled fires, and persistent timers record their last fire atomically so missed calendar or interval windows can be caught up after a restart.
- Managed runtime directories are cleaned only when FractalD created them and the unit did not request preservation; configuration, state, cache, and log directories persist. Configuration directories remain read-only when `ProtectSystem=strict` is active.
- `CapabilityBoundingSet` drops the child bounding set after namespace setup but before identity changes, then applies the permitted, effective, and inheritable masks after identity setup so `User=` can still be honored when the mask excludes `CAP_SETUID` or `CAP_SETGID`. When `AmbientCapabilities` is configured, the child enables keep capabilities before changing identity, restores the requested permitted and effective bits, adds them to the inheritable set, and raises the ambient set before `NoNewPrivileges` and seccomp are installed. `RestrictAddressFamilies` is installed after ambient capability setup and denies disallowed `socket(2)` and `socketpair(2)` families. `SystemCallFilter` is resolved from the standard syscall groups and installed after identity, descriptor preparation, and `NoNewPrivileges`; allow lists, deny lists, per-call errno actions, and `SystemCallArchitectures=native` are compiled into one child seccomp program. `RestrictSUIDSGID` adds an argument-aware chmod-family filter that allows mode changes without SUID or SGID bits. `RestrictNamespaces` adds flag-aware checks for namespace creation and switching. `ProtectClock` removes `CAP_SYS_TIME` where possible and blocks clock mutation syscalls. `ProtectProc` and `ProcSubset` are applied while the private mount namespace is being built, using `hidepid=` and `subset=pid` procfs options when supported. `PrivateTmp`, `PrivateMounts`, `PrivateIPC`, `PrivateNetwork`, `ProtectSystem`, and `ProtectHome` are applied before identity changes. `PrivateTmp=yes` binds per-unit host-backed temporary directories into `/tmp` and `/var/tmp`; `disconnected` mounts fresh tmpfs instances. `PrivateMounts=yes` creates a private mount namespace while keeping the service's mount topology unchanged. `PrivateIPC=yes` creates a private IPC namespace. `PrivateDevices=yes` creates a private `/dev` with a minimal safe device set and isolated `devpts` and shared memory mounts. `PrivateNetwork=yes` enters a separate network namespace. `ProtectSystem` remounts system paths read-only, `ProtectHome` restricts home trees, `ReadWritePaths` plus FractalD-managed directories are restored after the read-only mount is established, and `ReadOnlyPaths` or `InaccessiblePaths` apply explicit path restrictions in the child mount namespace. `ProtectControlGroups`, `ProtectKernelModules`, `ProtectKernelTunables`, and `ProtectKernelLogs` apply additional read-only or inaccessible mounts for kernel control interfaces. `ProtectHostname` adds a private UTS namespace before identity setup and installs its hostname syscall filter afterward. `LockPersonality` blocks personality changes with a seccomp rule. Private directories are reused by lifecycle helpers and removed after teardown; namespace setup errors fail that child spawn.
- Service output destinations are opened before launch, use restrictive permissions, and fail the spawn transaction when an explicitly requested destination cannot be opened.
- Append logs rotate before a new writer is attached once the configured size limit is reached; rotation retains one backup per stream.
- A successful stop never enters the restart path.
- `FailureAction=` and `SuccessAction=` are queued by the supervisor and consumed by the daemon only after its service shutdown transaction has settled; deliberate shutdown drains pending unit actions instead of recursively triggering another manager transition.
- State changes are appended with a monotonic sequence and flushed to a bounded persistent event log; control clients can replay the log or hold a subscription stream for new records; an unexpected manager exit arms directly managed children with a parent-death signal, and the next manager start reconciles stale FractalD-owned cgroups.

## Repository layout

- `crates/fractald-core`: side effect free service specification and lifecycle transitions.
- `crates/fractald-config`: systemd unit parser and conversion into native service specifications.
- `crates/fractald-supervisor`: child process, pidfd, process-group, restart, timeout, and dependency execution.
- `crates/fractald-chaos`: deterministic Lorenz, Mandelbrot, Lyapunov, Rössler, logistic map, and Duffing models with Rust and C kernels.
- `crates/rustybox`: independent Rust/C multi-call userland with mount, swap, filesystem, process, and chaos applets.
- `crates/fractald-control`: local control protocol, runtime paths, and persistent state paths.
- `crates/fractald-storage`: fstab and crypttab parsing plus native mount, swap, and topology unit generation.
- `crates/fractald-platform`: C backed Linux primitives exposed through a narrow Rust API.
- `crates/fractald`: daemon entry point and service supervisor.
- `crates/fractalctl`: operator commands and boot integration.
- `crates/fractald-systemctl`: package-facing `systemctl` compatibility executable.
- `crates/fractald-resolved`: separately packaged resolver process with an upstream-failover stub, bounded cache, and local control socket.
- `crates/fractald-resolver`: resolver discovery record and DNS client shared by the resolver tools.
- `crates/fractald-resolvectl`: `resolvectl` compatibility client.
- `chaos`: declarative policy sources.
- `docs`: compatibility contracts and implementation decisions.

## Compatibility boundaries

Systemd compatibility is implemented as a unit reader and control protocol adapter. Common service directives, environment files, credential files and credential-store imports, specifiers, conditions, `ExecCondition`, managed service directories, executable generators with bounded execution, dependency edges, `Conflicts`, `PartOf`, `BindsTo`, `OnFailure`, manual-start and manual-stop refusal, enablement, masking, process groups, `KillMode`, `User`/`Group`, `DynamicUser`, lifecycle hooks, output modes, native journal datagram forwarding, process hardening, `LimitNOFILE`, `LimitMEMLOCK`, `LimitNPROC`, watchdog notifications, the `systemd-notify` sender, `systemd-tmpfiles` path management, `systemd-escape` name conversion, resource limits, local output logs, target transactions, template instances, socket activation with descriptor names, timer schedules, path watches, mount and swap units, slice lifecycle nodes, eager automount transactions, manager shutdown action units, `ProtectSystem`, `ProtectHome`, `ProtectControlGroups`, `ProtectKernelModules`, `ProtectKernelTunables`, `ProtectKernelLogs`, `ProtectClock`, `ProtectProc`, `ProcSubset`, `ProtectHostname`, `LockPersonality`, `SystemCallFilter`, `SystemCallArchitectures`, `SystemCallErrorNumber`, `RestrictSUIDSGID`, `RestrictNamespaces`, `ReadWritePaths`, `ReadOnlyPaths`, `InaccessiblePaths`, durable state events, lifecycle properties through `systemctl show`, shutdown actions, and the common control operations are available in the current slice. Native lazy autofs behavior, broader namespace controls, native journal querying, and retention remain separate work.

OpenRC compatibility is implemented as an init script adapter. Scripts run through an explicit shell helper, `depend()` words become graph edges, action return status is converted into supervisor events, and a selected runlevel directory can seed daemon startup. `rc-service` and `rc-status` are thin control clients over the same protocol.

RustyBox is a separate userland boundary. Its Rust dispatcher and applets use a narrow C syscall layer, and its deterministic `chaos` applet shares the six models used by FractalD. When `FRACTALD_TOOLBOX_DIR` or `FRACTALD_RUSTYBOX` is configured, only FractalD-owned mount and swap helpers select those applets; service command paths remain unchanged. This keeps the init independent from both systemd and the compatibility userland.
