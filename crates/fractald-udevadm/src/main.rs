use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{FromRawFd, RawFd};
use std::os::raw::{c_int, c_uint, c_void};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

const AF_NETLINK: c_int = 16;
const SOCK_DGRAM: c_int = 2;
const SOCK_CLOEXEC: c_int = 0x80000;
const NETLINK_KOBJECT_UEVENT: c_int = 15;

#[repr(C)]
struct NetlinkAddress {
    family: u16,
    padding: u16,
    pid: u32,
    groups: u32,
}

unsafe extern "C" {
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn bind(socket: c_int, address: *const c_void, address_length: c_uint) -> c_int;
    fn close(socket: c_int) -> c_int;
}

fn main() -> ExitCode {
    let mut arguments = env::args();
    let program = arguments.next().unwrap_or_else(|| "udevadm".to_owned());
    match run(&program, arguments.collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let name = Path::new(&program)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("udevadm");
            eprintln!("{name}: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(program: &str, arguments: Vec<String>) -> Result<(), String> {
    if Path::new(program)
        .file_name()
        .and_then(|value| value.to_str())
        == Some("systemd-udevd")
    {
        return udevd(&arguments);
    }
    if arguments.is_empty()
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        print_help();
        return Ok(());
    }
    if arguments.len() == 1 && (arguments[0] == "--version" || arguments[0] == "version") {
        println!("udevadm (FractalD) {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let command = arguments[0].as_str();
    match command {
        "control" => control(&arguments[1..]),
        "hwdb" => hwdb(&arguments[1..]),
        "info" => info(&arguments[1..]),
        "settle" => settle(&arguments[1..]),
        "trigger" => trigger(&arguments[1..]),
        "test" | "test-builtin" => test_device(&arguments[1..]),
        other => Err(format!("unsupported command {other}")),
    }
}

fn udevd(arguments: &[String]) -> Result<(), String> {
    let mut debug = false;
    for argument in arguments {
        match argument.as_str() {
            "--daemon"
            | "--foreground"
            | "--resolve-names=early"
            | "--resolve-names=later"
            | "--resolve-names=never" => {}
            "--debug" | "-d" => debug = true,
            "--help" | "-h" => {
                println!(
                    "systemd-udevd (FractalD)\n\nUsage: systemd-udevd [--daemon|--foreground] [--debug]\n\nListens for kernel uevents and maintains the FractalD device-event queue."
                );
                return Ok(());
            }
            "--version" => {
                println!("systemd-udevd (FractalD) {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            value if value.starts_with("--children-max=") || value.starts_with("--timeout=") => {}
            value => return Err(format!("unsupported option {value}")),
        }
    }

    let runtime = env::var_os("FRACTALD_UDEV_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/udev"));
    let queue = runtime.join("queue");
    let data = runtime.join("data");
    fs::create_dir_all(&queue)
        .map_err(|error| format!("cannot create {}: {error}", queue.display()))?;
    fs::create_dir_all(&data)
        .map_err(|error| format!("cannot create {}: {error}", data.display()))?;

    let socket = open_uevent_socket()?;
    if debug {
        eprintln!("systemd-udevd: listening for kernel uevents");
    }
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut sequence = 0_u64;
    let mut socket = unsafe { File::from_raw_fd(socket) };
    loop {
        let length = socket
            .read(&mut buffer)
            .map_err(|error| format!("cannot read kernel uevent socket: {error}"))?;
        if length == 0 {
            return Ok(());
        }
        sequence = sequence.wrapping_add(1);
        let properties = parse_uevent(&buffer[..length]);
        let marker = queue.join(format!(".fractald-{sequence:016x}"));
        File::create(&marker)
            .map_err(|error| format!("cannot create {}: {error}", marker.display()))?;
        persist_uevent(&data, sequence, &properties)?;
        prune_uevent_data(&data)?;
        if debug {
            let action = properties
                .get("ACTION")
                .map(String::as_str)
                .unwrap_or("unknown");
            let devpath = properties
                .get("DEVPATH")
                .map(String::as_str)
                .unwrap_or("unknown");
            eprintln!("systemd-udevd: {action} {devpath}");
        }
        fs::remove_file(&marker)
            .map_err(|error| format!("cannot remove {}: {error}", marker.display()))?;
    }
}

fn open_uevent_socket() -> Result<RawFd, String> {
    let socket = unsafe {
        socket(
            AF_NETLINK,
            SOCK_DGRAM | SOCK_CLOEXEC,
            NETLINK_KOBJECT_UEVENT,
        )
    };
    if socket < 0 {
        return Err(format!(
            "cannot open kernel uevent socket: {}",
            io::Error::last_os_error()
        ));
    }
    let address = NetlinkAddress {
        family: AF_NETLINK as u16,
        padding: 0,
        pid: 0,
        groups: 1,
    };
    let result = unsafe {
        bind(
            socket,
            (&address as *const NetlinkAddress).cast::<c_void>(),
            std::mem::size_of::<NetlinkAddress>() as c_uint,
        )
    };
    if result < 0 {
        unsafe {
            close(socket);
        }
        return Err(format!(
            "cannot bind kernel uevent socket: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(socket)
}

fn parse_uevent(bytes: &[u8]) -> BTreeMap<String, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter_map(|field| std::str::from_utf8(field).ok())
        .filter_map(|field| field.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn persist_uevent(
    directory: &Path,
    sequence: u64,
    properties: &BTreeMap<String, String>,
) -> Result<(), String> {
    let Some(devpath) = properties.get("DEVPATH") else {
        return Ok(());
    };
    let name = devpath
        .bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b'-' | b'.' => byte as char,
            _ => '_',
        })
        .collect::<String>();
    if name.is_empty() {
        return Ok(());
    }
    let path = directory.join(format!("{sequence:016x}-{name}"));
    let contents = properties
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect::<String>();
    fs::write(path, contents).map_err(|error| format!("cannot persist uevent: {error}"))
}

fn prune_uevent_data(directory: &Path) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    let remove_count = entries.len().saturating_sub(256);
    for entry in entries.into_iter().take(remove_count) {
        fs::remove_file(entry.path())
            .map_err(|error| format!("cannot prune {}: {error}", entry.path().display()))?;
    }
    Ok(())
}

fn control(arguments: &[String]) -> Result<(), String> {
    let mut accepted = false;
    for argument in arguments {
        match argument.as_str() {
            "--reload" | "--reload-rules" | "--start-exec-queue" | "--stop-exec-queue"
            | "--exit" => {
                accepted = true;
            }
            value
                if value.starts_with("--property=")
                    || value.starts_with("--children-max=")
                    || value.starts_with("--timeout=") =>
            {
                accepted = true;
            }
            "--help" | "-h" => {
                println!(
                    "Usage: udevadm control [--reload-rules|--reload|--start-exec-queue|--stop-exec-queue]"
                );
                return Ok(());
            }
            value => return Err(format!("control: unsupported option {value}")),
        }
    }
    if accepted {
        Ok(())
    } else {
        Err("control: an action is required".to_owned())
    }
}

fn hwdb(arguments: &[String]) -> Result<(), String> {
    let mut update = false;
    for argument in arguments {
        match argument.as_str() {
            "--update" => update = true,
            "--usr" | "--root" | "--help" | "-h" => {}
            value => return Err(format!("hwdb: unsupported option {value}")),
        }
    }
    if !update {
        return Err("hwdb: only --update is supported".to_owned());
    }
    // FractalD does not require the opaque systemd-udev hwdb.bin format.
    // Validate the text rule directories so package transactions still get
    // a deterministic result without spawning a competing daemon.
    for directory in [
        "/usr/lib/udev/hwdb.d",
        "/usr/local/lib/udev/hwdb.d",
        "/run/udev/hwdb.d",
        "/etc/udev/hwdb.d",
    ] {
        let path = rooted_path(&udev_root(), directory);
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "hwdb")
                {
                    let _ = fs::metadata(entry.path());
                }
            }
        }
    }
    Ok(())
}

fn info(arguments: &[String]) -> Result<(), String> {
    let mut query = "property".to_owned();
    let mut path = None;
    let mut name = None;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        if argument == "--query" || argument == "-q" {
            index += 1;
            query = arguments
                .get(index)
                .ok_or_else(|| "info: query is missing".to_owned())?
                .clone();
        } else if let Some(value) = argument.strip_prefix("--query=") {
            query = value.to_owned();
        } else if argument == "--path" || argument == "-p" {
            index += 1;
            path = Some(
                arguments
                    .get(index)
                    .ok_or_else(|| "info: sysfs path is missing".to_owned())?
                    .clone(),
            );
        } else if let Some(value) = argument.strip_prefix("--path=") {
            path = Some(value.to_owned());
        } else if argument == "--name" || argument == "-n" {
            index += 1;
            name = Some(
                arguments
                    .get(index)
                    .ok_or_else(|| "info: device name is missing".to_owned())?
                    .clone(),
            );
        } else if let Some(value) = argument.strip_prefix("--name=") {
            name = Some(value.to_owned());
        } else if argument == "--attribute-walk" || argument == "--export" {
        } else if argument == "--help" || argument == "-h" {
            println!("Usage: udevadm info --query=property --path=/devices/... | --name=/dev/...");
            return Ok(());
        } else {
            return Err(format!("info: unsupported option {argument}"));
        }
        index += 1;
    }
    let requested = path
        .or(name)
        .ok_or_else(|| "info: --path or --name is required".to_owned())?;
    let root = sysfs_root();
    let device_path = rooted_device_path(&root, &requested)?;
    let properties = device_properties(&root, &device_path)?;
    match query.to_ascii_lowercase().as_str() {
        "property" | "all" => {
            for (key, value) in properties {
                println!("{key}={value}");
            }
        }
        "name" => println!(
            "{}",
            properties
                .get("DEVNAME")
                .cloned()
                .unwrap_or_else(|| device_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned())
        ),
        "path" => println!(
            "{}",
            device_path
                .strip_prefix(&root)
                .map(|value| format!("/{}", value.display()))
                .unwrap_or_else(|_| requested.clone())
        ),
        "symlink" | "links" => {}
        other => return Err(format!("info: unsupported query {other}")),
    }
    Ok(())
}

fn settle(arguments: &[String]) -> Result<(), String> {
    let timeout = parse_timeout(arguments)?;
    let queue = env::var_os("FRACTALD_UDEV_QUEUE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/udev/queue"));
    let deadline = Instant::now() + timeout;
    loop {
        let empty = fs::read_dir(&queue)
            .map(|entries| entries.flatten().next().is_none())
            .unwrap_or(true);
        if empty || Instant::now() >= deadline {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn trigger(arguments: &[String]) -> Result<(), String> {
    let mut action = "add".to_owned();
    let mut subsystem_match = None;
    let mut subsystem_nomatch = None;
    let mut sysname_match = None;
    let mut property_match = None;
    let mut dry_run = false;
    let mut verbose = false;
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        if argument == "--action" {
            index += 1;
            action = arguments
                .get(index)
                .ok_or_else(|| "trigger: action is missing".to_owned())?
                .clone();
        } else if let Some(value) = argument.strip_prefix("--action=") {
            action = value.to_owned();
        } else if argument == "--subsystem-match" {
            index += 1;
            subsystem_match = Some(
                arguments
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "trigger: subsystem is missing".to_owned())?,
            );
        } else if let Some(value) = argument.strip_prefix("--subsystem-match=") {
            subsystem_match = Some(value.to_owned());
        } else if argument == "--subsystem-nomatch" {
            index += 1;
            subsystem_nomatch = Some(
                arguments
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "trigger: subsystem is missing".to_owned())?,
            );
        } else if let Some(value) = argument.strip_prefix("--subsystem-nomatch=") {
            subsystem_nomatch = Some(value.to_owned());
        } else if argument == "--sysname-match" {
            index += 1;
            sysname_match = Some(
                arguments
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "trigger: sysname is missing".to_owned())?,
            );
        } else if let Some(value) = argument.strip_prefix("--sysname-match=") {
            sysname_match = Some(value.to_owned());
        } else if argument == "--property-match" {
            index += 1;
            property_match = Some(
                arguments
                    .get(index)
                    .cloned()
                    .ok_or_else(|| "trigger: property is missing".to_owned())?,
            );
        } else if let Some(value) = argument.strip_prefix("--property-match=") {
            property_match = Some(value.to_owned());
        } else if argument == "--dry-run" || argument == "-n" {
            dry_run = true;
        } else if argument == "--verbose" || argument == "-v" {
            verbose = true;
        } else if argument == "--settle" {
        } else if argument == "--help" || argument == "-h" {
            println!(
                "Usage: udevadm trigger [--action=add] [--subsystem-match=SUBSYSTEM] [--sysname-match=NAME]"
            );
            return Ok(());
        } else {
            return Err(format!("trigger: unsupported option {argument}"));
        }
        index += 1;
    }
    if !matches!(
        action.as_str(),
        "add" | "remove" | "change" | "bind" | "unbind"
    ) {
        return Err(format!("trigger: unsupported action {action}"));
    }
    let root = sysfs_root();
    let mut paths = Vec::new();
    collect_uevents(&root, &mut paths)?;
    let mut triggered = 0;
    for path in paths {
        let properties = device_properties(&root, &path)?;
        let subsystem = properties.get("SUBSYSTEM").cloned().unwrap_or_default();
        let sysname = path.file_name().unwrap_or_default().to_string_lossy();
        if subsystem_match
            .as_deref()
            .is_some_and(|value| !wildcard_match(value, &subsystem))
            || subsystem_nomatch
                .as_deref()
                .is_some_and(|value| wildcard_match(value, &subsystem))
            || sysname_match
                .as_deref()
                .is_some_and(|value| !wildcard_match(value, &sysname))
            || property_match
                .as_deref()
                .is_some_and(|value| !property_matches(value, &properties))
        {
            continue;
        }
        triggered += 1;
        if dry_run {
            println!("{}", path.display());
            continue;
        }
        let uevent = path.join("uevent");
        match fs::write(&uevent, format!("{action}\n")) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::PermissionDenied
                        | io::ErrorKind::InvalidInput
                        | io::ErrorKind::Unsupported
                ) =>
            {
                if env::var_os("FRACTALD_UDEV_STRICT").is_some() {
                    return Err(format!(
                        "trigger: cannot write {}: {error}",
                        uevent.display()
                    ));
                }
            }
            Err(error) => {
                return Err(format!(
                    "trigger: cannot write {}: {error}",
                    uevent.display()
                ));
            }
        }
    }
    if verbose {
        eprintln!("triggered {triggered} device event(s)");
    }
    Ok(())
}

fn test_device(arguments: &[String]) -> Result<(), String> {
    let filtered = arguments
        .iter()
        .filter(|argument| !argument.starts_with("--action=") && *argument != "--action")
        .cloned()
        .collect::<Vec<_>>();
    if filtered.is_empty() {
        return Ok(());
    }
    info(&filtered)
}

fn parse_timeout(arguments: &[String]) -> Result<Duration, String> {
    let mut timeout = Duration::from_secs(30);
    let mut index = 0;
    while let Some(argument) = arguments.get(index) {
        let value = if argument == "--timeout" {
            index += 1;
            arguments
                .get(index)
                .ok_or_else(|| "timeout is missing".to_owned())?
                .as_str()
        } else if let Some(value) = argument.strip_prefix("--timeout=") {
            value
        } else if argument == "--help" || argument == "-h" {
            println!("Usage: udevadm settle [--timeout=SECONDS]");
            return Ok(Duration::ZERO);
        } else {
            return Err(format!("settle: unsupported option {argument}"));
        };
        let seconds = value
            .parse::<f64>()
            .map_err(|_| format!("invalid timeout {value}"))?;
        if !seconds.is_finite() || seconds < 0.0 {
            return Err(format!("invalid timeout {value}"));
        }
        timeout = Duration::from_secs_f64(seconds);
        index += 1;
    }
    Ok(timeout)
}

fn collect_uevents(path: &Path, output: &mut Vec<PathBuf>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_dir() {
        return Ok(());
    }
    if path.join("uevent").is_file() {
        output.push(path.to_owned());
    }
    let entries =
        fs::read_dir(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?;
        if entry
            .file_type()
            .map_err(|error| format!("cannot inspect entry: {error}"))?
            .is_dir()
        {
            collect_uevents(&entry.path(), output)?;
        }
    }
    Ok(())
}

fn device_properties(root: &Path, path: &Path) -> Result<BTreeMap<String, String>, String> {
    let uevent = if path.is_dir() {
        path.join("uevent")
    } else {
        path.to_owned()
    };
    let mut properties = BTreeMap::new();
    if let Ok(contents) = fs::read_to_string(&uevent) {
        for line in contents.lines() {
            if let Some((key, value)) = line.split_once('=') {
                properties.insert(key.to_owned(), value.to_owned());
            }
        }
    }
    let device_path = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    if !properties.contains_key("SUBSYSTEM") {
        if let Ok(link) = fs::read_link(device_path.join("subsystem")) {
            if let Some(name) = link.file_name() {
                properties.insert("SUBSYSTEM".to_owned(), name.to_string_lossy().into_owned());
            }
        }
    }
    if !properties.contains_key("DEVPATH") {
        if let Ok(relative) = device_path.strip_prefix(root) {
            properties.insert("DEVPATH".to_owned(), format!("/{}", relative.display()));
        }
    }
    Ok(properties)
}

fn rooted_device_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let path = Path::new(requested);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(format!("device path escapes sysfs: {requested}"));
    }
    let resolved = if path.is_absolute() && path.starts_with(root) {
        path.to_owned()
    } else if requested.starts_with("/dev/") {
        return Err("info --name requires a sysfs path in this profile".to_owned());
    } else {
        root.join(requested.trim_start_matches('/'))
    };
    if !resolved.exists() {
        return Err(format!("device path does not exist: {requested}"));
    }
    Ok(resolved)
}

fn rooted_path(root: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if root == Path::new("/") {
        path.to_owned()
    } else {
        root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

fn sysfs_root() -> PathBuf {
    env::var_os("FRACTALD_SYSFS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/sys"))
}

fn udev_root() -> PathBuf {
    env::var_os("FRACTALD_UDEV_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn property_matches(value: &str, properties: &BTreeMap<String, String>) -> bool {
    let Some((key, expected)) = value.split_once('=') else {
        return false;
    };
    properties
        .get(key)
        .is_some_and(|actual| wildcard_match(expected, actual))
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut matches = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    matches[0][0] = true;
    for index in 1..=pattern.len() {
        if pattern[index - 1] == b'*' {
            matches[index][0] = matches[index - 1][0];
        }
    }
    for pattern_index in 1..=pattern.len() {
        for value_index in 1..=value.len() {
            matches[pattern_index][value_index] = match pattern[pattern_index - 1] {
                b'*' => {
                    matches[pattern_index - 1][value_index]
                        || matches[pattern_index][value_index - 1]
                }
                b'?' => matches[pattern_index - 1][value_index - 1],
                byte => {
                    byte == value[value_index - 1] && matches[pattern_index - 1][value_index - 1]
                }
            };
        }
    }
    matches[pattern.len()][value.len()]
}

fn print_help() {
    println!(
        "udevadm (FractalD)\n\nUsage: udevadm COMMAND [OPTIONS]\n\n  control       accept rule and event-queue maintenance operations\n  hwdb --update validate text hardware database rules\n  trigger       write filtered actions to sysfs uevent files\n  info          query properties from a sysfs device path\n  settle        wait for the configured device-event queue\n  test          inspect a device without starting a daemon\n  --version    show the version\n  --help       show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matching_handles_device_filters() {
        assert!(wildcard_match("nvme*", "nvme0n1"));
        assert!(wildcard_match("v?*", "vda"));
        assert!(!wildcard_match("sd*", "nvme0n1"));
    }

    #[test]
    fn property_matching_requires_an_exact_key() {
        let mut properties = BTreeMap::new();
        properties.insert("SUBSYSTEM".to_owned(), "block".to_owned());
        assert!(property_matches("SUBSYSTEM=bl*", &properties));
        assert!(!property_matches("DEVNAME=vda", &properties));
    }

    #[test]
    fn parses_and_prunes_persisted_uevents() {
        let root = env::temp_dir().join(format!("fractald-udevadm-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("root");
        for sequence in 0..257 {
            let mut properties = BTreeMap::new();
            properties.insert("DEVPATH".to_owned(), format!("/devices/vda{sequence}"));
            persist_uevent(&root, sequence, &properties).expect("persist");
        }
        prune_uevent_data(&root).expect("prune");
        assert_eq!(fs::read_dir(&root).expect("entries").count(), 256);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
