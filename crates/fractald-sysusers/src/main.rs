use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

const DEFAULT_SYSTEM_ID_MIN: u32 = 201;
const DEFAULT_SYSTEM_ID_MAX: u32 = 999;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-sysusers: {error}");
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
        println!("systemd-sysusers (FractalD) {}", env!("CARGO_PKG_VERSION"));
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
    files: Vec<String>,
    replace: Option<String>,
    inline: bool,
    dry_run: bool,
    action: Action,
    show_help: bool,
    show_version: bool,
}

fn parse_args(args: impl IntoIterator<Item = std::ffi::OsString>) -> Result<Options, String> {
    let mut options = Options {
        root: PathBuf::from("/"),
        files: Vec::new(),
        replace: None,
        inline: false,
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
            value if value.starts_with("--root=") => {
                options.root = PathBuf::from(&value[7..]);
            }
            "--replace" => {
                options.replace = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--replace requires a path".to_owned())?
                        .into_string()
                        .map_err(|_| "--replace path must be valid UTF-8".to_owned())?,
                );
            }
            value if value.starts_with("--replace=") => {
                options.replace = Some(value[10..].to_owned());
            }
            "--inline" => options.inline = true,
            "--dry-run" => options.dry_run = true,
            "--cat-config" => options.action = Action::CatConfig,
            "--tldr" => options.action = Action::Tldr,
            "--no-pager" => {}
            "--help" | "-h" => options.show_help = true,
            "--version" => options.show_version = true,
            "-" => options.files.push("-".to_owned()),
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            value => options.files.push(value.to_owned()),
        }
    }
    if options.show_help || options.show_version {
        return Ok(options);
    }
    if options.inline && options.files.is_empty() {
        return Err("--inline requires at least one configuration line".to_owned());
    }
    if options.replace.is_some() && options.files.is_empty() {
        return Err("--replace requires at least one configuration argument".to_owned());
    }
    Ok(options)
}

#[derive(Clone, Debug)]
struct Source {
    name: String,
    label: String,
    text: String,
}

fn load_sources(options: &Options) -> Result<Vec<Source>, String> {
    let directories = config_directories(&options.root);
    let mut selected = if options.files.is_empty() || options.replace.is_some() {
        discover_sources(&directories)?
    } else {
        Vec::new()
    };

    if options.files.is_empty() && options.replace.is_none() {
        return Ok(selected);
    }

    let replacements = if options.inline {
        options
            .files
            .iter()
            .enumerate()
            .map(|(index, line)| Source {
                name: options
                    .replace
                    .as_deref()
                    .and_then(|path| Path::new(path).file_name())
                    .and_then(|name| name.to_str())
                    .map_or_else(|| format!("<inline-{index}>"), ToOwned::to_owned),
                label: format!("<inline:{}>", index + 1),
                text: format!("{line}\n"),
            })
            .collect::<Vec<_>>()
    } else {
        options
            .files
            .iter()
            .map(|file| read_source_argument(file, &directories, &options.root))
            .collect::<Result<Vec<_>, _>>()?
    };

    if let Some(replace) = &options.replace {
        let name = Path::new(replace)
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid replacement path {replace}"))?
            .to_owned();
        let administrator_file = root_path(&options.root, Path::new("/etc/sysusers.d")).join(&name);
        if !administrator_file.exists() {
            selected.retain(|source| source.name != name);
            for mut source in replacements {
                source.name = name.clone();
                selected.push(source);
            }
        }
        selected.sort_by(|left, right| left.name.cmp(&right.name));
        return Ok(selected);
    }

    Ok(replacements)
}

fn config_directories(root: &Path) -> Vec<PathBuf> {
    if let Some(value) = env::var_os("FRACTALD_SYSUSERS_DIR") {
        return env::split_paths(&value).collect();
    }
    [
        "/usr/lib/sysusers.d",
        "/usr/local/lib/sysusers.d",
        "/run/sysusers.d",
        "/etc/sysusers.d",
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
                .ok_or_else(|| format!("invalid sysusers filename {}", path.display()))?
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
        .map(|(name, path)| {
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            Ok(Source {
                name,
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
            name: "-".to_owned(),
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
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or(argument)
        .to_owned();
    Ok(Source {
        name,
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

fn apply_sources(options: &Options, sources: &[Source]) -> Result<(), String> {
    let _lock = if options.dry_run {
        None
    } else {
        Some(LockFile::acquire(&options.root)?)
    };
    let mut database = Database::load(&options.root)?;
    for source in sources {
        for (line_index, line) in source.text.lines().enumerate() {
            let Some(directive) = parse_directive(line)
                .map_err(|error| format!("{}:{}: {error}", source.label, line_index + 1))?
            else {
                continue;
            };
            database
                .apply(directive, options.dry_run)
                .map_err(|error| format!("{}:{}: {error}", source.label, line_index + 1))?;
        }
    }
    if !options.dry_run {
        database.write_all()?;
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Directive {
    kind: DirectiveKind,
    fields: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirectiveKind {
    User { locked: bool },
    Group,
    Member,
    Range,
}

fn parse_directive(line: &str) -> Result<Option<Directive>, String> {
    let fields = split_fields(line)?;
    if fields.is_empty() {
        return Ok(None);
    }
    let kind = match fields[0].as_str() {
        "u" => DirectiveKind::User { locked: false },
        "u!" => DirectiveKind::User { locked: true },
        "g" => DirectiveKind::Group,
        "m" => DirectiveKind::Member,
        "r" => DirectiveKind::Range,
        value => return Err(format!("unknown directive type {value:?}")),
    };
    Ok(Some(Directive {
        kind,
        fields: fields[1..].to_vec(),
    }))
}

fn split_fields(line: &str) -> Result<Vec<String>, String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = None;
    let mut escaped = false;
    for character in line.chars() {
        if escaped {
            current.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(quote) = quoted {
            if character == quote {
                quoted = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quoted = Some(character);
            continue;
        }
        if character == '#' {
            break;
        }
        if character.is_ascii_whitespace() {
            if !current.is_empty() {
                fields.push(std::mem::take(&mut current));
            }
        } else {
            current.push(character);
        }
    }
    if escaped {
        current.push('\\');
    }
    if quoted.is_some() {
        return Err("unterminated quoted field".to_owned());
    }
    if !current.is_empty() {
        fields.push(current);
    }
    Ok(fields)
}

struct LockFile {
    path: PathBuf,
}

impl LockFile {
    fn acquire(root: &Path) -> Result<Self, String> {
        let directory = root_path(root, Path::new("/etc"));
        fs::create_dir_all(&directory)
            .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
        let path = directory.join(".fractald-sysusers.lock");
        for _ in 0..100 {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id())
                        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(format!("cannot lock {}: {error}", path.display())),
            }
        }
        Err(format!("timed out waiting for {}", path.display()))
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn root_path(root: &Path, path: &Path) -> PathBuf {
    if root == Path::new("/") {
        path.to_path_buf()
    } else {
        root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

struct Database {
    root: PathBuf,
    passwd: TextDatabase,
    group: TextDatabase,
    shadow: TextDatabase,
    gshadow: TextDatabase,
    ranges: Vec<(u32, u32)>,
}

impl Database {
    fn load(root: &Path) -> Result<Self, String> {
        let etc = root_path(root, Path::new("/etc"));
        Ok(Self {
            root: root.to_path_buf(),
            passwd: TextDatabase::load(etc.join("passwd"), 0o644)?,
            group: TextDatabase::load(etc.join("group"), 0o644)?,
            shadow: TextDatabase::load(etc.join("shadow"), 0o000)?,
            gshadow: TextDatabase::load(etc.join("gshadow"), 0o000)?,
            ranges: system_ranges(root),
        })
    }

    fn apply(&mut self, directive: Directive, dry_run: bool) -> Result<(), String> {
        match directive.kind {
            DirectiveKind::Range => {
                expect_fields(&directive.fields, 2, "r")?;
                if directive.fields[0] != "-" {
                    return Err("r directives require '-' as the name".to_owned());
                }
                self.ranges = vec![parse_range(&directive.fields[1])?];
            }
            DirectiveKind::Group => {
                expect_group_fields(&directive.fields)?;
                validate_name(&directive.fields[0])?;
                let id = directive.fields.get(1).map(String::as_str).unwrap_or("-");
                self.ensure_group(&directive.fields[0], id, dry_run)?;
            }
            DirectiveKind::User { locked } => {
                if directive.fields.len() < 2 || directive.fields.len() > 5 {
                    return Err(
                        "u directives require name and ID, with at most three optional fields"
                            .to_owned(),
                    );
                }
                validate_name(&directive.fields[0])?;
                self.ensure_user(
                    &directive.fields[0],
                    &directive.fields[1],
                    directive.fields.get(2).map(String::as_str),
                    directive.fields.get(3).map(String::as_str),
                    directive.fields.get(4).map(String::as_str),
                    locked,
                    dry_run,
                )?;
            }
            DirectiveKind::Member => {
                expect_fields(&directive.fields, 2, "m")?;
                validate_name(&directive.fields[0])?;
                validate_name(&directive.fields[1])?;
                self.ensure_user(&directive.fields[0], "-", None, None, None, false, dry_run)?;
                self.ensure_group(&directive.fields[1], "-", dry_run)?;
                self.add_member(&directive.fields[0], &directive.fields[1])?;
            }
        }
        Ok(())
    }

    fn ensure_group(&mut self, name: &str, requested: &str, dry_run: bool) -> Result<u32, String> {
        if let Some((_, fields)) = self.group.find(name) {
            return numeric_field(&fields, 2, "group", name);
        }
        let mut id = self.resolve_id(requested, false)?;
        if self.id_in_use(id) {
            eprintln!("Suggested group ID {id} for {name} already used.");
            id = self.allocate_id()?;
        }
        self.create_group(name, id, dry_run)?;
        Ok(id)
    }

    fn ensure_user(
        &mut self,
        name: &str,
        requested: &str,
        gecos: Option<&str>,
        home: Option<&str>,
        shell: Option<&str>,
        locked: bool,
        dry_run: bool,
    ) -> Result<u32, String> {
        if let Some((_, fields)) = self.passwd.find(name) {
            return numeric_field(&fields, 2, "user", name);
        }

        let (uid_request, primary_request) = requested
            .split_once(':')
            .map_or((requested, None), |(uid, primary)| (uid, Some(primary)));
        let (uid, gid) = if let Some(primary) = primary_request {
            let gid = if primary == "-" {
                self.allocate_id()?
            } else if let Ok(value) = primary.parse::<u32>() {
                validate_id(value)?;
                value
            } else {
                self.group
                    .find(primary)
                    .map(|(_, fields)| numeric_field(&fields, 2, "group", primary))
                    .transpose()?
                    .ok_or_else(|| format!("primary group {primary} does not exist"))?
            };
            let mut uid = self.resolve_id(uid_request, true)?;
            if self.uid_in_use(uid) {
                eprintln!("Suggested user ID {uid} for {name} already used.");
                uid = self.allocate_id()?;
            }
            (uid, gid)
        } else if let Some((_, group_fields)) = self.group.find(name) {
            let gid = numeric_field(&group_fields, 2, "group", name)?;
            let mut uid = self.resolve_id(uid_request, true)?;
            if self.uid_in_use_except(uid, Some(gid)) {
                eprintln!("Suggested user ID {uid} for {name} already used.");
                uid = self.allocate_id()?;
            }
            (uid, gid)
        } else {
            let mut id = self.resolve_id(uid_request, true)?;
            if self.id_in_use(id) {
                eprintln!("Suggested user ID {id} for {name} already used.");
                id = self.allocate_id()?;
            }
            self.create_group(name, id, dry_run)?;
            (id, id)
        };

        let gecos = optional_field(gecos);
        let home = optional_field(home).unwrap_or_else(|| "/".to_owned());
        let shell = optional_field(shell).unwrap_or_else(|| {
            if uid == 0 {
                "/bin/sh".to_owned()
            } else {
                "/usr/sbin/nologin".to_owned()
            }
        });
        self.create_user(
            name,
            uid,
            gid,
            gecos.as_deref().unwrap_or(""),
            &home,
            &shell,
            locked,
            dry_run,
        )?;
        Ok(uid)
    }

    fn add_member(&mut self, user: &str, group: &str) -> Result<(), String> {
        if let Some((index, fields)) = self.group.find(group) {
            let mut fields = fields;
            while fields.len() < 4 {
                fields.push(String::new());
            }
            let members = fields[3]
                .split(',')
                .filter(|member| !member.is_empty())
                .collect::<BTreeSet<_>>();
            if !members.contains(user) {
                fields[3] = members
                    .into_iter()
                    .chain(std::iter::once(user))
                    .collect::<Vec<_>>()
                    .join(",");
                self.group.replace(index, &fields);
            }
        }
        if let Some((index, fields)) = self.gshadow.find(group) {
            let mut fields = fields;
            while fields.len() < 4 {
                fields.push(String::new());
            }
            let members = fields[3]
                .split(',')
                .filter(|member| !member.is_empty())
                .collect::<BTreeSet<_>>();
            if !members.contains(user) {
                fields[3] = members
                    .into_iter()
                    .chain(std::iter::once(user))
                    .collect::<Vec<_>>()
                    .join(",");
                self.gshadow.replace(index, &fields);
            }
        }
        Ok(())
    }

    fn create_group(&mut self, name: &str, id: u32, dry_run: bool) -> Result<(), String> {
        if self.group.find(name).is_some() {
            return Ok(());
        }
        println!(
            "{} group '{name}' with GID {id}.",
            if dry_run { "Would create" } else { "Creating" }
        );
        self.group.append(&[
            name.to_owned(),
            "x".to_owned(),
            id.to_string(),
            String::new(),
        ]);
        if self.gshadow.exists {
            self.gshadow.append(&[
                name.to_owned(),
                "!*".to_owned(),
                String::new(),
                String::new(),
            ]);
        }
        Ok(())
    }

    fn create_user(
        &mut self,
        name: &str,
        uid: u32,
        gid: u32,
        gecos: &str,
        home: &str,
        shell: &str,
        locked: bool,
        dry_run: bool,
    ) -> Result<(), String> {
        if self.passwd.find(name).is_some() {
            return Ok(());
        }
        let description = if gecos.is_empty() { "n/a" } else { gecos };
        println!(
            "{} user '{name}' ({description}) with UID {uid} and GID {gid}.",
            if dry_run { "Would create" } else { "Creating" }
        );
        self.passwd.append(&[
            name.to_owned(),
            "x".to_owned(),
            uid.to_string(),
            gid.to_string(),
            gecos.to_owned(),
            home.to_owned(),
            shell.to_owned(),
        ]);
        if self.shadow.exists {
            self.shadow.append(&[
                name.to_owned(),
                "!*".to_owned(),
                "0".to_owned(),
                "0".to_owned(),
                "99999".to_owned(),
                "7".to_owned(),
                String::new(),
                String::new(),
                if locked { "1" } else { "" }.to_owned(),
            ]);
        }
        Ok(())
    }

    fn resolve_id(&mut self, requested: &str, user: bool) -> Result<u32, String> {
        if requested == "-" {
            return self.allocate_id();
        }
        if let Ok(value) = requested.parse::<u32>() {
            validate_id(value)?;
            return Ok(value);
        }
        if requested.starts_with('/') {
            let path = root_path(&self.root, Path::new(requested));
            let metadata = fs::metadata(&path)
                .map_err(|error| format!("cannot inspect ID source {}: {error}", path.display()))?;
            return Ok(if user { metadata.uid() } else { metadata.gid() });
        }
        Err(format!("invalid ID {requested:?}"))
    }

    fn allocate_id(&self) -> Result<u32, String> {
        let used = self.used_ids();
        for &(low, high) in self.ranges.iter().rev() {
            for value in (low..=high).rev() {
                if !used.contains(&value) {
                    return Ok(value);
                }
            }
        }
        Err("no free system UID/GID is available".to_owned())
    }

    fn used_ids(&self) -> BTreeSet<u32> {
        let mut used = BTreeSet::new();
        for database in [&self.passwd, &self.group] {
            for line in &database.lines {
                let fields = line.split(':').collect::<Vec<_>>();
                if let Some(value) = fields.get(2).and_then(|value| value.parse::<u32>().ok()) {
                    used.insert(value);
                }
            }
        }
        used
    }

    fn id_in_use(&self, id: u32) -> bool {
        self.used_ids().contains(&id)
    }

    fn uid_in_use(&self, id: u32) -> bool {
        self.passwd.lines.iter().any(|line| {
            line.split(':')
                .nth(2)
                .and_then(|value| value.parse::<u32>().ok())
                == Some(id)
        })
    }

    fn uid_in_use_except(&self, id: u32, allowed_group: Option<u32>) -> bool {
        self.uid_in_use(id)
            || self.group.lines.iter().any(|line| {
                let value = line
                    .split(':')
                    .nth(2)
                    .and_then(|value| value.parse::<u32>().ok());
                value == Some(id) && value != allowed_group
            })
    }

    fn write_all(&mut self) -> Result<(), String> {
        self.passwd.write_if_changed()?;
        self.group.write_if_changed()?;
        self.shadow.write_if_changed()?;
        self.gshadow.write_if_changed()?;
        Ok(())
    }
}

fn system_ranges(root: &Path) -> Vec<(u32, u32)> {
    let path = root_path(root, Path::new("/etc/login.defs"));
    let mut values = BTreeMap::new();
    if let Ok(source) = fs::read_to_string(path) {
        for line in source.lines() {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() == 2 {
                if let Ok(value) = fields[1].parse::<u32>() {
                    values.insert(fields[0].to_owned(), value);
                }
            }
        }
    }
    let low = values
        .get("SYS_UID_MIN")
        .copied()
        .into_iter()
        .chain(values.get("SYS_GID_MIN").copied())
        .min()
        .unwrap_or(DEFAULT_SYSTEM_ID_MIN);
    let high = values
        .get("SYS_UID_MAX")
        .copied()
        .into_iter()
        .chain(values.get("SYS_GID_MAX").copied())
        .max()
        .unwrap_or(DEFAULT_SYSTEM_ID_MAX);
    vec![(low, high)]
}

fn parse_range(value: &str) -> Result<(u32, u32), String> {
    let (low, high) = value
        .split_once('-')
        .map_or((value, value), |(low, high)| (low, high));
    let low = low
        .parse::<u32>()
        .map_err(|_| format!("invalid allocation range {value:?}"))?;
    let high = high
        .parse::<u32>()
        .map_err(|_| format!("invalid allocation range {value:?}"))?;
    validate_id(low)?;
    validate_id(high)?;
    if low > high {
        return Err(format!("allocation range {value:?} is reversed"));
    }
    Ok((low, high))
}

fn validate_id(value: u32) -> Result<(), String> {
    if value == u16::MAX as u32 || value == u32::MAX {
        return Err(format!("ID {value} is reserved"));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), String> {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return Err("account name is empty".to_owned());
    };
    if name.len() > 31
        || !(first.is_ascii_alphabetic() || first == '_')
        || !characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
    {
        return Err(format!("invalid user or group name {name:?}"));
    }
    Ok(())
}

fn optional_field(value: Option<&str>) -> Option<String> {
    value.filter(|value| *value != "-").map(ToOwned::to_owned)
}

fn numeric_field(fields: &[String], index: usize, kind: &str, name: &str) -> Result<u32, String> {
    fields
        .get(index)
        .ok_or_else(|| format!("{kind} {name} has no numeric ID"))?
        .parse::<u32>()
        .map_err(|_| format!("{kind} {name} has an invalid numeric ID"))
}

fn expect_fields(fields: &[String], count: usize, kind: &str) -> Result<(), String> {
    if fields.len() != count {
        return Err(format!("{kind} directives require exactly {count} fields"));
    }
    Ok(())
}

fn expect_group_fields(fields: &[String]) -> Result<(), String> {
    if fields.is_empty() || fields.len() > 4 {
        return Err("g directives require a name and up to three optional fields".to_owned());
    }
    Ok(())
}

struct TextDatabase {
    path: PathBuf,
    lines: Vec<String>,
    exists: bool,
    mode: u32,
    changed: bool,
}

impl TextDatabase {
    fn load(path: PathBuf, default_mode: u32) -> Result<Self, String> {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                    return Err(format!(
                        "account database {} is not a regular file",
                        path.display()
                    ));
                }
                Some(metadata)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        };
        let text = match metadata {
            Some(_) => fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?,
            None => String::new(),
        };
        Ok(Self {
            path,
            lines: text.lines().map(ToOwned::to_owned).collect(),
            exists: metadata.is_some(),
            mode: metadata
                .as_ref()
                .map_or(default_mode, |metadata| metadata.mode() & 0o7777),
            changed: false,
        })
    }

    fn find(&self, name: &str) -> Option<(usize, Vec<String>)> {
        self.lines.iter().enumerate().find_map(|(index, line)| {
            let fields = line.split(':').map(ToOwned::to_owned).collect::<Vec<_>>();
            (fields.first().map(String::as_str) == Some(name) && fields.len() >= 3)
                .then_some((index, fields))
        })
    }

    fn append(&mut self, fields: &[String]) {
        self.lines.push(fields.join(":"));
        self.changed = true;
    }

    fn replace(&mut self, index: usize, fields: &[String]) {
        self.lines[index] = fields.join(":");
        self.changed = true;
    }

    fn write_if_changed(&mut self) -> Result<(), String> {
        if !self.changed {
            return Ok(());
        }
        let parent = self
            .path
            .parent()
            .ok_or_else(|| format!("account database has no parent: {}", self.path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        let temporary = parent.join(format!(
            ".fractald-sysusers.{}.{}",
            std::process::id(),
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("db")
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(self.mode)
            .open(&temporary)
            .map_err(|error| format!("cannot create {}: {error}", temporary.display()))?;
        let mut content = self.lines.join("\n");
        if !content.is_empty() {
            content.push('\n');
        }
        let result = (|| {
            file.write_all(content.as_bytes())
                .map_err(|error| format!("cannot write {}: {error}", temporary.display()))?;
            file.sync_all()
                .map_err(|error| format!("cannot sync {}: {error}", temporary.display()))?;
            fs::set_permissions(&temporary, fs::Permissions::from_mode(self.mode)).map_err(
                |error| format!("cannot set permissions on {}: {error}", temporary.display()),
            )?;
            fs::rename(&temporary, &self.path)
                .map_err(|error| format!("cannot replace {}: {error}", self.path.display()))?;
            self.exists = true;
            self.changed = false;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

fn print_help() {
    println!(
        "systemd-sysusers (FractalD)\n\nCreates system users and groups from sysusers.d configuration.\n\nUsage: systemd-sysusers [OPTIONS] [CONFIGURATION FILE...]\n\n  --root=PATH       operate below an alternate filesystem root\n  --replace=PATH    replace a configuration file with supplied input\n  --inline          treat positional arguments as configuration lines\n  --dry-run         report changes without writing account databases\n  --cat-config      print selected configuration files\n  --tldr            print non-comment configuration lines\n  --version         show the version"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_quoted_fields_and_comments() {
        let fields = split_fields(r#"u demo - "Demo User" /srv/demo # comment"#).expect("fields");
        assert_eq!(fields, ["u", "demo", "-", "Demo User", "/srv/demo"]);
    }

    #[test]
    fn allocates_ranges_from_high_to_low() {
        let mut database = Database {
            root: PathBuf::from("/"),
            passwd: TextDatabase {
                path: PathBuf::new(),
                lines: vec!["taken:x:2001:2001::/:/usr/sbin/nologin".to_owned()],
                exists: true,
                mode: 0o644,
                changed: false,
            },
            group: TextDatabase {
                path: PathBuf::new(),
                lines: Vec::new(),
                exists: false,
                mode: 0o644,
                changed: false,
            },
            shadow: TextDatabase {
                path: PathBuf::new(),
                lines: Vec::new(),
                exists: false,
                mode: 0,
                changed: false,
            },
            gshadow: TextDatabase {
                path: PathBuf::new(),
                lines: Vec::new(),
                exists: false,
                mode: 0,
                changed: false,
            },
            ranges: vec![(2000, 2002)],
        };
        assert_eq!(database.allocate_id().expect("free ID"), 2002);
        database.create_group("demo", 2002, true).expect("group");
        assert_eq!(database.allocate_id().expect("next free ID"), 2000);
    }

    #[test]
    fn validates_account_names() {
        assert!(validate_name("_service-1").is_ok());
        assert!(validate_name("1service").is_err());
        assert!(validate_name("service.name").is_err());
    }
}
