%global debug_package %{nil}

Name:           fractald
Version:        0.1.0
Release:        1%{?dist}
Summary:        Standalone native PID1 and Linux service supervisor

License:        GPL-2.0-or-later
URL:            https://github.com/SisyphusAeolides/FractalD
Source0:        %{url}/archive/main/%{name}-main.tar.gz

BuildRequires:  cargo
BuildRequires:  clang
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  rust

%description
FractalD is a standalone Linux service manager and native PID1. It owns the
boot process, service graph, process supervision, storage activation, package
integration, and local control socket. It does not read, execute, or generate
configuration for another init system.

%prep
%autosetup -n FractalD-main

%build
make PREFIX=/usr BINDIR=%{_bindir} LIBEXECDIR=%{_libexecdir} \
    SERVICE_DIR=/usr/lib/fractald/services INSTALL_ALPM_HOOK=0

%install
make DESTDIR=%{buildroot} PREFIX=/usr BINDIR=%{_bindir} LIBEXECDIR=%{_libexecdir} \
    SERVICE_DIR=/usr/lib/fractald/services INSTALL_ALPM_HOOK=0 install

%posttrans
if [ -x %{_bindir}/fractald-package-trigger ]; then
    %{_bindir}/fractald-package-trigger sync >/dev/null 2>&1 || :
fi

%files
%license LICENSE
%doc README.md
%{_bindir}/*
%{_libexecdir}/fractald/
/usr/lib/fractald/
/usr/lib/libnss_fractald.so*
/usr/share/doc/fractald/

%changelog
* Sun Sep 13 2026 Kenny Glauner <SisyphusAeolides@pm.me> - 0.1.0-1
- Initial RPM packaging
