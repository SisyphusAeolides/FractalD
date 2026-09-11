.PHONY: build release nss rustybox-profile rustybox-check install check test fmt fmt-check run-self-check compatibility-check fractalctl-now-check resolver-compatibility-check recovery-check manager-action-check job-timeout-check unit-inventory-check unit-load-check enablement-check preset-check marker-check sysusers-check sysctl-check update-helper-check host-tools-check analyze-check kernel-install-check udevadm-check journal-daemon-check journald-activation-check device-unit-check pid1-check pid1-storage-check pid1-storage-recovery-check pid1-static-initramfs chaos-check package-check clean

PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
NSSLIBDIR ?= $(PREFIX)/lib
DATADIR ?= $(PREFIX)/share
SYSCONFDIR ?= /etc
SYSTEMDUNITDIR ?= $(PREFIX)/lib/systemd/system
LIBEXECDIR ?= $(PREFIX)/libexec
RUSTYBOX_TOOLBOX_DIR ?= $(LIBEXECDIR)/fractald/toolbox
RUSTYBOX_LINK_TARGET ?= ../../../bin/rustybox
DESTDIR ?=

RUSTYBOX_APPLETS = cat chaos chroot dmesg echo env false findmnt init insmod kill ln ls mkdir modprobe mount mv pwd readlink rm rmdir sleep sort swapoff swapon switch_root sync tr true umount uname which

build:
	cargo build --workspace

release:
	cargo build --release --workspace

nss:
	mkdir -p target/release
	$(CC) -std=c11 -O2 -Wall -Wextra -Werror -fPIC -shared -Wl,-soname,libnss_fractald.so.2 -o target/release/libnss_fractald.so.2 crates/fractald-nss/fractald_nss.c -lresolv

rustybox-profile: release
	install -d $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)
	install -Dm755 target/release/rustybox $(DESTDIR)$(BINDIR)/rustybox
	for applet in $(RUSTYBOX_APPLETS); do ln -sfn $(RUSTYBOX_LINK_TARGET) $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)/$$applet; done
	install -Dm644 crates/rustybox/APPLET-MANIFEST.txt $(DESTDIR)$(LIBEXECDIR)/fractald/rustybox-applets.txt

rustybox-check: build
	sh tests/rustybox-smoke.sh

install: release nss
	install -Dm755 target/release/fractald $(DESTDIR)$(BINDIR)/fractald
	install -Dm755 target/release/rustybox $(DESTDIR)$(BINDIR)/rustybox
	install -d $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)
	for applet in $(RUSTYBOX_APPLETS); do ln -sfn $(RUSTYBOX_LINK_TARGET) $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)/$$applet; done
	install -Dm644 crates/rustybox/APPLET-MANIFEST.txt $(DESTDIR)$(LIBEXECDIR)/fractald/rustybox-applets.txt
	install -Dm755 target/release/fractald-launch $(DESTDIR)$(BINDIR)/fractald-launch
	install -Dm755 target/release/fractalctl $(DESTDIR)$(BINDIR)/fractalctl
	install -Dm755 target/release/systemctl $(DESTDIR)$(BINDIR)/systemctl
	install -Dm755 target/release/rc-service $(DESTDIR)$(BINDIR)/rc-service
	install -Dm755 target/release/rc-status $(DESTDIR)$(BINDIR)/rc-status
	install -Dm755 target/release/systemd-notify $(DESTDIR)$(BINDIR)/systemd-notify
	install -Dm755 target/release/systemd-tmpfiles $(DESTDIR)$(BINDIR)/systemd-tmpfiles
	install -Dm755 target/release/systemd-sysusers $(DESTDIR)$(BINDIR)/systemd-sysusers
	install -Dm755 target/release/systemd-sysusers $(DESTDIR)$(PREFIX)/lib/systemd/systemd-sysusers
	install -Dm755 target/release/systemd-sysctl $(DESTDIR)$(BINDIR)/systemd-sysctl
	install -Dm755 target/release/systemd-sysctl $(DESTDIR)$(PREFIX)/lib/systemd/systemd-sysctl
	install -Dm755 target/release/systemd-update-helper $(DESTDIR)$(BINDIR)/systemd-update-helper
	install -Dm755 target/release/systemd-update-helper $(DESTDIR)$(PREFIX)/lib/systemd/systemd-update-helper
	install -Dm755 target/release/systemd-machine-id-setup $(DESTDIR)$(BINDIR)/systemd-machine-id-setup
	install -Dm755 target/release/systemd-detect-virt $(DESTDIR)$(BINDIR)/systemd-detect-virt
	install -Dm755 target/release/systemd-analyze $(DESTDIR)$(BINDIR)/systemd-analyze
	install -Dm755 target/release/udevadm $(DESTDIR)$(BINDIR)/udevadm
	install -Dm755 target/release/udevadm $(DESTDIR)$(PREFIX)/lib/systemd/systemd-udevd
	install -Dm755 target/release/kernel-install $(DESTDIR)$(BINDIR)/kernel-install
	ln -sf kernel-install $(DESTDIR)$(BINDIR)/installkernel
	install -Dm755 target/release/systemd-escape $(DESTDIR)$(BINDIR)/systemd-escape
	install -Dm755 target/release/resolvectl $(DESTDIR)$(BINDIR)/resolvectl
	ln -sf resolvectl $(DESTDIR)$(BINDIR)/systemd-resolve
	install -Dm755 target/release/journalctl $(DESTDIR)$(BINDIR)/journalctl
	install -Dm755 target/release/systemd-cat $(DESTDIR)$(BINDIR)/systemd-cat
	install -Dm755 target/release/systemd-journald $(DESTDIR)$(PREFIX)/lib/systemd/systemd-journald
	install -Dm755 target/release/fractald-resolved $(DESTDIR)$(BINDIR)/fractald-resolved
	install -Dm755 target/release/libnss_fractald.so.2 $(DESTDIR)$(NSSLIBDIR)/libnss_fractald.so.2
	install -Dm644 packaging/fractald.service.in $(DESTDIR)$(SYSTEMDUNITDIR)/fractald.service
	sed -i 's|@FRACTALD_BINDIR@|$(BINDIR)|g' $(DESTDIR)$(SYSTEMDUNITDIR)/fractald.service
	install -Dm644 packaging/fractald-resolved.service.in $(DESTDIR)$(SYSCONFDIR)/fractald/services/fractald-resolved.service
	sed -i 's|@FRACTALD_BINDIR@|$(BINDIR)|g' $(DESTDIR)$(SYSCONFDIR)/fractald/services/fractald-resolved.service
	install -Dm644 README.md $(DESTDIR)$(DATADIR)/doc/fractald/README.md
	install -Dm644 docs/ARCHITECTURE.md $(DESTDIR)$(DATADIR)/doc/fractald/ARCHITECTURE.md
	install -Dm644 docs/COMPATIBILITY.md $(DESTDIR)$(DATADIR)/doc/fractald/COMPATIBILITY.md
	install -Dm644 docs/RUSTYBOX.md $(DESTDIR)$(DATADIR)/doc/fractald/RUSTYBOX.md
	install -Dm644 crates/rustybox/APPLET-MANIFEST.txt $(DESTDIR)$(DATADIR)/doc/fractald/RUSTYBOX-APPLETS.txt

check: nss
	cargo check --workspace

test:
	cargo test --workspace

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

run-self-check:
	cargo run -p fractald -- self-check

compatibility-check: build
	sh tests/fractalctl-now-smoke.sh
	sh tests/compatibility-smoke.sh
	sh tests/resolver-compatibility.sh
	sh tests/rustybox-smoke.sh
	sh tests/device-unit-smoke.sh
	sh tests/systemd-enablement-smoke.sh
	sh tests/systemd-preset-smoke.sh
	sh tests/systemd-markers-smoke.sh
	sh tests/systemd-sysusers-smoke.sh
	sh tests/systemd-sysctl-smoke.sh
	sh tests/systemd-update-helper-smoke.sh
	sh tests/systemd-host-tools-smoke.sh
	sh tests/systemd-analyze-smoke.sh
	sh tests/kernel-install-smoke.sh
	sh tests/udevadm-smoke.sh
	sh tests/journal-daemon-smoke.sh
	sh tests/journald-activation-smoke.sh
	sh tests/systemd-unit-load.sh
	sh tests/manager-action-smoke.sh
	sh tests/job-timeout-smoke.sh

fractalctl-now-check: build
	sh tests/fractalctl-now-smoke.sh

resolver-compatibility-check: build
	sh tests/resolver-compatibility.sh

recovery-check: build
	sh tests/recovery-fault-smoke.sh

manager-action-check: build
	sh tests/manager-action-smoke.sh

job-timeout-check: build
	sh tests/job-timeout-smoke.sh

unit-inventory-check: release
	sh tests/systemd-unit-inventory.sh

unit-load-check: build
	sh tests/systemd-unit-load.sh

enablement-check: build
	sh tests/systemd-enablement-smoke.sh

preset-check: build
	sh tests/systemd-preset-smoke.sh

marker-check: build
	sh tests/systemd-markers-smoke.sh

sysusers-check: build
	sh tests/systemd-sysusers-smoke.sh

sysctl-check: build
	sh tests/systemd-sysctl-smoke.sh

update-helper-check: build
	sh tests/systemd-update-helper-smoke.sh

host-tools-check: build
	sh tests/systemd-host-tools-smoke.sh

analyze-check: build
	sh tests/systemd-analyze-smoke.sh

kernel-install-check: build
	sh tests/kernel-install-smoke.sh

udevadm-check: build
	sh tests/udevadm-smoke.sh

journal-daemon-check: build
	sh tests/journal-daemon-smoke.sh

journald-activation-check: build
	sh tests/journald-activation-smoke.sh

device-unit-check: build
	sh tests/device-unit-smoke.sh

pid1-check:
	sh tests/pid1-smoke.sh

pid1-storage-check:
	sh tests/storage-matrix-smoke.sh

pid1-storage-recovery-check:
	sh tests/storage-native-recovery.sh

pid1-static-initramfs: release
	FRACTALD_INITRAMFS_PROFILE=static sh tests/pid1-initramfs-build.sh

package-check: release nss
	sh tests/package-manager-smoke.sh

chaos-check:
	@command -v chaos >/dev/null 2>&1 || { echo "chaos compiler is required for this check" >&2; exit 1; }
	@chaos chaos/policy.kaos

clean:
	cargo clean
