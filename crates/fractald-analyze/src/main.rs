use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

const CONFIG_DIRECTORIES: &[&str] = &[
    "/usr/lib/systemd",
    "/usr/local/lib/systemd",
    "/run/systemd",
    "/etc/systemd",
];

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-analyze: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let mut tldr = false;
    let mut root = None;
    let mut command = None;
    let mut operands = Vec::new();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" | "help" => {
                print_help();
                return Ok(());
            }
            "--version" | "version" => {
                println!("systemd-analyze (FractalD) {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--tldr" => tldr = true,
            "--no-pager" | "--no-legend" | "--quiet" | "--system" | "--user" => {}
            "--offline" | "--generators" | "--man" | "--recursive-errors" => {}
            "--root" => {
                root = Some(PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--root requires a path".to_owned())?,
                ));
            }
            value if value.starts_with("--root=") => {
                root = Some(PathBuf::from(value.trim_start_matches("--root=")));
            }
            value if value.starts_with('-') => {
                return Err(format!("unsupported option {value}"));
            }
            value if command.is_none() => command = Some(value.to_owned()),
            value => operands.push(value.to_owned()),
        }
    }

    match command.as_deref() {
        Some("cat-config") => cat_config(&operands, tldr, root.as_deref()),
        Some("unit-paths") => unit_paths(),
        Some("verify") => verify(&operands, root.as_deref()),
        Some(other) => Err(format!("unsupported command {other}")),
        None => {
            print_help();
            Ok(())
        }
    }
}

fn cat_config(names: &[String], tldr: bool, root_override: Option<&Path>) -> Result<(), String> {
    if names.is_empty() {
        return Err("cat-config requires at least one configuration name".to_owned());
    }
    let root = root_override
        .map(Path::to_owned)
        .or_else(|| env::var_os("FRACTALD_ANALYZE_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut found = false;
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for name in names {
        let relative = normalize_config_name(name)?;
        for directory in CONFIG_DIRECTORIES {
            let base = rooted_path(&root, directory);
            let main = base.join(&relative);
            if main.is_file() {
                print_config_file(&mut output, &main, tldr)?;
                found = true;
            }
            let drop_in_directory = base.join(format!("{}.d", relative.display()));
            for path in configuration_files(&drop_in_directory)? {
                print_config_file(&mut output, &path, tldr)?;
                found = true;
            }
        }
    }
    if found {
        Ok(())
    } else {
        Err(format!("configuration not found: {}", names.join(" ")))
    }
}

fn verify(names: &[String], root_override: Option<&Path>) -> Result<(), String> {
    if names.is_empty() {
        return Err("verify requires at least one unit name".to_owned());
    }
    let root = root_override
        .map(Path::to_owned)
        .or_else(|| env::var_os("FRACTALD_ANALYZE_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"));
    let directories = [
        "/etc/systemd/system",
        "/run/systemd/system",
        "/usr/local/lib/systemd/system",
        "/usr/lib/systemd/system",
        "/lib/systemd/system",
    ];
    let mut failures = Vec::new();
    for name in names {
        let path = resolve_unit_path(&root, &directories, name);
        let Some(path) = path else {
            failures.push(format!("{name}: unit file was not found"));
            continue;
        };
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                failures.push(format!("{}: cannot read: {error}", path.display()));
                continue;
            }
        };
        let unit = match fractald_config::UnitFile::parse(&source) {
            Ok(unit) => unit,
            Err(error) => {
                failures.push(format!("{}: {error}", path.display()));
                continue;
            }
        };
        if let Err(error) = validate_unit(&unit, name) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn resolve_unit_path(root: &Path, directories: &[&str], name: &str) -> Option<PathBuf> {
    let requested = Path::new(name);
    if requested.is_absolute() || name.contains('/') {
        let path = if requested.is_absolute() {
            rooted_path(root, name)
        } else {
            root.join(requested)
        };
        return path.is_file().then_some(path);
    }
    directories
        .iter()
        .map(|directory| rooted_path(root, directory).join(name))
        .find(|path| path.is_file())
}

fn validate_unit(unit: &fractald_config::UnitFile, name: &str) -> Result<(), String> {
    match name.rsplit_once('.').map(|(_, suffix)| suffix) {
        Some("service") => {
            if unit.has_section("Service") {
                unit.to_service_spec(name.to_owned())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            } else if unit.has_section("Unit") {
                unit.to_action_spec(name.to_owned())
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            } else {
                Err("unit has no recognized section".to_owned())
            }
        }
        Some("target") => unit
            .to_target_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("socket") => unit
            .to_socket_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("timer") => unit
            .to_timer_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("path") => unit
            .to_path_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("mount") => unit
            .to_mount_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("swap") => unit
            .to_swap_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("automount") => unit
            .to_automount_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("slice") => unit
            .to_slice_spec(name.to_owned())
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Some("device") => Ok(()),
        Some("scope") | Some("busname") => Ok(()),
        _ => Err("unsupported unit suffix".to_owned()),
    }
}

fn normalize_config_name(name: &str) -> Result<PathBuf, String> {
    let name = name.strip_prefix("systemd/").unwrap_or(name);
    let path = Path::new(name);
    if name.is_empty() || path.is_absolute() {
        return Err(format!("invalid configuration name {name:?}"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "configuration name escapes systemd directories: {name:?}"
        ));
    }
    Ok(path.to_owned())
}

fn configuration_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!("cannot read {}: {error}", directory.display()));
        }
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|extension| extension == "conf")
            && path.is_file()
        {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

fn print_config_file(output: &mut impl Write, path: &Path, tldr: bool) -> Result<(), String> {
    let contents = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    if !tldr {
        writeln!(output, "# {}", path.display())
            .map_err(|error| format!("cannot write configuration: {error}"))?;
    }
    if tldr {
        for line in contents.lines() {
            let trimmed = line.trim_start();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with(';') {
                writeln!(output, "{line}")
                    .map_err(|error| format!("cannot write configuration: {error}"))?;
            }
        }
    } else {
        output
            .write_all(contents.as_bytes())
            .map_err(|error| format!("cannot write configuration: {error}"))?;
        if !contents.ends_with('\n') {
            writeln!(output).map_err(|error| format!("cannot write configuration: {error}"))?;
        }
    }
    Ok(())
}

fn unit_paths() -> Result<(), String> {
    for path in [
        "/etc/systemd/system",
        "/run/systemd/system",
        "/usr/local/lib/systemd/system",
        "/usr/lib/systemd/system",
        "/lib/systemd/system",
    ] {
        println!("{path}");
    }
    Ok(())
}

fn rooted_path(root: &Path, absolute_path: &str) -> PathBuf {
    if root == Path::new("/") {
        PathBuf::from(absolute_path)
    } else {
        root.join(absolute_path.trim_start_matches('/'))
    }
}

fn print_help() {
    println!(
        "systemd-analyze (FractalD)\n\nUsage: systemd-analyze COMMAND [OPTIONS]\n\n  cat-config NAME...  print effective systemd configuration files\n  unit-paths          print the system unit search path\n  verify UNIT...      validate unit syntax and typed directives\n  --root=PATH         validate or inspect below an alternate root\n  --tldr              omit comments and blank lines from cat-config\n  --no-pager          accept the standard noninteractive option\n  --version           show the version\n  --help              show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("fractald-analyze-{stamp}"));
        fs::create_dir_all(&root).expect("root");
        root
    }

    #[test]
    fn normalizes_systemd_prefixed_names() {
        assert_eq!(
            normalize_config_name("systemd/resolved.conf").expect("name"),
            PathBuf::from("resolved.conf")
        );
        assert!(normalize_config_name("../resolved.conf").is_err());
    }

    #[test]
    fn sorts_drop_ins() {
        let root = test_root();
        let directory = root.join("drop-ins");
        fs::create_dir_all(&directory).expect("directory");
        fs::write(directory.join("20-later.conf"), "").expect("later");
        fs::write(directory.join("10-first.conf"), "").expect("first");
        assert_eq!(
            configuration_files(&directory).expect("files"),
            vec![
                directory.join("10-first.conf"),
                directory.join("20-later.conf")
            ]
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
