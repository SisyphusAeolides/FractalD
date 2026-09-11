#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
root=$(mktemp -d "${TMPDIR:-/tmp}/fractald-package-check.XXXXXX")
trap 'rm -r "$root"' EXIT INT TERM

rpm_checked=0
deb_checked=0

if command -v rpmbuild >/dev/null 2>&1 && command -v rpm >/dev/null 2>&1; then
    rpm_top="$root/rpm"
    mkdir -p "$rpm_top/BUILD" "$rpm_top/BUILDROOT" "$rpm_top/RPMS" \
        "$rpm_top/SOURCES" "$rpm_top/SPECS" "$rpm_top/SRPMS"
    archive_root="$root/archive/fractald-0.1.0"
    mkdir -p "$archive_root"
    tar --exclude=./target --exclude=./.git -C "$project_dir" -cf - . | tar -C "$archive_root" -xf -
    tar -C "$root/archive" -czf "$rpm_top/SOURCES/fractald-0.1.0.tar.gz" fractald-0.1.0
    cp "$project_dir/packaging/fedora/fractald.spec" "$rpm_top/SPECS/fractald.spec"
    rpmbuild -bb "$rpm_top/SPECS/fractald.spec" --define "_topdir $rpm_top"
    rpm_file=
    for candidate in "$rpm_top"/RPMS/*/*.rpm "$rpm_top"/RPMS/*.rpm; do
        [ -f "$candidate" ] || continue
        if rpm -qpl "$candidate" | grep -qx '/usr/bin/fractald'; then
            rpm_file=$candidate
            break
        fi
    done
    test -n "$rpm_file"
    rpm_contents=$(rpm -qpl "$rpm_file")
    case "$rpm_contents" in
        *"/usr/bin/fractald"*) ;;
        *) echo "RPM is missing fractald" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/resolvectl"*) ;;
        *) echo "RPM is missing resolvectl" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-escape"*) ;;
        *) echo "RPM is missing systemd-escape" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-sysusers"*) ;;
        *) echo "RPM is missing systemd-sysusers" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-sysctl"*) ;;
        *) echo "RPM is missing systemd-sysctl" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/lib/systemd/systemd-sysctl"*) ;;
        *) echo "RPM is missing the systemd-sysctl compatibility path" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-update-helper"*) ;;
        *) echo "RPM is missing systemd-update-helper" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/lib/systemd/systemd-update-helper"*) ;;
        *) echo "RPM is missing the systemd-update-helper compatibility path" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-machine-id-setup"*) ;;
        *) echo "RPM is missing systemd-machine-id-setup" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-detect-virt"*) ;;
        *) echo "RPM is missing systemd-detect-virt" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/systemd-analyze"*) ;;
        *) echo "RPM is missing systemd-analyze" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/udevadm"*) ;;
        *) echo "RPM is missing udevadm" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/lib/systemd/systemd-udevd"*) ;;
        *) echo "RPM is missing the native udev daemon endpoint" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/lib/systemd/systemd-journald"*) ;;
        *) echo "RPM is missing the native journal daemon endpoint" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/kernel-install"*) ;;
        *) echo "RPM is missing kernel-install" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/installkernel"*) ;;
        *) echo "RPM is missing installkernel compatibility path" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/bin/rustybox"*) ;;
        *) echo "RPM is missing rustybox" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/libexec/fractald/toolbox/mount"*) ;;
        *) echo "RPM is missing the RustyBox mount applet" >&2; exit 1 ;;
    esac
    case "$rpm_contents" in
        *"/usr/lib/systemd/system/fractald.service"*) ;;
        *) echo "RPM is missing the manager unit" >&2; exit 1 ;;
    esac
    rpm_checked=1
fi

if command -v dpkg-deb >/dev/null 2>&1; then
    deb_root="$root/deb"
    mkdir -p "$deb_root/DEBIAN"
    make -C "$project_dir" install \
        PREFIX=/usr BINDIR=/usr/bin NSSLIBDIR=/usr/lib \
        DATADIR=/usr/share SYSCONFDIR=/etc SYSTEMDUNITDIR=/usr/lib/systemd/system \
        DESTDIR="$deb_root"
    architecture=all
    if command -v dpkg >/dev/null 2>&1; then
        architecture=$(dpkg --print-architecture)
    fi
    cat >"$deb_root/DEBIAN/control" <<EOF
Package: fractald
Version: 0.1.0-1
Section: admin
Priority: optional
Architecture: $architecture
Maintainer: FractalD Maintainers <maintainers@example.invalid>
Description: FractalD service manager and compatibility layer
 FractalD provides a supervised Linux service manager with package-facing
 compatibility tools and an optional separately supervised resolver.
EOF
    dpkg-deb --build "$deb_root" "$root/fractald.deb" >/dev/null
    deb_contents=$(dpkg-deb --contents "$root/fractald.deb")
    case "$deb_contents" in
        *"/usr/bin/fractald"*) ;;
        *) echo "DEB is missing fractald" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/resolvectl"*) ;;
        *) echo "DEB is missing resolvectl" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-escape"*) ;;
        *) echo "DEB is missing systemd-escape" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-sysusers"*) ;;
        *) echo "DEB is missing systemd-sysusers" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-sysctl"*) ;;
        *) echo "DEB is missing systemd-sysctl" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/lib/systemd/systemd-sysctl"*) ;;
        *) echo "DEB is missing the systemd-sysctl compatibility path" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-update-helper"*) ;;
        *) echo "DEB is missing systemd-update-helper" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/lib/systemd/systemd-update-helper"*) ;;
        *) echo "DEB is missing the systemd-update-helper compatibility path" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-machine-id-setup"*) ;;
        *) echo "DEB is missing systemd-machine-id-setup" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-detect-virt"*) ;;
        *) echo "DEB is missing systemd-detect-virt" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/systemd-analyze"*) ;;
        *) echo "DEB is missing systemd-analyze" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/udevadm"*) ;;
        *) echo "DEB is missing udevadm" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/lib/systemd/systemd-udevd"*) ;;
        *) echo "DEB is missing the native udev daemon endpoint" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/lib/systemd/systemd-journald"*) ;;
        *) echo "DEB is missing the native journal daemon endpoint" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/kernel-install"*) ;;
        *) echo "DEB is missing kernel-install" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/installkernel"*) ;;
        *) echo "DEB is missing installkernel compatibility path" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/bin/rustybox"*) ;;
        *) echo "DEB is missing rustybox" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/libexec/fractald/toolbox/mount"*) ;;
        *) echo "DEB is missing the RustyBox mount applet" >&2; exit 1 ;;
    esac
    case "$deb_contents" in
        *"/usr/lib/systemd/system/fractald.service"*) ;;
        *) echo "DEB is missing the manager unit" >&2; exit 1 ;;
    esac
    deb_checked=1
fi

if [ "$rpm_checked" -eq 0 ] && [ "$deb_checked" -eq 0 ]; then
    echo "no supported package builder is installed" >&2
    exit 1
fi

echo "package manager compatibility: rpm=$rpm_checked deb=$deb_checked"
