# FractalD architecture

FractalD is a complete native boot and service boundary. The process can run as
an ordinary user daemon for development, or as the kernel's PID1. The same
supervisor code is used in both modes; PID1 mode adds mount topology setup,
child adoption, shutdown signal handling, and kernel power actions.

## Boot path

The bootloader or initramfs starts `/usr/bin/fractald` with `init=`. FractalD
creates or reuses procfs, sysfs, `/run`, `/dev`, cgroup v2, devpts, shared
memory, and message queue mounts. It loads `.svc` files, validates every native
directive, creates the package generated storage descriptors, and starts the
selected profile.

The boot profile is selected in this order:

1. `FRACTALD_BOOT_PROFILE`.
2. `profile=` in `FRACTALD_BOOT_PROFILE_FILE` or `/etc/fractald/boot.conf`.
3. `boot` when the process is PID1.

The package trigger keeps package owned profile markers current. A package
transaction therefore changes the next boot and, when a daemon is running, the
current descriptor registry after a reload.

## Service graph

Each descriptor becomes a `ServiceSpec`. Required and wanted dependencies are
closed before a transaction is planned. Ordering edges are topologically
sorted; explicit cycles are reported. There are no hidden manager service
names. A package or administrator can define a grouping profile as a native
`kind=group` descriptor and choose its dependencies explicitly.

The supervisor tracks one record per service, its generation, process identity,
readiness state, restart budget, resource ownership, and event history. pidfds
are used when the kernel supports them. Process groups and the configured kill
mode provide cleanup for children that do not expose a pidfd.

Listeners, timers, path watches, mounts, swaps, and device waits are all
service records with native trigger data. A listener can pass descriptors to a
process; a notify service can report readiness over
`FRACTALD_NOTIFY_SOCKET`; a watchdog uses `FRACTALD_WATCHDOG_USEC`.

## Storage

`fractald-storage` parses the host tables without importing another manager's
format. It emits `storage-prepare.svc`, `storage.svc`, mount descriptors,
swap descriptors, and `device-*` wait services. The preparation service runs
block discovery, RAID assembly, volume activation, and encrypted mapping setup
before filesystem services are started.

## Control and state

`fractald-control` exposes a permission protected Unix socket with status,
transaction, reload, lifecycle, event subscription, and shutdown operations.
Persistent enablement and masks are regular files below the FractalD state
directory. Updates are written atomically and never use symlinked markers.

## Linux boundary

Rust owns policy, parsing, graph construction, and lifecycle decisions. The C
layer only wraps operations that require Linux ABI calls: pidfds, namespaces,
mounts, cgroups, seccomp, capabilities, netlink, signals, and power actions.
Every wrapper returns an errno based result to Rust.
