use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

const CONFIG_DIRECTORIES: &[&str] = &[
    "/usr/lib/fractald",
    "/usr/local/lib/fractald",
    "/run/fractald",
    "/etc/fractald",
];
const SERVICE_DIRECTORIES: &[&str] = &[
    "/etc/fractald/services",
    "/run/fractald/services",
    "/usr/local/lib/fractald/services",
    "/usr/lib/fractald/services",
];

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fractald-analyze: {error}");
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
                println!("fractald-analyze {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--tldr" => tldr = true,
            "--no-pager" | "--no-legend" | "--quiet" => {}
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
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            value if command.is_none() => command = Some(value.to_owned()),
            value => operands.push(value.to_owned()),
        }
    }

    match command.as_deref() {
        Some("cat-config") => cat_config(&operands, tldr, root.as_deref()),
        Some("service-paths") | Some("paths") => service_paths(root.as_deref()),
        Some("verify") => verify(&operands, root.as_deref()),
        Some(other) => Err(format!("unsupported command {other}")),
        None => {
            print_help();
            Ok(())
        }
    }
}

fn selected_root(root_override: Option<&Path>) -> PathBuf {
    root_override
        .map(Path::to_owned)
        .or_else(|| env::var_os("FRACTALD_ANALYZE_ROOT").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn cat_config(names: &[String], tldr: bool, root_override: Option<&Path>) -> Result<(), String> {
    if names.is_empty() {
        return Err("cat-config requires at least one configuration name".to_owned());
    }
    let root = selected_root(root_override);
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
            let drop_ins = base.join(format!("{}.d", relative.display()));
            for path in configuration_files(&drop_ins)? {
                print_config_file(&mut output, &path, tldr)?;
                found = true;
            }
        }
    }
    found
        .then_some(())
        .ok_or_else(|| format!("configuration not found: {}", names.join(" ")))
}

fn verify(names: &[String], root_override: Option<&Path>) -> Result<(), String> {
    if names.is_empty() {
        return Err("verify requires at least one service name".to_owned());
    }
    let root = selected_root(root_override);
    let mut failures = Vec::new();
    for requested in names {
        let name = requested.strip_suffix(".svc").unwrap_or(requested);
        let file_name = format!("{name}.svc");
        let path = if requested.contains('/') || Path::new(requested).is_absolute() {
            rooted_path(&root, requested)
        } else {
            SERVICE_DIRECTORIES
                .iter()
                .map(|directory| rooted_path(&root, directory).join(&file_name))
                .find(|path| path.is_file())
                .unwrap_or_else(|| rooted_path(&root, "/etc/fractald/services").join(file_name))
        };
        if let Err(error) = fractald_config::parse_service_file(&path) {
            failures.push(format!("{}: {error}", path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn normalize_config_name(name: &str) -> Result<PathBuf, String> {
    let name = name.strip_prefix("fractald/").unwrap_or(name);
    let path = Path::new(name);
    if name.is_empty() || path.is_absolute() {
        return Err(format!("invalid configuration name {name:?}"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(format!(
            "configuration name escapes FractalD directories: {name:?}"
        ));
    }
    Ok(path.to_owned())
}

fn configuration_files(directory: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
            .path();
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
    for line in contents.lines() {
        let trimmed = line.trim_start();
        if !tldr || (!trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with(';'))
        {
            writeln!(output, "{line}")
                .map_err(|error| format!("cannot write configuration: {error}"))?;
        }
    }
    Ok(())
}

fn service_paths(root_override: Option<&Path>) -> Result<(), String> {
    let root = selected_root(root_override);
    for path in SERVICE_DIRECTORIES {
        println!("{}", rooted_path(&root, path).display());
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
        "fractald-analyze (FractalD)\n\nUsage: fractald-analyze COMMAND [OPTIONS]\n\n  cat-config NAME...  print effective FractalD configuration\n  service-paths       print native service search paths\n  verify SERVICE...   validate native .svc descriptors\n  --root=PATH         inspect below an alternate root\n  --tldr              omit comments and blank lines\n  --version           show the version\n  --help              show this help"
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
    fn normalizes_fractald_prefixed_names() {
        assert_eq!(
            normalize_config_name("fractald/manager.conf").expect("name"),
            PathBuf::from("manager.conf")
        );
        assert!(normalize_config_name("../manager.conf").is_err());
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
