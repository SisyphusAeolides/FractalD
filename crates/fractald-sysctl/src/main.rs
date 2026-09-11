use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-sysctl: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let options = parse_args(env::args_os().skip(1))?;
    if options.show_help {
        print_help();
        return Ok(());
    }
    if options.show_version {
        println!("systemd-sysctl (FractalD) {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let sources = load_sources(&options)?;
    match options.action {
        Action::CatConfig | Action::Tldr => print_sources(&sources, options.action),
        Action::Apply => apply_sources(&options, &sources),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Apply,
    CatConfig,
    Tldr,
}

#[derive(Debug)]
struct Options {
    root: PathBuf,
    prefixes: Vec<String>,
    files: Vec<String>,
    strict: bool,
    dry_run: bool,
    action: Action,
    show_help: bool,
    show_version: bool,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut options = Options {
        root: PathBuf::from("/"),
        prefixes: Vec::new(),
        files: Vec::new(),
        strict: false,
        dry_run: false,
        action: Action::Apply,
        show_help: false,
        show_version: false,
    };
    let mut arguments = args.into_iter();
    let mut after_separator = false;
    while let Some(raw_value) = arguments.next() {
        let value = raw_value
            .into_string()
            .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
        if after_separator {
            options.files.push(value);
            continue;
        }
        match value.as_str() {
            "--" => after_separator = true,
            "--root" => {
                options.root = PathBuf::from(
                    arguments
                        .next()
                        .ok_or_else(|| "--root requires a path".to_owned())?
                        .into_string()
                        .map_err(|_| "--root path must be valid UTF-8".to_owned())?,
                );
            }
            value if value.starts_with("--root=") => options.root = PathBuf::from(&value[7..]),
            "--prefix" => options.prefixes.push(
                arguments
                    .next()
                    .ok_or_else(|| "--prefix requires a value".to_owned())?
                    .into_string()
                    .map_err(|_| "--prefix must be valid UTF-8".to_owned())?,
            ),
            value if value.starts_with("--prefix=") => options.prefixes.push(value[9..].to_owned()),
            "--strict" => options.strict = true,
            "--dry-run" => options.dry_run = true,
            "--cat-config" => options.action = Action::CatConfig,
            "--tldr" => options.action = Action::Tldr,
            "--system" | "--no-pager" => {}
            "--help" | "-h" => options.show_help = true,
            "--version" => options.show_version = true,
            "-" => options.files.push("-".to_owned()),
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            value => options.files.push(value.to_owned()),
        }
    }
    Ok(options)
}

#[derive(Clone, Debug)]
struct Source {
    label: String,
    text: String,
}

fn load_sources(options: &Options) -> Result<Vec<Source>, String> {
    let directories = config_directories(&options.root);
    if options.files.is_empty() {
        return discover_sources(&directories);
    }
    options
        .files
        .iter()
        .map(|file| read_source_argument(file, &directories, &options.root))
        .collect()
}

fn config_directories(root: &Path) -> Vec<PathBuf> {
    if let Some(value) = env::var_os("FRACTALD_SYSCTL_DIR") {
        return env::split_paths(&value).collect();
    }
    [
        "/usr/lib/sysctl.d",
        "/usr/local/lib/sysctl.d",
        "/run/sysctl.d",
        "/etc/sysctl.d",
    ]
    .into_iter()
    .map(|path| root_path(root, Path::new(path)))
    .collect()
}

fn discover_sources(directories: &[PathBuf]) -> Result<Vec<Source>, String> {
    let mut selected = BTreeMap::<String, PathBuf>::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot read {}: {error}", directory.display())),
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("conf") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid sysctl filename {}", path.display()))?
                .to_owned();
            if fs::read_link(&path).ok().as_deref() == Some(Path::new("/dev/null")) {
                selected.remove(&name);
            } else {
                selected.insert(name, path);
            }
        }
    }
    selected
        .into_iter()
        .map(|(_name, path)| {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            Ok(Source {
                label: path.display().to_string(),
                text,
            })
        })
        .collect()
}

fn read_source_argument(
    argument: &str,
    directories: &[PathBuf],
    root: &Path,
) -> Result<Source, String> {
    if argument == "-" {
        let mut text = String::new();
        io::stdin()
            .read_to_string(&mut text)
            .map_err(|error| format!("cannot read configuration from stdin: {error}"))?;
        return Ok(Source {
            label: "stdin".to_owned(),
            text,
        });
    }
    let direct = Path::new(argument);
    let path = if direct.components().count() > 1 || direct.is_absolute() {
        if direct.is_absolute() {
            if root != Path::new("/") && direct.starts_with(root) {
                direct.to_path_buf()
            } else {
                root_path(root, direct)
            }
        } else {
            direct.to_path_buf()
        }
    } else {
        directories
            .iter()
            .rev()
            .map(|directory| directory.join(argument))
            .find(|path| path.is_file())
            .ok_or_else(|| format!("configuration file {argument} was not found"))?
    };
    let text = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    Ok(Source {
        label: path.display().to_string(),
        text,
    })
}

fn print_sources(sources: &[Source], action: Action) -> Result<(), String> {
    for source in sources {
        if action == Action::CatConfig {
            println!("# {}", source.label);
        }
        for line in source.text.lines() {
            let trimmed = line.trim();
            if action == Action::Tldr && (trimmed.is_empty() || trimmed.starts_with('#')) {
                continue;
            }
            println!("{line}");
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    key: String,
    value: String,
    ignore_errors: bool,
}

fn parse_rule(line: &str) -> Result<Option<Rule>, String> {
    let line = line.split('#').next().unwrap_or_default().trim();
    if line.is_empty() {
        return Ok(None);
    }
    let (raw_key, raw_value) = line.split_once('=').map_or_else(
        || {
            let mut fields = line.split_whitespace();
            (
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            )
        },
        |(key, value)| (key.trim(), value.trim()),
    );
    if raw_key.is_empty() || raw_value.is_empty() {
        return Err("sysctl rule requires a key and value".to_owned());
    }
    let ignore_errors = raw_key.starts_with('-');
    let key = raw_key.trim_start_matches('-').trim().to_owned();
    if key.is_empty()
        || key
            .split(['.', '/'])
            .any(|part| part == ".." || part.is_empty())
    {
        return Err(format!("invalid sysctl key {raw_key:?}"));
    }
    Ok(Some(Rule {
        key,
        value: raw_value.trim_matches(['"', '\'']).trim().to_owned(),
        ignore_errors,
    }))
}

fn apply_sources(options: &Options, sources: &[Source]) -> Result<(), String> {
    let proc_root = root_path(&options.root, Path::new("/proc/sys"));
    let mut failures = Vec::new();
    for source in sources {
        for (line_index, line) in source.text.lines().enumerate() {
            let Some(rule) = parse_rule(line)
                .map_err(|error| format!("{}:{}: {error}", source.label, line_index + 1))?
            else {
                continue;
            };
            if !options.prefixes.is_empty()
                && !options
                    .prefixes
                    .iter()
                    .any(|prefix| rule.key.starts_with(prefix))
            {
                continue;
            }
            let targets = expand_rule(&proc_root, &rule.key)
                .map_err(|error| format!("{}:{}: {error}", source.label, line_index + 1))?;
            if targets.is_empty() {
                if !rule.ignore_errors {
                    failures.push(format!(
                        "{}:{}: no matching sysctl path for {}",
                        source.label,
                        line_index + 1,
                        rule.key
                    ));
                }
                continue;
            }
            for target in targets {
                if options.dry_run {
                    println!("Would set {} = {}", target.display(), rule.value);
                    continue;
                }
                if let Err(error) = fs::write(&target, format!("{}\n", rule.value)) {
                    if !rule.ignore_errors {
                        failures.push(format!(
                            "{}:{}: cannot set {}: {error}",
                            source.label,
                            line_index + 1,
                            target.display()
                        ));
                    }
                }
            }
        }
    }
    if options.strict && !failures.is_empty() {
        return Err(failures.join("; "));
    }
    for failure in failures {
        eprintln!("systemd-sysctl: {failure}");
    }
    Ok(())
}

fn expand_rule(root: &Path, key: &str) -> Result<Vec<PathBuf>, String> {
    let normalized = key.replace('.', "/");
    let components = normalized
        .split('/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components.is_empty() || components.iter().any(|component| *component == "..") {
        return Err(format!("invalid sysctl key {key:?}"));
    }
    expand_components(root, &components)
}

fn expand_components(base: &Path, components: &[&str]) -> Result<Vec<PathBuf>, String> {
    let Some(component) = components.first() else {
        return Ok(vec![base.to_path_buf()]);
    };
    if !has_glob(component) {
        let path = base.join(component);
        if components.len() == 1 {
            return Ok(path.is_file().then_some(path).into_iter().collect());
        }
        return expand_components(&path, &components[1..]);
    }
    let entries = match fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("cannot enumerate {}: {error}", base.display())),
    };
    let mut matches = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("cannot enumerate {}: {error}", base.display()))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !glob_matches(component, &name) {
            continue;
        }
        let path = entry.path();
        if components.len() == 1 {
            if path.is_file() {
                matches.push(path);
            }
        } else if path.is_dir() {
            matches.extend(expand_components(&path, &components[1..])?);
        }
    }
    matches.sort();
    Ok(matches)
}

fn has_glob(value: &str) -> bool {
    value.bytes().any(|byte| matches!(byte, b'*' | b'?'))
}

fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for index in 0..pattern.len() {
        if pattern[index] == b'*' {
            table[index + 1][0] = table[index][0];
        }
        for value_index in 0..value.len() {
            table[index + 1][value_index + 1] = match pattern[index] {
                b'*' => table[index][value_index + 1] || table[index + 1][value_index],
                b'?' => table[index][value_index],
                byte => table[index][value_index] && byte == value[value_index],
            };
        }
    }
    table[pattern.len()][value.len()]
}

fn root_path(root: &Path, path: &Path) -> PathBuf {
    if root == Path::new("/") {
        path.to_path_buf()
    } else {
        root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

fn print_help() {
    println!(
        "systemd-sysctl (FractalD)\n\nApply sysctl.d configuration without a systemd manager.\n\nUsage: systemd-sysctl [OPTIONS] [CONFIGURATION FILE...]\n\n  --root=PATH       operate below an alternate root\n  --prefix=PREFIX   apply only matching keys\n  --strict          fail if a rule cannot be applied\n  --dry-run         print changes without writing\n  --cat-config      print selected configuration files\n  --tldr            print non-comment configuration lines\n  --version         show the version"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_equals_and_ignore_error_rules() {
        assert_eq!(
            parse_rule("-net.ipv4.ip_forward = 1 # optional").expect("rule"),
            Some(Rule {
                key: "net.ipv4.ip_forward".to_owned(),
                value: "1".to_owned(),
                ignore_errors: true,
            })
        );
    }

    #[test]
    fn matches_wildcard_components() {
        assert!(glob_matches("*", "default"));
        assert!(glob_matches("eth?", "eth0"));
        assert!(!glob_matches("eth?", "wlan0"));
    }
}
