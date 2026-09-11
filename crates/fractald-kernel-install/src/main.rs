use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

#[derive(Clone, Debug, Default)]
struct Options {
    root: PathBuf,
    boot_path: Option<PathBuf>,
    entry_token: Option<String>,
    layout: Option<String>,
    make_entry_directory: Option<bool>,
    entry_type: Option<String>,
    verbose: u8,
    json: Option<String>,
}

#[derive(Clone, Debug)]
struct Context {
    options: Options,
    boot_root: PathBuf,
    entry_token: String,
    layout: String,
    machine_id: Option<String>,
}

fn main() -> ExitCode {
    match run(env::args()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("kernel-install: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(mut arguments: impl Iterator<Item = String>) -> Result<(), String> {
    let program = arguments
        .next()
        .unwrap_or_else(|| "kernel-install".to_owned());
    let program_name = Path::new(&program)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("kernel-install");
    let mut options = Options {
        root: PathBuf::from("/"),
        ..Options::default()
    };
    let mut command = None;
    let mut operands = Vec::new();
    while let Some(argument) = arguments.next() {
        if parse_option(&argument, &mut arguments, &mut options)? {
            continue;
        }
        if command.is_none() {
            command = Some(argument);
        } else {
            operands.push(argument);
        }
    }

    if program_name == "installkernel" {
        let version =
            command.ok_or_else(|| "installkernel requires a kernel version".to_owned())?;
        let image = operands
            .first()
            .cloned()
            .ok_or_else(|| "installkernel requires a kernel image".to_owned())?;
        return add(&options, &[version, image]);
    }
    let Some(command) = command else {
        print_help();
        return Ok(());
    };
    match command.as_str() {
        "add" => add(&options, &operands),
        "add-all" => add_all(&options, &operands),
        "remove" => remove(&options, &operands),
        "inspect" => inspect(&options, &operands),
        "list" => list(&options, &operands),
        "help" | "--help" => {
            print_help();
            Ok(())
        }
        "--version" | "version" => {
            println!("kernel-install (FractalD) {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unsupported command {other}")),
    }
}

fn parse_option(
    argument: &str,
    arguments: &mut impl Iterator<Item = String>,
    options: &mut Options,
) -> Result<bool, String> {
    let mut value = |name: &str| {
        arguments
            .next()
            .ok_or_else(|| format!("{name} requires a value"))
    };
    match argument {
        "--help" | "-h" => {
            print_help();
            std::process::exit(0);
        }
        "--version" => {
            println!("kernel-install (FractalD) {}", env!("CARGO_PKG_VERSION"));
            std::process::exit(0);
        }
        "-v" | "--verbose" => options.verbose = options.verbose.saturating_add(1),
        "--no-pager" | "--no-legend" => {}
        "--root" => options.root = PathBuf::from(value("--root")?),
        "--boot-path" | "--esp-path" => {
            options.boot_path = Some(PathBuf::from(value(argument)?));
        }
        "--entry-token" => options.entry_token = Some(value("--entry-token")?),
        "--entry-type" => options.entry_type = Some(value("--entry-type")?),
        "--make-entry-directory" => {
            options.make_entry_directory = Some(parse_yes_no(&value(argument)?)?);
        }
        "--json" => options.json = Some(value("--json")?),
        value if value.starts_with("--root=") => {
            options.root = PathBuf::from(value.trim_start_matches("--root="));
        }
        value if value.starts_with("--boot-path=") || value.starts_with("--esp-path=") => {
            options.boot_path = Some(PathBuf::from(value.split_once('=').expect("equals").1));
        }
        value if value.starts_with("--entry-token=") => {
            options.entry_token = Some(value.trim_start_matches("--entry-token=").to_owned());
        }
        value if value.starts_with("--entry-type=") => {
            options.entry_type = Some(value.trim_start_matches("--entry-type=").to_owned());
        }
        value if value.starts_with("--make-entry-directory=") => {
            options.make_entry_directory = Some(parse_yes_no(
                value.trim_start_matches("--make-entry-directory="),
            )?);
        }
        value if value.starts_with("--json=") => {
            options.json = Some(value.trim_start_matches("--json=").to_owned());
        }
        value if value.starts_with("--image") || value.starts_with("--image-policy") => {
            return Err(
                "disk-image operation is unavailable in the native filesystem tool".to_owned(),
            );
        }
        value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
        _ => return Ok(false),
    }
    Ok(true)
}

fn parse_yes_no(value: &str) -> Result<bool, String> {
    match value {
        "yes" | "true" | "1" => Ok(true),
        "no" | "false" | "0" => Ok(false),
        "auto" => Ok(true),
        other => Err(format!("expected yes, no, or auto; got {other}")),
    }
}

fn context(options: &Options) -> Result<Context, String> {
    let root = &options.root;
    let boot_root = if let Some(path) = &options.boot_path {
        rooted_path(root, path)
    } else {
        ["/efi", "/boot", "/boot/efi"]
            .into_iter()
            .map(|path| rooted_path(root, Path::new(path)))
            .find(|path| path.join("loader/entries").is_dir())
            .unwrap_or_else(|| rooted_path(root, Path::new("/boot")))
    };
    let config = config_values(root);
    let layout = options
        .layout
        .clone()
        .or_else(|| env::var("KERNEL_INSTALL_LAYOUT").ok())
        .or_else(|| config.get("layout").cloned())
        .unwrap_or_else(|| {
            if boot_root.join("loader/entries").is_dir() {
                "bls".to_owned()
            } else {
                "other".to_owned()
            }
        });
    let machine_id = read_machine_id(root);
    let entry_token = entry_token(options, root, &config, machine_id.as_deref())?;
    Ok(Context {
        options: options.clone(),
        boot_root,
        entry_token,
        layout,
        machine_id,
    })
}

fn config_values(root: &Path) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    for path in ["/usr/lib/kernel/install.conf", "/etc/kernel/install.conf"] {
        let path = rooted_path(root, Path::new(path));
        let Ok(contents) = fs::read_to_string(path) else {
            continue;
        };
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            values.insert(
                key.trim().to_owned(),
                value.trim().trim_matches('"').to_owned(),
            );
        }
    }
    values
}

fn entry_token(
    options: &Options,
    root: &Path,
    config: &BTreeMap<String, String>,
    machine_id: Option<&str>,
) -> Result<String, String> {
    let requested = options
        .entry_token
        .clone()
        .or_else(|| env::var("KERNEL_INSTALL_ENTRY_TOKEN").ok())
        .or_else(|| config.get("entry_token").cloned())
        .unwrap_or_else(|| "auto".to_owned());
    let token = match requested.as_str() {
        "auto" => fs::read_to_string(rooted_path(root, Path::new("/etc/kernel/entry-token")))
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .or_else(|| machine_id.map(str::to_owned))
            .or_else(|| os_release(root).get("IMAGE_ID").cloned())
            .or_else(|| os_release(root).get("ID").cloned())
            .unwrap_or_else(|| "fractald".to_owned()),
        "machine-id" => machine_id
            .map(str::to_owned)
            .ok_or_else(|| "machine-id is unavailable".to_owned())?,
        "os-id" => os_release(root)
            .get("ID")
            .cloned()
            .ok_or_else(|| "os-release has no ID".to_owned())?,
        "os-image-id" => os_release(root)
            .get("IMAGE_ID")
            .cloned()
            .ok_or_else(|| "os-release has no IMAGE_ID".to_owned())?,
        value if value.starts_with("literal:") => value.trim_start_matches("literal:").to_owned(),
        value => value.to_owned(),
    };
    validate_token(&token)?;
    Ok(token)
}

fn add(options: &Options, operands: &[String]) -> Result<(), String> {
    let context = context(options)?;
    let (version, kernel_image, supplied_initrd) = add_arguments(&context, operands)?;
    let make_entry = context
        .options
        .make_entry_directory
        .unwrap_or(context.layout == "bls");
    let entry_directory = context.boot_root.join(&context.entry_token).join(&version);
    if make_entry {
        fs::create_dir_all(&entry_directory)
            .map_err(|error| format!("cannot create {}: {error}", entry_directory.display()))?;
    }

    let mut initrds = supplied_initrd;
    if initrds.is_empty() {
        if let Some(candidate) = adjacent_initrd(&kernel_image) {
            initrds.push(candidate);
        } else if context.layout == "bls" {
            let generated = entry_directory.join("initrd");
            if generate_initrd(&context, &version, &kernel_image, &generated)? {
                initrds.push(generated);
            }
        }
    }

    if context.layout == "bls" && make_entry {
        copy_image(&kernel_image, &entry_directory.join("linux"))?;
        let mut initrd_names = Vec::new();
        for initrd in &initrds {
            let name = initrd
                .file_name()
                .ok_or_else(|| format!("invalid initrd path {}", initrd.display()))?;
            copy_image(initrd, &entry_directory.join(name))?;
            initrd_names.push(name.to_string_lossy().into_owned());
        }
        write_loader_entry(&context, &version, &initrd_names)?;
    } else {
        fs::create_dir_all(&context.boot_root)
            .map_err(|error| format!("cannot create {}: {error}", context.boot_root.display()))?;
        copy_image(
            &kernel_image,
            &context.boot_root.join(format!("vmlinuz-{version}")),
        )?;
        for initrd in &initrds {
            if let Some(name) = initrd.file_name() {
                copy_image(initrd, &context.boot_root.join(name))?;
            }
        }
    }
    run_module_index(&context, "add", &version)?;
    run_admin_plugins(&context, "add", &version, &kernel_image, &initrds)?;
    if context.options.verbose > 0 {
        eprintln!(
            "installed kernel {version} in {}",
            context.boot_root.display()
        );
    }
    Ok(())
}

fn add_all(options: &Options, operands: &[String]) -> Result<(), String> {
    if !operands.is_empty() {
        return Err("add-all does not accept positional arguments".to_owned());
    }
    if options.root != Path::new("/") {
        return Err("add-all does not accept --root".to_owned());
    }
    let modules = rooted_path(&options.root, Path::new("/usr/lib/modules"));
    let entries = fs::read_dir(&modules)
        .map_err(|error| format!("cannot read {}: {error}", modules.display()))?;
    let mut versions = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate modules: {error}"))?;
        if entry
            .file_type()
            .map_err(|error| error.to_string())?
            .is_dir()
            && entry.path().join("vmlinuz").is_file()
        {
            versions.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    versions.sort();
    for version in versions {
        add(options, &[version, String::new()])?;
    }
    Ok(())
}

fn remove(options: &Options, operands: &[String]) -> Result<(), String> {
    if operands.len() != 1 {
        return Err("remove requires exactly one kernel version".to_owned());
    }
    validate_version(&operands[0])?;
    let context = context(options)?;
    let version = &operands[0];
    let entry_directory = context.boot_root.join(&context.entry_token).join(version);
    let image = rooted_path(
        &context.options.root,
        Path::new(&format!("/usr/lib/modules/{version}/vmlinuz")),
    );
    run_module_index(&context, "remove", version)?;
    run_admin_plugins(&context, "remove", version, &image, &[])?;
    let loader_entry = context
        .boot_root
        .join("loader/entries")
        .join(format!("{}-{version}.conf", context.entry_token));
    remove_path(&loader_entry)?;
    remove_path(&entry_directory)?;
    if context.layout != "bls" {
        remove_path(&context.boot_root.join(format!("vmlinuz-{version}")))?;
    }
    Ok(())
}

fn inspect(options: &Options, operands: &[String]) -> Result<(), String> {
    let context = context(options)?;
    let (version, image, initrds) = if operands.is_empty() {
        (String::new(), PathBuf::new(), Vec::new())
    } else {
        add_arguments(&context, operands)?
    };
    let mut values = BTreeMap::new();
    values.insert(
        "KERNEL_INSTALL_BOOT_ROOT",
        context.boot_root.display().to_string(),
    );
    values.insert("KERNEL_INSTALL_ENTRY_TOKEN", context.entry_token.clone());
    values.insert("KERNEL_INSTALL_LAYOUT", context.layout.clone());
    values.insert(
        "KERNEL_INSTALL_MACHINE_ID",
        context.machine_id.unwrap_or_default(),
    );
    values.insert("KERNEL_VERSION", version);
    values.insert("KERNEL_IMAGE", image.display().to_string());
    values.insert(
        "INITRD",
        initrds
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(" "),
    );
    if options.json.as_deref().is_some_and(|value| value != "off") {
        print_json(&values);
    } else {
        for (key, value) in values {
            println!("{key}={value}");
        }
    }
    Ok(())
}

fn list(options: &Options, operands: &[String]) -> Result<(), String> {
    if !operands.is_empty() {
        return Err("list does not accept positional arguments".to_owned());
    }
    let modules = rooted_path(&options.root, Path::new("/usr/lib/modules"));
    let mut versions = Vec::new();
    if let Ok(entries) = fs::read_dir(&modules) {
        for entry in entries {
            let entry = entry.map_err(|error| format!("cannot enumerate modules: {error}"))?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                let version = entry.file_name().to_string_lossy().into_owned();
                versions.push((version, entry.path().join("vmlinuz").is_file()));
            }
        }
    }
    versions.sort_by(|left, right| left.0.cmp(&right.0));
    for (version, installed) in versions {
        println!(
            "{} {}",
            version,
            if installed { "installed" } else { "missing" }
        );
    }
    Ok(())
}

fn add_arguments(
    context: &Context,
    operands: &[String],
) -> Result<(String, PathBuf, Vec<PathBuf>), String> {
    let version = operands
        .first()
        .filter(|value| !value.is_empty() && value.as_str() != "-")
        .cloned()
        .unwrap_or_else(uname_release);
    validate_version(&version)?;
    let image = operands
        .get(1)
        .filter(|value| !value.is_empty() && value.as_str() != "-")
        .map(|value| rooted_path(&context.options.root, Path::new(value)))
        .unwrap_or_else(|| {
            rooted_path(
                &context.options.root,
                Path::new(&format!("/usr/lib/modules/{version}/vmlinuz")),
            )
        });
    if !image.is_file() {
        return Err(format!("kernel image does not exist: {}", image.display()));
    }
    let initrds = operands
        .iter()
        .skip(2)
        .map(|value| rooted_path(&context.options.root, Path::new(value)))
        .collect::<Vec<_>>();
    for initrd in &initrds {
        if !initrd.is_file() {
            return Err(format!("initrd does not exist: {}", initrd.display()));
        }
    }
    Ok((version, image, initrds))
}

fn adjacent_initrd(kernel_image: &Path) -> Option<PathBuf> {
    let parent = kernel_image.parent()?;
    for name in ["initrd", "initramfs.img"] {
        let candidate = parent.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn generate_initrd(
    context: &Context,
    version: &str,
    kernel_image: &Path,
    destination: &Path,
) -> Result<bool, String> {
    if env::var_os("FRACTALD_KERNEL_INSTALL_GENERATE_INITRD").is_some_and(|value| value == "0") {
        return Ok(false);
    }
    let Some(dracut) = find_program("dracut") else {
        return Ok(false);
    };
    if context.options.root != Path::new("/") {
        return Ok(false);
    }
    if context.options.verbose > 0 {
        eprintln!(
            "{} -f {} --kver {}",
            dracut.display(),
            destination.display(),
            version
        );
    }
    let mut command = Command::new(dracut);
    command.args(["-f", &destination.to_string_lossy(), "--kver", version]);
    command.env("KERNEL_IMAGE", kernel_image);
    let status = command
        .status()
        .map_err(|error| format!("cannot launch dracut: {error}"))?;
    if status.success() {
        Ok(true)
    } else {
        Err(format!("dracut exited with {status}"))
    }
}

fn run_module_index(context: &Context, operation: &str, version: &str) -> Result<(), String> {
    if env::var_os("FRACTALD_KERNEL_INSTALL_DEPMOD").is_some_and(|value| value == "0") {
        return Ok(());
    }
    let module_directory = ["/usr/lib/modules", "/lib/modules"]
        .into_iter()
        .map(|directory| rooted_path(&context.options.root, Path::new(directory)).join(version))
        .find(|directory| directory.is_dir());
    let Some(module_directory) = module_directory else {
        return Ok(());
    };

    if operation == "remove" {
        if env::var_os("KERNEL_INSTALL_BOOT_ENTRY_TYPE").is_some_and(|value| !value.is_empty())
            && module_directory.join("kernel").is_dir()
        {
            return Ok(());
        }
        for name in [
            "modules.alias",
            "modules.alias.bin",
            "modules.builtin.bin",
            "modules.builtin.alias.bin",
            "modules.dep",
            "modules.dep.bin",
            "modules.devname",
            "modules.softdep",
            "modules.weakdep",
            "modules.symbols",
            "modules.symbols.bin",
        ] {
            match fs::remove_file(module_directory.join(name)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "cannot remove {}: {error}",
                        module_directory.join(name).display()
                    ));
                }
            }
        }
        return Ok(());
    }

    if !module_directory.join("kernel").is_dir() {
        return Ok(());
    }
    let depmod = env::var_os("FRACTALD_KERNEL_INSTALL_DEPMOD")
        .map(PathBuf::from)
        .filter(|path| path.as_os_str() != "0")
        .or_else(|| find_program("depmod"));
    let Some(depmod) = depmod else {
        return Ok(());
    };
    if context.options.verbose > 0 {
        eprintln!(
            "{} -a{} {}",
            depmod.display(),
            if context.options.root == Path::new("/") {
                String::new()
            } else {
                format!(" -b {}", context.options.root.display())
            },
            version
        );
    }
    let mut command = Command::new(&depmod);
    command.arg("-a");
    if context.options.root != Path::new("/") {
        command.args(["-b", &context.options.root.to_string_lossy()]);
    }
    let status = command
        .arg(version)
        .status()
        .map_err(|error| format!("cannot launch {}: {error}", depmod.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", depmod.display()))
    }
}

fn write_loader_entry(context: &Context, version: &str, initrds: &[String]) -> Result<(), String> {
    let entries = context.boot_root.join("loader/entries");
    fs::create_dir_all(&entries)
        .map_err(|error| format!("cannot create {}: {error}", entries.display()))?;
    let entry_path = entries.join(format!("{}-{version}.conf", context.entry_token));
    let pretty_name = os_release(&context.options.root)
        .get("PRETTY_NAME")
        .cloned()
        .unwrap_or_else(|| format!("Linux {version}"));
    let mut contents = String::new();
    contents.push_str("# Boot Loader Specification type#1 entry\n");
    contents.push_str(&format!("title      {pretty_name}\nversion    {version}\n"));
    if context
        .machine_id
        .as_deref()
        .is_some_and(|machine_id| machine_id == context.entry_token)
    {
        contents.push_str(&format!("machine-id {}\n", context.entry_token));
    }
    let options = kernel_cmdline(&context.options.root);
    if !options.is_empty() {
        contents.push_str(&format!("options    {options}\n"));
    }
    contents.push_str(&format!(
        "linux      /{}/{}/linux\n",
        context.entry_token, version
    ));
    for initrd in initrds {
        contents.push_str(&format!(
            "initrd     /{}/{}/{}\n",
            context.entry_token, version, initrd
        ));
    }
    fs::write(&entry_path, contents)
        .map_err(|error| format!("cannot write {}: {error}", entry_path.display()))
}

fn kernel_cmdline(root: &Path) -> String {
    for path in ["/etc/kernel/cmdline", "/usr/lib/kernel/cmdline"] {
        if let Ok(contents) = fs::read_to_string(rooted_path(root, Path::new(path))) {
            return contents
                .split_whitespace()
                .filter(|word| !word.starts_with('#'))
                .collect::<Vec<_>>()
                .join(" ");
        }
    }
    if root == Path::new("/") {
        fs::read_to_string("/proc/cmdline")
            .unwrap_or_default()
            .split_whitespace()
            .filter(|word| !word.starts_with("BOOT_IMAGE=") && !word.starts_with("initrd="))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        String::new()
    }
}

fn run_admin_plugins(
    context: &Context,
    operation: &str,
    version: &str,
    image: &Path,
    initrds: &[PathBuf],
) -> Result<(), String> {
    if env::var_os("FRACTALD_KERNEL_INSTALL_PLUGINS").is_some_and(|value| value == "0") {
        return Ok(());
    }
    let plugins = install_plugins(&context.options.root)?;
    let entry_directory = context.boot_root.join(&context.entry_token).join(version);
    for plugin in plugins {
        let mut command = Command::new(&plugin);
        command
            .arg(operation)
            .arg(version)
            .arg(&entry_directory)
            .arg(image);
        command.args(initrds);
        command.env("KERNEL_INSTALL_BOOT_ROOT", &context.boot_root);
        command.env("KERNEL_INSTALL_ENTRY_TOKEN", &context.entry_token);
        command.env("KERNEL_INSTALL_LAYOUT", &context.layout);
        command.env("KERNEL_INSTALL_ROOT", &context.options.root);
        command.env(
            "KERNEL_INSTALL_CONF_ROOT",
            rooted_path(&context.options.root, Path::new("/etc/kernel")),
        );
        command.env("KERNEL_INSTALL_STAGING_AREA", &entry_directory);
        command.env(
            "KERNEL_INSTALL_VERBOSE",
            context.options.verbose.to_string(),
        );
        if let Some(entry_type) = &context.options.entry_type {
            command.env("KERNEL_INSTALL_BOOT_ENTRY_TYPE", entry_type);
        }
        let status = command
            .status()
            .map_err(|error| format!("cannot launch {}: {error}", plugin.display()))?;
        if status.code() == Some(77) {
            break;
        }
        if !status.success() {
            return Err(format!("{} exited with {status}", plugin.display()));
        }
    }
    Ok(())
}

fn install_plugins(root: &Path) -> Result<Vec<PathBuf>, String> {
    let directories = [
        "/etc/kernel/install.d",
        "/run/kernel/install.d",
        "/usr/local/lib/kernel/install.d",
        "/usr/lib/kernel/install.d",
        "/lib/kernel/install.d",
    ];
    let mut selected = BTreeMap::<String, Option<PathBuf>>::new();
    for directory in directories {
        let directory = rooted_path(root, Path::new(directory));
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("cannot enumerate {}: {error}", directory.display()));
            }
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "install")
            {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid kernel install plugin path {}", path.display()))?
                .to_owned();
            if selected.contains_key(&name) {
                continue;
            }
            if is_plugin_mask(root, &path) {
                selected.insert(name, None);
            } else if path.is_file() {
                selected.insert(name, Some(path));
            }
        }
    }
    Ok(selected.into_values().flatten().collect())
}

fn is_plugin_mask(root: &Path, path: &Path) -> bool {
    let Ok(target) = fs::read_link(path) else {
        return false;
    };
    if target == Path::new("/dev/null") {
        return true;
    }
    let Ok(path) = fs::canonicalize(path) else {
        return false;
    };
    fs::canonicalize(rooted_path(root, Path::new("/dev/null")))
        .is_ok_and(|null_path| path == null_path)
}

fn remove_path(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
    .map_err(|error| format!("cannot remove {}: {error}", path.display()))
}

fn copy_image(source: &Path, destination: &Path) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("destination has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    fs::copy(source, destination).map_err(|error| {
        format!(
            "cannot copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(())
}

fn read_machine_id(root: &Path) -> Option<String> {
    let value = fs::read_to_string(rooted_path(root, Path::new("/etc/machine-id"))).ok()?;
    let value = value.trim().to_ascii_lowercase();
    (value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(value)
}

fn os_release(root: &Path) -> BTreeMap<String, String> {
    for path in ["/etc/os-release", "/usr/lib/os-release"] {
        let Ok(contents) = fs::read_to_string(rooted_path(root, Path::new(path))) else {
            continue;
        };
        let mut values = BTreeMap::new();
        for line in contents.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            values.insert(key.to_owned(), value.trim_matches('"').to_owned());
        }
        return values;
    }
    BTreeMap::new()
}

fn rooted_path(root: &Path, path: &Path) -> PathBuf {
    if root == Path::new("/") {
        path.to_owned()
    } else if path.is_absolute() {
        root.join(path.strip_prefix("/").expect("absolute path"))
    } else {
        root.join(path)
    }
}

fn validate_token(token: &str) -> Result<(), String> {
    if token.is_empty()
        || token == "."
        || token == ".."
        || token.contains('/')
        || token.chars().any(char::is_whitespace)
    {
        return Err(format!("invalid entry token {token:?}"));
    }
    Ok(())
}

fn validate_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version == "."
        || version == ".."
        || version.contains('/')
        || version.chars().any(char::is_whitespace)
    {
        return Err(format!("invalid kernel version {version:?}"));
    }
    Ok(())
}

fn uname_release() -> String {
    Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn find_program(name: &str) -> Option<PathBuf> {
    env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(PathBuf::from)
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

fn print_json(values: &BTreeMap<&str, String>) {
    print!("{{");
    for (index, (key, value)) in values.iter().enumerate() {
        if index > 0 {
            print!(",");
        }
        print!("\"{}\":\"{}\"", json_escape(key), json_escape(value));
    }
    println!("}}");
}

fn json_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn print_help() {
    println!(
        "kernel-install (FractalD)\n\nUsage: kernel-install [OPTIONS] COMMAND ...\n\n  add VERSION IMAGE [INITRD...]  install a kernel and BLS entry\n  add-all                         install every /usr/lib/modules kernel\n  remove VERSION                  remove a kernel and BLS entry\n  inspect [VERSION IMAGE ...]     show effective install parameters\n  list                            list installed module images\n  --root=PATH                     operate below an alternate root\n  --boot-path=PATH                select the boot filesystem\n  --entry-token=TOKEN              select the BLS entry token\n  --entry-type=TYPE               pass type1/type2 selection to admin hooks\n  --make-entry-directory=yes|no   control BLS resource directories\n  --verbose                       print install actions\n  --version                       show the version"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_tokens_and_versions() {
        assert!(validate_token("valid-token").is_ok());
        assert!(validate_token("../boot").is_err());
        assert!(validate_version("6.1.0").is_ok());
        assert!(validate_version("../kernel").is_err());
    }

    #[test]
    fn parses_yes_no_and_auto() {
        assert!(parse_yes_no("yes").expect("yes"));
        assert!(!parse_yes_no("no").expect("no"));
        assert!(parse_yes_no("auto").expect("auto"));
        assert!(parse_yes_no("maybe").is_err());
    }
}
