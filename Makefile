.PHONY: build release nss rustybox-profile install check test fmt fmt-check \
    run-self-check native-check package-check pid1-check pid1-static-initramfs \
    chaos-check clean

PREFIX ?= /usr/local
BINDIR ?= $(PREFIX)/bin
NSSLIBDIR ?= $(PREFIX)/lib
DATADIR ?= $(PREFIX)/share
SYSCONFDIR ?= /etc
LIBEXECDIR ?= $(PREFIX)/libexec
SERVICE_DIR ?= $(PREFIX)/lib/fractald/services
ALPM_HOOK_DIR ?= $(DATADIR)/libalpm/hooks
RUSTYBOX_TOOLBOX_DIR ?= $(LIBEXECDIR)/fractald/toolbox
RUSTYBOX_LINK_TARGET ?= ../../../bin/rustybox
DESTDIR ?=

RUSTYBOX_APPLETS = cat chaos chroot dmesg echo env false findmnt init insmod kill ln ls mkdir modprobe mount mv pwd readlink rm rmdir sleep sort swapoff swapon switch_root sync tr true umount uname which
NATIVE_TOOLS = fractald fractalctl fractald-analyze fractald-detect-virt fractald-escape \
    fractald-journalctl fractald-cat fractald-journald kernel-install \
    fractald-machine-id fractald-notify fractald-package-trigger fractald-resolved \
    fractald-resolvectl fractald-sysctl fractald-sysusers fractald-tmpfiles \
    fractald-udevadm fractald-launch rustybox

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

install: release nss
	for tool in $(NATIVE_TOOLS); do \
		test -x target/release/$$tool || { echo "missing release tool: $$tool" >&2; exit 1; }; \
		install -Dm755 target/release/$$tool $(DESTDIR)$(BINDIR)/$$tool || exit 1; \
	done
	ln -sfn fractald-udevadm $(DESTDIR)$(BINDIR)/fractald-udevd
	install -Dm755 target/release/libnss_fractald.so.2 $(DESTDIR)$(NSSLIBDIR)/libnss_fractald.so.2
	install -d $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)
	for applet in $(RUSTYBOX_APPLETS); do ln -sfn $(RUSTYBOX_LINK_TARGET) $(DESTDIR)$(RUSTYBOX_TOOLBOX_DIR)/$$applet; done
	install -Dm644 crates/rustybox/APPLET-MANIFEST.txt $(DESTDIR)$(LIBEXECDIR)/fractald/rustybox-applets.txt
	install -d $(DESTDIR)$(SERVICE_DIR)
	for service in packaging/arch/services/*.svc; do install -Dm644 $$service $(DESTDIR)$(SERVICE_DIR)/$$(basename $$service); done
	install -Dm644 packaging/arch/90-fractald-package.hook $(DESTDIR)$(ALPM_HOOK_DIR)/90-fractald-package.hook
	install -Dm644 packaging/arch/boot.conf.example $(DESTDIR)$(DATADIR)/doc/fractald/boot.conf.example
	install -Dm755 target/release/fractald $(DESTDIR)$(PREFIX)/lib/fractald/init
	install -Dm644 README.md $(DESTDIR)$(DATADIR)/doc/fractald/README.md
	install -Dm644 docs/ARCHITECTURE.md $(DESTDIR)$(DATADIR)/doc/fractald/ARCHITECTURE.md
	install -Dm644 docs/CONFIGURATION.md $(DESTDIR)$(DATADIR)/doc/fractald/CONFIGURATION.md
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

native-check: build
	sh tests/native-service-smoke.sh
	sh tests/native-package-trigger-smoke.sh
	sh tests/native-helper-smoke.sh
	sh tests/native-journal-smoke.sh
	sh tests/native-storage-smoke.sh

package-check: release nss
	sh tests/native-package-layout.sh

pid1-check:
	sh tests/pid1-smoke.sh

pid1-static-initramfs: release
	sh tests/pid1-initramfs-build.sh

chaos-check: build
	@target/debug/fractald self-check
	@target/debug/rustybox chaos list >/dev/null

clean:
	cargo clean
