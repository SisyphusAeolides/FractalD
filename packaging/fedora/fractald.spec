Name:           fractald
Version:        0.1.0
Release:        1%{?dist}
Summary:        FractalD service manager and compatibility layer
License:        NOASSERTION
URL:            https://copr.fedorainfracloud.org/coprs/sisyphuscode/fractald
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  systemd-rpm-macros
Requires:       dbus-tools

%description
FractalD provides a supervised Linux service manager with systemd-style unit
compatibility, OpenRC adapters, local control tools, and an optional separately
supervised resolver. It also ships an independent RustyBox Rust/C multi-call
userland for rescue and initramfs profiles.

%prep
%autosetup

%build
cargo build --release --workspace --offline
mkdir -p target/release
%{__cc} %{optflags} -std=c11 -Wall -Wextra -Werror -fPIC -shared -Wl,-soname,libnss_fractald.so.2 -o target/release/libnss_fractald.so.2 crates/fractald-nss/fractald_nss.c -lresolv

%check
cargo test --workspace --offline -- --test-threads=1

%install
%make_install PREFIX=%{_prefix} BINDIR=%{_bindir} NSSLIBDIR=%{_libdir} DATADIR=%{_datadir} SYSCONFDIR=%{_sysconfdir} SYSTEMDUNITDIR=%{_unitdir}

%post
%systemd_post fractald.service

%preun
%systemd_preun fractald.service

%postun
%systemd_postun_with_restart fractald.service

%files
%{_bindir}/fractald
%{_bindir}/rustybox
%{_bindir}/fractald-launch
%{_bindir}/fractalctl
%{_bindir}/systemctl
%{_bindir}/rc-service
%{_bindir}/rc-status
%{_bindir}/systemd-notify
%{_bindir}/systemd-tmpfiles
%{_bindir}/systemd-sysusers
%{_bindir}/systemd-sysctl
%{_bindir}/systemd-update-helper
%{_bindir}/systemd-machine-id-setup
%{_bindir}/systemd-detect-virt
%{_bindir}/systemd-analyze
%{_bindir}/udevadm
%{_prefix}/lib/systemd/systemd-udevd
%{_bindir}/kernel-install
%{_bindir}/installkernel
%{_prefix}/lib/systemd/systemd-sysusers
%{_prefix}/lib/systemd/systemd-sysctl
%{_prefix}/lib/systemd/systemd-update-helper
%{_bindir}/systemd-escape
%{_bindir}/resolvectl
%{_bindir}/systemd-resolve
%{_bindir}/journalctl
%{_bindir}/systemd-cat
%{_prefix}/lib/systemd/systemd-journald
%{_bindir}/fractald-resolved
%{_libdir}/libnss_fractald.so.2
%config(noreplace) %{_sysconfdir}/fractald/services/fractald-resolved.service
%{_unitdir}/fractald.service
%{_libexecdir}/fractald/toolbox/*
%{_libexecdir}/fractald/rustybox-applets.txt
%doc %{_datadir}/doc/fractald/README.md
%doc %{_datadir}/doc/fractald/ARCHITECTURE.md
%doc %{_datadir}/doc/fractald/COMPATIBILITY.md
%doc %{_datadir}/doc/fractald/RUSTYBOX.md
%doc %{_datadir}/doc/fractald/RUSTYBOX-APPLETS.txt

%changelog
* Thu Sep 11 2026 Kenny Glauner <SisyphusAeolides@pm.me> - 0.1.0-1
- Initial package for the FractalD service manager.
