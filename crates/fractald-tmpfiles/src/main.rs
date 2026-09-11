use std::collections::BTreeMap;
use std::env;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Create,
    Clean,
    Remove,
    Purge,
    CatConfig,
    Tldr,
}

#[derive(Debug, Default)]
struct Options {
    action: Option<Action>,
    root: Option<PathBuf>,
    prefixes: Vec<String>,
    excluded_prefixes: Vec<String>,
    files: Vec<PathBuf>,
    user: bool,
    boot: bool,
    graceful: bool,
    dry_run: bool,
    ignore_system_paths: bool,
    show_help: bool,
    show_version: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    kind: char,
    force: bool,
    path: String,
    mode: Option<u32>,
    user: Option<String>,
    group: Option<String>,
    age: Option<Duration>,
    argument: Option<String>,
}

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-tmpfiles: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<(), String> {
    let options = parse_args(args)?;
    if options.show_help {
        print_help();
        return Ok(());
    }
    if options.show_version {
        println!("systemd-tmpfiles (FractalD) {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if options.action.is_none() {
        return Err(
            "one of --create, --clean, --remove, --purge, --cat-config, or --tldr is required"
                .to_owned(),
        );
    }
    let files = configuration_files(&options)?;
    match options.action.expect("checked action") {
        Action::CatConfig | Action::Tldr => print_configuration(&files, options.action)?,
        action => {
            let executor = Executor { options: &options };
            for file in files {
                executor.process_file(&file, action)?;
            }
        }
    }
    Ok(())
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut options = Options::default();
    let mut values = args.into_iter().peekable();
    let mut after_separator = false;
    while let Some(value) = values.next() {
        let value = value
            .into_string()
            .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
        if after_separator {
            options.files.push(PathBuf::from(value));
            continue;
        }
        match value.as_str() {
            "--create" => options.action = Some(Action::Create),
            "--clean" => options.action = Some(Action::Clean),
            "--remove" => options.action = Some(Action::Remove),
            "--purge" => options.action = Some(Action::Purge),
            "--cat-config" => options.action = Some(Action::CatConfig),
            "--tldr" => options.action = Some(Action::Tldr),
            "--user" => options.user = true,
            "--boot" => options.boot = true,
            "--graceful" => options.graceful = true,
            "--dry-run" => options.dry_run = true,
            "-E" => options.ignore_system_paths = true,
            "--no-pager" | "--no-legend" => {}
            "--root" => options.root = Some(next_value(&mut values, "--root")?),
            "--prefix" => options.prefixes.push(
                next_value(&mut values, "--prefix")?
                    .to_string_lossy()
                    .into_owned(),
            ),
            "--exclude-prefix" => options.excluded_prefixes.push(
                next_value(&mut values, "--exclude-prefix")?
                    .to_string_lossy()
                    .into_owned(),
            ),
            "--help" | "-h" => {
                options.show_help = true;
            }
            "--version" => {
                options.show_version = true;
            }
            "--replace" => {
                let replacement = next_value(&mut values, "--replace")?;
                options.files.push(replacement);
            }
            "--" => after_separator = true,
            value if value.starts_with("--root=") => {
                options.root = Some(PathBuf::from(&value[7..]));
            }
            value if value.starts_with("--prefix=") => options.prefixes.push(value[9..].to_owned()),
            value if value.starts_with("--exclude-prefix=") => {
                options.excluded_prefixes.push(value[17..].to_owned())
            }
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            value => options.files.push(PathBuf::from(value)),
        }
    }
    Ok(options)
}

fn next_value<I>(values: &mut std::iter::Peekable<I>, option: &str) -> Result<PathBuf, String>
where
    I: Iterator<Item = std::ffi::OsString>,
{
    values
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| format!("{option} requires a value"))
}

fn configuration_files(options: &Options) -> Result<Vec<PathBuf>, String> {
    if !options.files.is_empty() {
        return Ok(options.files.clone());
    }
    let mut candidates = BTreeMap::new();
    let mut directories = Vec::new();
    if let Some(path) = env::var_os("FRACTALD_TMPFILES_DIR") {
        directories.push(PathBuf::from(path));
    } else if options.user {
        if let Some(path) = env::var_os("XDG_CONFIG_HOME") {
            directories.push(PathBuf::from(path).join("user-tmpfiles.d"));
        } else if let Some(path) = env::var_os("HOME") {
            directories.push(PathBuf::from(path).join(".config/user-tmpfiles.d"));
        }
        if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
            directories.push(PathBuf::from(path).join("user-tmpfiles.d"));
        }
    } else {
        directories.extend([
            PathBuf::from("/usr/lib/tmpfiles.d"),
            PathBuf::from("/usr/local/lib/tmpfiles.d"),
            PathBuf::from("/run/tmpfiles.d"),
            PathBuf::from("/etc/tmpfiles.d"),
        ]);
    }
    for directory in directories {
        let entries = match fs::read_dir(&directory) {
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
            if let Some(name) = path.file_name().and_then(|value| value.to_str()) {
                candidates.insert(name.to_owned(), path);
            }
        }
    }
    Ok(candidates.into_values().collect())
}

fn print_configuration(files: &[PathBuf], action: Option<Action>) -> Result<(), String> {
    for file in files {
        let source = fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        if action == Some(Action::CatConfig) {
            println!("# {}", file.display());
        }
        for line in source.lines() {
            let trimmed = line.trim();
            if action == Some(Action::Tldr) && (trimmed.is_empty() || trimmed.starts_with('#')) {
                continue;
            }
            println!("{line}");
        }
    }
    Ok(())
}

struct Executor<'a> {
    options: &'a Options,
}

impl Executor<'_> {
    fn process_file(&self, file: &Path, action: Action) -> Result<(), String> {
        let source = fs::read_to_string(file)
            .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
        for (index, line) in source.lines().enumerate() {
            let line_number = index + 1;
            let Some(rule) = parse_rule(line)
                .map_err(|error| format!("{}:{}: {error}", file.display(), line_number))?
            else {
                continue;
            };
            if rule.force && !self.options.boot {
                continue;
            }
            if !self.selected(&rule.path) {
                continue;
            }
            self.apply(&rule, action)
                .map_err(|error| format!("{}:{}: {error}", file.display(), line_number))?;
        }
        Ok(())
    }

    fn selected(&self, path: &str) -> bool {
        let matches_prefix = self.options.prefixes.is_empty()
            || self
                .options
                .prefixes
                .iter()
                .any(|prefix| path_has_prefix(path, prefix));
        let excluded = self
            .options
            .excluded_prefixes
            .iter()
            .any(|prefix| path_has_prefix(path, prefix));
        matches_prefix && !excluded
    }

    fn apply(&self, rule: &Rule, action: Action) -> Result<(), String> {
        let kind = rule.kind;
        if matches!(
            kind,
            'x' | 'X' | 'a' | 'A' | 'h' | 'H' | 't' | 'T' | 'c' | 'b'
        ) {
            return Ok(());
        }
        let path = self.resolve_path(&expand_specifiers(&rule.path))?;
        match action {
            Action::Create => self.create(rule, &path),
            Action::Clean => self.clean(rule, &path),
            Action::Remove => self.remove(rule, &path),
            Action::Purge => self.purge(rule, &path),
            Action::CatConfig | Action::Tldr => Ok(()),
        }
    }

    fn create(&self, rule: &Rule, path: &Path) -> Result<(), String> {
        match rule.kind {
            'd' | 'D' | 'v' | 'q' | 'Q' => {
                self.announce("create directory", path);
                if !self.options.dry_run {
                    fs::create_dir_all(path).map_err(io_message)?;
                    self.apply_metadata(path, rule, false)?;
                }
            }
            'e' => {
                if fs::symlink_metadata(path).is_ok() {
                    self.announce("adjust directory", path);
                    if !self.options.dry_run {
                        self.apply_metadata(path, rule, false)?;
                    }
                }
            }
            'f' | 'F' => {
                self.announce("create file", path);
                if !self.options.dry_run {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(io_message)?;
                    }
                    let mut options = OpenOptions::new();
                    options
                        .create(true)
                        .write(true)
                        .mode(rule.mode.unwrap_or(0o644));
                    if rule.kind == 'F' {
                        options.truncate(true);
                    }
                    let mut file = options.open(path).map_err(io_message)?;
                    if let Some(argument) = rule.argument.as_ref() {
                        file.write_all(argument.as_bytes()).map_err(io_message)?;
                    }
                    self.apply_metadata(path, rule, false)?;
                }
            }
            'w' => {
                self.announce("write file", path);
                if !self.options.dry_run {
                    let mut options = OpenOptions::new();
                    options.write(true);
                    if rule.force {
                        options.create(true).mode(rule.mode.unwrap_or(0o644));
                    }
                    let mut file = options.open(path).map_err(io_message)?;
                    file.write_all(rule.argument.as_deref().unwrap_or_default().as_bytes())
                        .map_err(io_message)?;
                }
            }
            'p' => {
                self.announce("create fifo", path);
                if !self.options.dry_run {
                    let mode = rule.mode.unwrap_or(0o644);
                    let fd = fractald_platform::open_fifo(path, mode).map_err(io_message)?;
                    fractald_platform::close_fd(fd).map_err(io_message)?;
                    self.apply_metadata(path, rule, false)?;
                }
            }
            'L' => {
                let target = rule
                    .argument
                    .as_deref()
                    .ok_or_else(|| "symlink rule has no target".to_owned())?;
                self.announce("create symlink", path);
                if !self.options.dry_run {
                    if fs::symlink_metadata(path).is_ok() {
                        if rule.force {
                            remove_one(path, true).map_err(io_message)?;
                        } else {
                            return Ok(());
                        }
                    }
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(io_message)?;
                    }
                    std::os::unix::fs::symlink(target, path).map_err(io_message)?;
                }
            }
            'C' => {
                let source = rule
                    .argument
                    .as_deref()
                    .ok_or_else(|| "copy rule has no source".to_owned())?;
                let source = self.resolve_path(&expand_specifiers(source))?;
                self.announce("copy file", path);
                if !self.options.dry_run {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).map_err(io_message)?;
                    }
                    fs::copy(source, path).map_err(io_message)?;
                    self.apply_metadata(path, rule, false)?;
                }
            }
            'z' | 'Z' => {
                self.announce("adjust metadata", path);
                if !self.options.dry_run && fs::symlink_metadata(path).is_ok() {
                    self.apply_metadata(path, rule, rule.kind == 'Z')?;
                }
            }
            'r' | 'R' => {
                self.announce("remove", path);
                if !self.options.dry_run {
                    remove_matches(path, rule.kind == 'R').map_err(io_message)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn clean(&self, rule: &Rule, path: &Path) -> Result<(), String> {
        let Some(age) = rule.age else {
            return Ok(());
        };
        if !path.is_dir() {
            if older_than(path, age) {
                self.announce("clean", path);
                if !self.options.dry_run {
                    remove_one(path, true).map_err(io_message)?;
                }
            }
            return Ok(());
        }
        let entries = fs::read_dir(path).map_err(io_message)?;
        for entry in entries {
            let entry = entry.map_err(io_message)?;
            let child = entry.path();
            if older_than(&child, age) {
                self.announce("clean", &child);
                if !self.options.dry_run {
                    remove_one(&child, true).map_err(io_message)?;
                }
            }
        }
        Ok(())
    }

    fn remove(&self, rule: &Rule, path: &Path) -> Result<(), String> {
        if !matches!(rule.kind, 'D' | 'R' | 'r') {
            return Ok(());
        }
        self.announce("remove", path);
        if !self.options.dry_run {
            if rule.kind == 'D' {
                remove_contents(path).map_err(io_message)?;
            } else {
                remove_matches(path, true).map_err(io_message)?;
            }
        }
        Ok(())
    }

    fn purge(&self, rule: &Rule, path: &Path) -> Result<(), String> {
        if !matches!(
            rule.kind,
            'd' | 'D' | 'e' | 'v' | 'q' | 'Q' | 'f' | 'F' | 'L' | 'p' | 'C' | 'w'
        ) {
            return Ok(());
        }
        self.announce("purge", path);
        if !self.options.dry_run {
            remove_matches(path, matches!(rule.kind, 'd' | 'D' | 'v' | 'q' | 'Q'))
                .map_err(io_message)?;
        }
        Ok(())
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf, String> {
        let path = Path::new(path);
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(format!(
                "path {} must be absolute and contain no '..'",
                path.display()
            ));
        }
        let root = self
            .options
            .root
            .clone()
            .unwrap_or_else(|| PathBuf::from("/"));
        let relative = path.strip_prefix("/").unwrap_or(path);
        Ok(root.join(relative))
    }

    fn apply_metadata(&self, path: &Path, rule: &Rule, recursive: bool) -> Result<(), String> {
        if let Some(mode) = rule.mode {
            let mut permissions = fs::symlink_metadata(path)
                .map_err(io_message)?
                .permissions();
            permissions.set_mode(mode);
            fs::set_permissions(path, permissions).map_err(io_message)?;
        }
        if rule.user.is_some() || rule.group.is_some() {
            let user = rule
                .user
                .as_deref()
                .filter(|value| *value != "-")
                .map(CString::new)
                .transpose()
                .map_err(|_| "tmpfiles user contains NUL".to_owned())?;
            let group = rule
                .group
                .as_deref()
                .filter(|value| *value != "-")
                .map(CString::new)
                .transpose()
                .map_err(|_| "tmpfiles group contains NUL".to_owned())?;
            let path_c = CString::new(path.as_os_str().as_bytes())
                .map_err(|_| "tmpfiles path contains NUL".to_owned())?;
            match fractald_platform::chown_path(&path_c, user.as_deref(), group.as_deref()) {
                Ok(()) => {}
                Err(error) if self.options.graceful && error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        if recursive && path.is_dir() {
            for entry in fs::read_dir(path).map_err(io_message)? {
                let child = entry.map_err(io_message)?.path();
                self.apply_metadata(&child, rule, true)?;
            }
        }
        Ok(())
    }

    fn announce(&self, operation: &str, path: &Path) {
        if self.options.dry_run {
            println!("{operation} {}", path.display());
        }
    }
}

fn parse_rule(line: &str) -> Result<Option<Rule>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let fields = split_words(line)?;
    if fields.len() < 2 {
        return Err("tmpfiles rule requires a type and path".to_owned());
    }
    let type_field = &fields[0];
    let mut characters = type_field.chars();
    let kind = characters
        .next()
        .ok_or_else(|| "empty tmpfiles type".to_owned())?;
    let force = type_field.contains('!');
    let path = fields[1].clone();
    let mode = match fields.get(2) {
        Some(value) => parse_mode(value)?,
        None => None,
    };
    let user = fields.get(3).filter(|value| value.as_str() != "-").cloned();
    let group = fields.get(4).filter(|value| value.as_str() != "-").cloned();
    let age = fields
        .get(5)
        .filter(|value| value.as_str() != "-")
        .map(|value| parse_age(value))
        .transpose()?;
    let argument = (fields.len() > 6).then(|| fields[6..].join(" "));
    Ok(Some(Rule {
        kind,
        force,
        path,
        mode,
        user,
        group,
        age,
        argument,
    }))
}

fn parse_mode(value: &str) -> Result<Option<u32>, String> {
    if value == "-" {
        return Ok(None);
    }
    let value = value.strip_prefix('~').unwrap_or(value);
    u32::from_str_radix(value, 8)
        .map(Some)
        .map_err(|_| format!("invalid mode {value}"))
}

fn parse_age(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value == "" || value == "-" {
        return Err("empty age".to_owned());
    }
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    let split = value
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .ok_or_else(|| format!("invalid age {value}"))?;
    let number = value[..split]
        .parse::<f64>()
        .map_err(|_| format!("invalid age {value}"))?;
    let unit = &value[split..];
    let seconds = match unit {
        "s" | "sec" | "secs" => number,
        "m" | "min" | "mins" => number * 60.0,
        "h" | "hour" | "hours" => number * 3_600.0,
        "d" | "day" | "days" => number * 86_400.0,
        "w" | "week" | "weeks" => number * 604_800.0,
        "M" | "month" | "months" => number * 2_629_800.0,
        "y" | "year" | "years" => number * 31_557_600.0,
        _ => return Err(format!("invalid age {value}")),
    };
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("invalid age {value}"));
    }
    Ok(Duration::from_secs_f64(seconds))
}

fn split_words(value: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    for character in value.chars() {
        if escaped {
            current.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                started = true;
            }
            character if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if escaped || quote.is_some() {
        return Err("unterminated tmpfiles escape or quote".to_owned());
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

fn expand_specifiers(value: &str) -> String {
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let machine_id = fs::read_to_string("/etc/machine-id")
        .ok()
        .map(|value| value.trim().to_owned())
        .unwrap_or_default();
    let runtime = env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/run".to_owned());
    let state = env::var("XDG_STATE_HOME").unwrap_or_else(|_| "/var/lib".to_owned());
    let home = env::var("HOME").unwrap_or_else(|_| "/root".to_owned());
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(specifier) = characters.next() else {
            output.push('%');
            break;
        };
        let replacement = match specifier {
            '%' => "%".to_owned(),
            'b' => boot_id.clone(),
            'm' => machine_id.clone(),
            't' => runtime.clone(),
            'S' => state.clone(),
            'h' => home.clone(),
            'u' => env::var("USER").unwrap_or_default(),
            'U' => fractald_platform::effective_uid().to_string(),
            _ => {
                output.push('%');
                output.push(specifier);
                continue;
            }
        };
        output.push_str(&replacement);
    }
    output
}

fn path_has_prefix(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    path == prefix || path.starts_with(&format!("{prefix}/"))
}

fn older_than(path: &Path, age: Duration) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

fn remove_matches(path: &Path, recursive: bool) -> io::Result<()> {
    if has_wildcard(path) {
        for match_path in expand_glob(path)? {
            remove_one(&match_path, recursive)?;
        }
        Ok(())
    } else {
        remove_one(path, recursive)
    }
}

fn remove_contents(path: &Path) -> io::Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        remove_one(&entry?.path(), true)?;
    }
    Ok(())
}

fn remove_one(path: &Path, recursive: bool) -> io::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_dir() {
        if recursive {
            fs::remove_dir_all(path)
        } else {
            fs::remove_dir(path)
        }
    } else {
        fs::remove_file(path)
    }
}

fn has_wildcard(path: &Path) -> bool {
    path.as_os_str()
        .to_string_lossy()
        .bytes()
        .any(|character| matches!(character, b'*' | b'?'))
}

fn expand_glob(path: &Path) -> io::Result<Vec<PathBuf>> {
    let mut current = vec![PathBuf::from("/")];
    for component in path.components() {
        let Component::Normal(component) = component else {
            continue;
        };
        let pattern = component.to_string_lossy();
        let mut next = Vec::new();
        for base in current {
            if pattern.bytes().any(|value| matches!(value, b'*' | b'?')) {
                let entries = match fs::read_dir(&base) {
                    Ok(entries) => entries,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name();
                    if wildcard_match(&pattern, &name.to_string_lossy()) {
                        next.push(entry.path());
                    }
                }
            } else {
                next.push(base.join(component));
            }
        }
        current = next;
    }
    Ok(current)
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut states = vec![false; value.len() + 1];
    states[0] = true;
    for &character in pattern {
        let mut next = vec![false; value.len() + 1];
        for index in 0..=value.len() {
            if !states[index] {
                continue;
            }
            match character {
                b'*' => {
                    for state in next.iter_mut().skip(index) {
                        *state = true;
                    }
                }
                b'?' if index < value.len() => next[index + 1] = true,
                literal if index < value.len() && literal == value[index] => next[index + 1] = true,
                _ => {}
            }
        }
        states = next;
    }
    states[value.len()]
}

fn io_message(error: io::Error) -> String {
    error.to_string()
}

fn print_help() {
    println!(
        "systemd-tmpfiles (FractalD)\n\nUsage: systemd-tmpfiles COMMAND [OPTIONS] [CONFIGURATION FILE...]\n\n  --create              create and adjust files and directories\n  --clean               clean entries older than their configured age\n  --remove              remove entries marked for removal\n  --purge               remove entries from the supplied configuration\n  --cat-config          print effective configuration\n  --tldr                print non-comment configuration\n  --root=PATH           operate below an alternate filesystem root\n  --prefix=PATH         select rules below a path\n  --exclude-prefix=PATH exclude rules below a path\n  --boot                include boot-only rules\n  --graceful            ignore unknown owners\n  --dry-run             print operations without changing files"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_creation_and_symlink_rules() {
        assert_eq!(
            parse_rule("d /var/lib/example 0750 example example 2d").expect("rule"),
            Some(Rule {
                kind: 'd',
                force: false,
                path: "/var/lib/example".to_owned(),
                mode: Some(0o750),
                user: Some("example".to_owned()),
                group: Some("example".to_owned()),
                age: Some(Duration::from_secs(172_800)),
                argument: None,
            })
        );
        let rule = parse_rule("L! /etc/example - - - - ../run/example")
            .expect("symlink rule")
            .expect("rule");
        assert_eq!(rule.kind, 'L');
        assert!(rule.force);
        assert_eq!(rule.argument.as_deref(), Some("../run/example"));
    }

    #[test]
    fn creates_and_removes_rules_below_an_alternate_root() {
        let root = std::env::temp_dir().join(format!("fractald-tmpfiles-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("root");
        let config = root.join("rules.conf");
        fs::write(
            &config,
            "D /var/lib/example 0750 - - -\nf /var/lib/example/state 0640 - - -\nL /var/lib/example/current - - - - state\n",
        )
        .expect("rules");
        let options = Options {
            action: Some(Action::Create),
            root: Some(root.clone()),
            files: vec![config.clone()],
            ..Options::default()
        };
        Executor { options: &options }
            .process_file(&config, Action::Create)
            .expect("create rules");
        assert!(root.join("var/lib/example/state").is_file());
        assert_eq!(
            fs::read_link(root.join("var/lib/example/current")).expect("symlink"),
            PathBuf::from("state")
        );
        Executor { options: &options }
            .process_file(&config, Action::Remove)
            .expect("remove rules");
        assert!(root.join("var/lib/example").is_dir());
        assert!(!root.join("var/lib/example/state").exists());
        assert!(!root.join("var/lib/example/current").exists());
        let _ = fs::remove_dir_all(root);
    }
}
