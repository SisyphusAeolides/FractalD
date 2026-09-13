use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use fractald_config::parse_service_file;
use fractald_control::{
    Request, Response, RuntimePaths, StatePaths, remove_stale_socket, service_directories,
};

fn main() -> std::process::ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fractald-package-trigger: {error}");
            std::process::ExitCode::from(1)
        }
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let operation = args.next().unwrap_or_else(|| "sync".to_owned());
    if args.next().is_some() {
        return Err("one operation is accepted: sync, verify, or reload".to_owned());
    }
    match operation.as_str() {
        "sync" => sync_packages(true),
        "verify" => sync_packages(false),
        "reload" => request_reload(),
        "help" | "--help" => {
            println!("usage: fractald-package-trigger [sync|verify|reload]");
            Ok(())
        }
        other => Err(format!("unsupported operation {other}")),
    }
}

fn sync_packages(update_state: bool) -> Result<(), String> {
    let services = discover_package_services()?;
    let profiles = validate_package_services(&services)?;
    if !update_state {
        println!(
            "verified {} native service descriptor(s)",
            service_count(&services)
        );
        return Ok(());
    }

    let state = StatePaths::from_environment();
    state
        .ensure_directory()
        .map_err(|error| format!("cannot prepare FractalD state: {error}"))?;
    let previous = read_index(&state)?;
    let active_profile = selected_profile();
    let current = profiles.keys().cloned().collect::<BTreeSet<_>>();
    let mut profile_services = BTreeSet::new();
    for stale in previous.keys().filter(|name| !current.contains(*name)) {
        state
            .disable(stale)
            .map_err(|error| format!("cannot disable removed service {stale}: {error}"))?;
    }
    for (service, service_profiles) in &profiles {
        if service_profiles.contains(&active_profile) {
            profile_services.insert(service.clone());
            state
                .enable(service)
                .map_err(|error| format!("cannot enable {service}: {error}"))?;
        }
    }
    for service in previous
        .keys()
        .filter(|name| !profile_services.contains(*name))
    {
        state.disable(service).map_err(|error| {
            format!("cannot disable inactive package service {service}: {error}")
        })?;
    }
    write_index(&state, &services)?;
    request_reload()
}

fn validate_package_services(
    services: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let mut profiles = BTreeMap::new();
    let mut owners = BTreeMap::new();
    for (package, paths) in services {
        for path in paths {
            let spec = parse_service_file(path).map_err(|error| {
                format!("cannot validate {} from {package}: {error}", path.display())
            })?;
            let owner = format!("{package}:{}", path.display());
            if let Some(previous) = owners.insert(spec.name.clone(), owner.clone()) {
                return Err(format!(
                    "service {} is declared more than once ({previous} and {owner})",
                    spec.name
                ));
            }
            profiles.insert(spec.name, spec.profiles);
        }
    }
    Ok(profiles)
}

fn selected_profile() -> String {
    if let Some(profile) = env::var("FRACTALD_BOOT_PROFILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return profile;
    }
    let path = env::var_os("FRACTALD_BOOT_PROFILE_FILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/fractald/boot.conf"));
    if let Ok(source) = fs::read_to_string(path) {
        if let Some(profile) = source.lines().find_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once('=')?;
            (key.trim() == "profile" && !value.trim().is_empty()).then(|| value.trim().to_owned())
        }) {
            return profile;
        }
    }
    "boot".to_owned()
}

fn discover_package_services() -> Result<BTreeMap<String, Vec<PathBuf>>, String> {
    if let Ok(mode) = env::var("FRACTALD_PACKAGE_DISCOVERY") {
        match mode.as_str() {
            "native" | "filesystem" => return discover_native_services(),
            "package" | "pacman" => {}
            other => {
                return Err(format!(
                    "unsupported FRACTALD_PACKAGE_DISCOVERY mode {other}"
                ));
            }
        }
    }
    let explicit_database = env::var_os("FRACTALD_PACKAGE_DB");
    let database = explicit_database
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/pacman/local"));
    let package_root = env::var_os("FRACTALD_PACKAGE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let entries = match fs::read_dir(&database) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound && explicit_database.is_none() => {
            return discover_native_services();
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(format!(
                "cannot read package database {}: {error}",
                database.display()
            ));
        }
    };
    let mut result = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate package database: {error}"))?;
        let package = entry.file_name().to_string_lossy().into_owned();
        let files = entry.path().join("files");
        let source = match fs::read_to_string(&files) {
            Ok(source) => source,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read {}: {error}", files.display())),
        };
        let mut in_files = false;
        for line in source.lines() {
            if line == "%FILES%" {
                in_files = true;
                continue;
            }
            if !in_files || line.starts_with('%') || !line.ends_with(".svc") {
                continue;
            }
            let relative = line.trim_start_matches('/').trim_start_matches("./");
            if !relative.starts_with("usr/lib/fractald/services/")
                && !relative.starts_with("usr/libexec/fractald/services/")
                && !relative.starts_with("usr/local/lib/fractald/services/")
                && !relative.starts_with("usr/local/libexec/fractald/services/")
                && !relative.starts_with("etc/fractald/services/")
            {
                continue;
            }
            let path = package_root.join(relative);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_file() => {
                    let paths = result.entry(package.clone()).or_insert_with(Vec::new);
                    if !paths.iter().any(|existing| existing == &path) {
                        paths.push(path);
                    }
                }
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "package descriptor {} is a symlink",
                        path.display()
                    ));
                }
                Ok(_) => {
                    return Err(format!(
                        "package descriptor {} is not a regular file",
                        path.display()
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "cannot inspect package descriptor {}: {error}",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(result)
}

fn discover_native_services() -> Result<BTreeMap<String, Vec<PathBuf>>, String> {
    let mut selected = BTreeMap::new();
    for directory in service_directories() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read service directory {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
                .path();
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(".svc") else {
                continue;
            };
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
            if metadata.file_type().is_symlink() {
                return Err(format!("service descriptor {} is a symlink", path.display()));
            }
            if !metadata.file_type().is_file() {
                return Err(format!(
                    "service descriptor {} is not a regular file",
                    path.display()
                ));
            }
            // Directory order is the native overlay order; a later directory
            // replaces an earlier descriptor with the same service name.
            selected.insert(name.to_owned(), path);
        }
    }

    if selected.is_empty() {
        return Ok(BTreeMap::new());
    }
    Ok(BTreeMap::from([(
        "filesystem".to_owned(),
        selected.into_values().collect(),
    )]))
}

fn service_count(services: &BTreeMap<String, Vec<PathBuf>>) -> usize {
    services.values().map(Vec::len).sum()
}

fn index_path(state: &StatePaths) -> PathBuf {
    state.directory.join("packages.index")
}

fn read_index(state: &StatePaths) -> Result<BTreeMap<String, String>, String> {
    let path = index_path(state);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let mut index = BTreeMap::new();
    for line in source.lines().filter(|line| !line.is_empty()) {
        let (service, package) = line
            .split_once('\t')
            .ok_or_else(|| format!("invalid package index line in {}", path.display()))?;
        index.insert(service.to_owned(), package.to_owned());
    }
    Ok(index)
}

fn write_index(
    state: &StatePaths,
    services: &BTreeMap<String, Vec<PathBuf>>,
) -> Result<(), String> {
    let path = index_path(state);
    let temporary = path.with_extension(format!("tmp-{}", process_stamp()));
    let mut contents = String::new();
    for (package, paths) in services {
        for path in paths {
            let Some(service) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            contents.push_str(service);
            contents.push('\t');
            contents.push_str(package);
            contents.push('\n');
        }
    }
    fs::write(&temporary, contents)
        .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .map_err(|error| format!("cannot install {}: {error}", path.display()))
}

fn request_reload() -> Result<(), String> {
    let runtime = RuntimePaths::from_environment();
    if !runtime.socket.exists() {
        return Ok(());
    }
    match runtime.request(Request::Reload) {
        Ok(Response::Reloaded) => Ok(()),
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected reload response: {response:?}")),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::NotFound
            ) =>
        {
            remove_stale_socket(&runtime.socket).map_err(|remove_error| {
                format!("cannot remove stale FractalD socket: {remove_error}")
            })?;
            Ok(())
        }
        Err(error) => Err(format!("cannot contact FractalD: {error}")),
    }
}

fn process_stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}
