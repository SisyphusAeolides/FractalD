use std::env;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::ExitCode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Any,
    Container,
    Vm,
    Chroot,
    PrivateUsers,
    Cvm,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("systemd-detect-virt: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut mode = Mode::Any;
    let mut quiet = false;
    let mut list = false;
    let mut list_cvm = false;
    let mut args = env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(0);
            }
            "--version" => {
                println!(
                    "systemd-detect-virt (FractalD) {}",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(0);
            }
            "--quiet" | "-q" => quiet = true,
            "--container" | "-c" => mode = select_mode(mode, Mode::Container)?,
            "--vm" | "-v" => mode = select_mode(mode, Mode::Vm)?,
            "--chroot" | "-r" => mode = select_mode(mode, Mode::Chroot)?,
            "--private-users" => mode = select_mode(mode, Mode::PrivateUsers)?,
            "--cvm" => mode = select_mode(mode, Mode::Cvm)?,
            "--list" => list = true,
            "--list-cvm" => list_cvm = true,
            other => return Err(format!("unsupported option {other}")),
        }
    }

    if list {
        for value in known_virtualization_types() {
            println!("{value}");
        }
        return Ok(0);
    }
    if list_cvm {
        for value in ["sev", "sev-snp", "tdx", "cca"] {
            println!("{value}");
        }
        return Ok(0);
    }

    let detection = detect();
    let (matched, label) = match mode {
        Mode::Any => detection.container.clone().map_or_else(
            || (detection.vm.is_some(), detection.vm),
            |value| (true, Some(value)),
        ),
        Mode::Container => (detection.container.is_some(), detection.container),
        Mode::Vm => (detection.vm.is_some(), detection.vm),
        Mode::Chroot => (detection.chroot, Some("chroot".to_owned())),
        Mode::PrivateUsers => (detection.private_users, Some("private-users".to_owned())),
        Mode::Cvm => (detection.cvm, detection.cvm_type),
    };
    if !quiet {
        println!("{}", label.as_deref().unwrap_or("none"));
    }
    Ok(u8::from(!matched))
}

fn select_mode(current: Mode, next: Mode) -> Result<Mode, String> {
    if current != Mode::Any {
        return Err("detection modes are mutually exclusive".to_owned());
    }
    Ok(next)
}

#[derive(Debug, Default)]
struct Detection {
    container: Option<String>,
    vm: Option<String>,
    chroot: bool,
    private_users: bool,
    cvm: bool,
    cvm_type: Option<String>,
}

fn detect() -> Detection {
    Detection {
        container: detect_container(),
        vm: detect_vm(),
        chroot: detect_chroot(),
        private_users: detect_private_users(),
        cvm: detect_cvm().is_some(),
        cvm_type: detect_cvm(),
    }
}

fn detect_container() -> Option<String> {
    if let Ok(value) = env::var("container") {
        if !value.is_empty() {
            return Some(value);
        }
    }
    for path in ["/run/systemd/container", "/run/oci/container"] {
        if let Ok(value) = fs::read_to_string(path) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    if let Ok(value) = fs::read("/proc/1/environ") {
        if let Some(value) = nul_environment_value(&value, "container") {
            return Some(value);
        }
    }
    if Path::new("/.dockerenv").exists() {
        return Some("docker".to_owned());
    }
    if Path::new("/.containerenv").exists() {
        return Some("podman".to_owned());
    }
    None
}

fn detect_vm() -> Option<String> {
    let mut description = String::new();
    for path in [
        "/sys/class/dmi/id/product_name",
        "/sys/class/dmi/id/sys_vendor",
        "/sys/class/dmi/id/board_vendor",
        "/sys/hypervisor/type",
    ] {
        if let Ok(value) = fs::read_to_string(path) {
            description.push(' ');
            description.push_str(&value.to_ascii_lowercase());
        }
    }
    let candidates = [
        ("vmware", "vmware"),
        ("virtualbox", "oracle"),
        ("innotek", "oracle"),
        ("qemu", "qemu"),
        ("kvm", "kvm"),
        ("microsoft", "microsoft"),
        ("virtual machine", "microsoft"),
        ("xen", "xen"),
        ("bhyve", "bhyve"),
        ("parallels", "parallels"),
        ("amazon", "amazon"),
        ("google", "google"),
        ("bochs", "bochs"),
    ];
    candidates
        .iter()
        .find_map(|(needle, value)| description.contains(needle).then(|| (*value).to_owned()))
        .or_else(|| {
            fs::read_to_string("/proc/cpuinfo")
                .ok()
                .filter(|value| value.to_ascii_lowercase().contains(" hypervisor"))
                .map(|_| "vm".to_owned())
        })
}

fn detect_chroot() -> bool {
    let Ok(root) = fs::metadata("/") else {
        return false;
    };
    let Ok(pid_root) = fs::metadata("/proc/1/root") else {
        return false;
    };
    root.dev() != pid_root.dev() || root.ino() != pid_root.ino()
}

fn detect_private_users() -> bool {
    let Ok(value) = fs::read_to_string("/proc/self/uid_map") else {
        return false;
    };
    let Some((inside, outside, length)) = value.lines().next().and_then(parse_id_map) else {
        return false;
    };
    inside != 0 || outside != 0 || length < u64::from(u32::MAX)
}

fn parse_id_map(line: &str) -> Option<(u64, u64, u64)> {
    let mut fields = line.split_whitespace();
    Some((
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
        fields.next()?.parse().ok()?,
    ))
}

fn detect_cvm() -> Option<String> {
    [
        ("/sys/firmware/tdx_guest", "tdx"),
        ("/sys/firmware/sev", "sev"),
        ("/sys/firmware/sev-snp", "sev-snp"),
    ]
    .iter()
    .find_map(|(path, value)| Path::new(path).exists().then(|| (*value).to_owned()))
}

fn nul_environment_value(environment: &[u8], name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    environment
        .split(|byte| *byte == 0)
        .find_map(|value| value.strip_prefix(prefix.as_bytes()))
        .and_then(|value| std::str::from_utf8(value).ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn known_virtualization_types() -> [&'static str; 13] {
    [
        "kvm",
        "qemu",
        "vmware",
        "microsoft",
        "oracle",
        "xen",
        "bhyve",
        "amazon",
        "google",
        "bochs",
        "parallels",
        "docker",
        "podman",
    ]
}

fn print_help() {
    println!(
        "systemd-detect-virt (FractalD)\n\nUsage: systemd-detect-virt [OPTIONS]\n\n  --container, -c    detect containers only\n  --vm, -v           detect virtual machines only\n  --chroot, -r       detect chroot execution\n  --private-users     detect a private user namespace\n  --cvm              detect confidential virtual machines\n  --quiet, -q         suppress output\n  --list              list known virtualization types\n  --list-cvm          list confidential VM types\n  --version           show the version\n  --help              show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_user_namespace_maps() {
        assert_eq!(parse_id_map("0 0 4294967295"), Some((0, 0, 4_294_967_295)));
        assert_eq!(parse_id_map("0 100000 1"), Some((0, 100000, 1)));
        assert_eq!(parse_id_map("bad"), None);
    }

    #[test]
    fn finds_container_environment_values() {
        assert_eq!(
            nul_environment_value(b"PATH=/bin\0container=podman\0", "container"),
            Some("podman".to_owned())
        );
        assert_eq!(nul_environment_value(b"PATH=/bin\0", "container"), None);
    }
}
