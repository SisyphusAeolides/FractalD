# Roadmap

## Completed foundation

- Native `.svc` parser with strict schema validation.
- Explicit dependency graph and transaction planning.
- pidfd, process group, restart, timeout, watchdog, and readiness handling.
- Native listeners, timers, path watches, mounts, swaps, and device waits.
- Credentials, namespaces, resource limits, cgroup ownership, and hardening.
- Native PID1 mount setup, child adoption, shutdown, and power actions.
- Arch pacman package detection and post transaction reload hook.
- Local control socket, persistent markers, masks, and event subscriptions.
- Arch package metadata, native helper tools, and initramfs builder.

## Next expansions

- Netlink based device policy refresh for hotplugged hardware.
- Parallel storage preparation with bounded per provider timeouts.
- Incremental package ownership updates for very large pacman databases.
- More Arch kernel and initramfs integration tests under QEMU.
- A stable versioned descriptor schema and migration checker.
