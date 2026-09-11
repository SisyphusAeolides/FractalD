use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use fractald_control::{
    Request, Response, RuntimePaths, ServiceStatus, ShutdownAction, StatePaths,
};

const WAIT_ATTEMPTS: usize = 200;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("systemctl: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut args = env::args_os();
    let _program = args.next();
    let mut command = None;
    let mut forwarded = Vec::new();
    let mut quiet = false;
    let mut no_legend = false;
    let mut now = false;
    let mut no_block = false;
    let mut wait_requested = false;
    let mut marked = false;
    let mut global = false;
    let mut preset_mode = "full".to_owned();
    let mut runtime = false;
    let mut properties = Vec::new();
    let mut value_only = false;

    while let Some(raw_value) = args.next() {
        let value = raw_value
            .into_string()
            .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
        if value == "-p" || value == "--property" {
            let property = args
                .next()
                .ok_or_else(|| format!("{value} requires a property name"))?
                .into_string()
                .map_err(|_| "property names must be valid UTF-8".to_owned())?;
            add_properties(&mut properties, &property);
            continue;
        }
        if let Some(property) = value.strip_prefix("--property=") {
            add_properties(&mut properties, property);
            continue;
        }
        if value == "--value" {
            value_only = true;
            continue;
        }
        if value == "--now" {
            now = true;
            continue;
        }
        if value == "--no-block" {
            no_block = true;
            continue;
        }
        if value == "--wait" {
            wait_requested = true;
            continue;
        }
        if value == "--marked" {
            marked = true;
            continue;
        }
        if value == "--global" {
            global = true;
            continue;
        }
        if value == "--runtime" {
            runtime = true;
            continue;
        }
        if value == "-M" || value == "--machine" {
            let _ = args
                .next()
                .ok_or_else(|| format!("{value} requires a machine or user manager"))?;
            continue;
        }
        if let Some(machine) = value.strip_prefix("--machine=") {
            if machine.is_empty() {
                return Err("--machine requires a machine or user manager".to_owned());
            }
            continue;
        }
        if value == "--preset-mode" {
            preset_mode = args
                .next()
                .ok_or_else(|| "--preset-mode requires a value".to_owned())?
                .into_string()
                .map_err(|_| "preset mode must be valid UTF-8".to_owned())?;
            continue;
        }
        if let Some(mode) = value.strip_prefix("--preset-mode=") {
            preset_mode = mode.to_owned();
            continue;
        }
        if value == "--force" {
            continue;
        }
        if is_global_option(&value) {
            quiet |= value == "--quiet" || value == "-q";
            no_legend |= value == "--no-legend";
            continue;
        }
        if command.is_none() {
            command = Some(value);
        } else {
            forwarded.push(value);
        }
    }

    let command = command.as_deref().unwrap_or("help");
    if command == "--version" || command == "version" {
        println!(
            "systemctl compatibility for FractalD {}",
            env!("CARGO_PKG_VERSION")
        );
        return Ok(0);
    }
    if command == "help" || command == "--help" {
        print_help();
        return Ok(0);
    }
    if now && !matches!(command, "enable" | "disable") {
        return Err("--now is supported with enable and disable only".to_owned());
    }
    if marked && matches!(command, "reload-or-restart" | "try-reload-or-restart") {
        return apply_marked(
            &forwarded,
            quiet,
            no_block,
            wait_requested,
            command == "try-reload-or-restart",
        );
    }
    if command == "is-enabled" {
        if forwarded.is_empty() {
            return Err("is-enabled requires at least one unit".to_owned());
        }
        return is_enabled_units(&forwarded, quiet);
    }

    let mut translated = Vec::new();
    match command {
        "isolate" => {
            if forwarded.len() != 1 {
                return Err("isolate requires exactly one target unit".to_owned());
            }
            translated.push("isolate".to_owned());
            translated.push(forwarded[0].clone());
        }
        "start"
        | "stop"
        | "restart"
        | "reload"
        | "status"
        | "enable"
        | "disable"
        | "is-active"
        | "try-restart"
        | "reset-failed"
        | "is-failed"
        | "reload-or-restart"
        | "try-reload-or-restart"
        | "mask"
        | "unmask" => {
            if matches!(command, "reload-or-restart" | "try-reload-or-restart") {
                if forwarded.is_empty() {
                    return Err(format!("{command} requires at least one unit"));
                }
                return invoke_units(
                    command,
                    &forwarded,
                    now,
                    quiet,
                    should_wait(command, now, no_block, wait_requested),
                );
            }
            if forwarded.len() == 1 && has_wildcard(&forwarded[0]) {
                return invoke_units(
                    command,
                    &forwarded,
                    now,
                    quiet,
                    should_wait(command, now, no_block, wait_requested),
                );
            }
            if command == "try-restart" && forwarded.is_empty() {
                return Err("try-restart requires at least one unit".to_owned());
            }
            if forwarded.len() > 1 {
                return invoke_units(
                    command,
                    &forwarded,
                    now,
                    quiet,
                    should_wait(command, now, no_block, wait_requested),
                );
            }
            if command == "try-restart"
                && !service_is_active(forwarded.first().expect("try-restart unit"))?
            {
                return Ok(0);
            }
            translated.push(if command == "try-restart" {
                "restart".to_owned()
            } else {
                command.to_owned()
            });
            if now {
                translated.push("--now".to_owned());
            }
            translated.extend(forwarded);
        }
        "daemon-reload" => translated.push("reload".to_owned()),
        "daemon-reexec" => return daemon_reexec(&forwarded),
        "list-units" => translated.push("list".to_owned()),
        "list-unit-files" => return list_unit_files(&forwarded, quiet, no_legend),
        "show" => {
            return show_units(&forwarded, &properties, value_only, quiet);
        }
        "set-property" => return set_properties(&forwarded, runtime, quiet),
        "preset" => {
            if forwarded.is_empty() {
                return Err("preset requires at least one unit".to_owned());
            }
            return apply_preset(
                &forwarded,
                &preset_mode,
                global,
                quiet,
                no_block,
                wait_requested,
            );
        }
        "preset-all" => {
            if !forwarded.is_empty() {
                return Err("preset-all does not accept unit arguments".to_owned());
            }
            return apply_preset_all(&preset_mode, global, quiet, no_block, wait_requested);
        }
        "poweroff" | "reboot" | "halt" => {
            return request_power_action(command, &forwarded, quiet);
        }
        "is-system-running" => {
            if !forwarded.is_empty() {
                return Err("is-system-running does not accept unit arguments".to_owned());
            }
            return system_running(quiet);
        }
        other => return Err(format!("unsupported command {other:?}")),
    }
    let code = invoke_fractalctl(&translated, quiet)?;
    if code != 0 {
        return Ok(code);
    }
    if forwarded_len(&translated) == 1 && should_wait(command, now, no_block, wait_requested) {
        wait_for_unit(
            translated.last().expect("translated unit argument"),
            if command == "try-restart" {
                "restart"
            } else {
                command
            },
        )?;
    }
    Ok(code)
}

fn add_properties(properties: &mut Vec<String>, value: &str) {
    properties.extend(
        value
            .split(',')
            .map(str::trim)
            .filter(|property| !property.is_empty())
            .map(str::to_owned),
    );
}

fn request_power_action(command: &str, forwarded: &[String], quiet: bool) -> Result<u8, String> {
    if !forwarded.is_empty() {
        return Err(format!("{command} does not accept unit arguments"));
    }
    let action = match command {
        "poweroff" => ShutdownAction::Poweroff,
        "reboot" => ShutdownAction::Reboot,
        "halt" => ShutdownAction::Halt,
        _ => return Err(format!("unsupported power action {command}")),
    };
    match RuntimePaths::from_environment().request(Request::Shutdown(action)) {
        Ok(Response::Stopping) => {
            if !quiet {
                println!("{command} requested");
            }
            Ok(0)
        }
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected {command} response: {response:?}")),
        Err(error) if is_not_running(&error) => Err("FractalD is not running".to_owned()),
        Err(error) => Err(format!("cannot contact FractalD: {error}")),
    }
}

fn daemon_reexec(arguments: &[String]) -> Result<u8, String> {
    if !arguments.is_empty() {
        return Err("daemon-reexec does not accept unit arguments".to_owned());
    }
    // FractalD keeps manager state in its own process and has no separate
    // systemd manager image to reexec. Package transactions use this command
    // as a best-effort refresh point, so a successful no-op preserves the
    // compatibility contract without coupling FractalD to systemd.
    Ok(0)
}

fn system_running(quiet: bool) -> Result<u8, String> {
    match RuntimePaths::from_environment().request(Request::Status) {
        Ok(Response::Status { .. }) => {
            if !quiet {
                println!("running");
            }
            Ok(0)
        }
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected manager response: {response:?}")),
        Err(error) if is_not_running(&error) => {
            if !quiet {
                println!("offline");
            }
            Ok(1)
        }
        Err(error) => Err(format!("cannot contact FractalD: {error}")),
    }
}

fn is_global_option(value: &str) -> bool {
    matches!(
        value,
        "--user"
            | "--system"
            | "--no-pager"
            | "--no-legend"
            | "--plain"
            | "--full"
            | "--quiet"
            | "-q"
            | "--all"
            | "--runtime"
            | "--no-reload"
            | "--no-warn"
            | "--no-ask-password"
            | "--marked"
    ) || value.starts_with("--root=")
        || value.starts_with("--machine=")
        || value.starts_with("--host=")
        || value.starts_with("--job-mode=")
        || value.starts_with("--type=")
        || value.starts_with("--state=")
        || value.starts_with("--legend=")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PresetAction {
    Enable,
    Disable,
    Ignore,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PresetRule {
    action: PresetAction,
    pattern: String,
}

fn apply_preset(
    requested: &[String],
    mode: &str,
    global: bool,
    quiet: bool,
    no_block: bool,
    wait_requested: bool,
) -> Result<u8, String> {
    let (rules, has_files) = load_preset_rules(global)?;
    let units = expand_preset_units(requested)?;
    let default_enable = !has_files;
    apply_preset_to_units(
        &units,
        &rules,
        default_enable,
        mode,
        quiet,
        no_block,
        wait_requested,
    )
}

fn apply_preset_all(
    mode: &str,
    global: bool,
    quiet: bool,
    no_block: bool,
    wait_requested: bool,
) -> Result<u8, String> {
    let (rules, has_files) = load_preset_rules(global)?;
    let directories = unit_directories();
    let mut units = discover_unit_file_names(&directories)?
        .into_iter()
        .filter(|name| is_presettable_unit_name(name))
        .collect::<Vec<_>>();
    units.extend(
        StatePaths::from_environment()
            .enabled_names()
            .map_err(|error| format!("cannot enumerate enabled units: {error}"))?,
    );
    units.sort();
    units.dedup();
    apply_preset_to_units(
        &units,
        &rules,
        !has_files,
        mode,
        quiet,
        no_block,
        wait_requested,
    )
}

fn apply_preset_to_units(
    units: &[String],
    rules: &[PresetRule],
    default_enable: bool,
    mode: &str,
    quiet: bool,
    no_block: bool,
    wait_requested: bool,
) -> Result<u8, String> {
    if !matches!(mode, "full" | "enable-only" | "disable-only") {
        return Err(format!(
            "unsupported --preset-mode={mode}; expected full, enable-only, or disable-only"
        ));
    }
    let wait = should_wait("enable", false, no_block, wait_requested);
    let mut result = 0;
    for unit in units {
        let action = rules
            .iter()
            .find(|rule| preset_rule_matches(&rule.pattern, unit))
            .map(|rule| rule.action)
            .or_else(|| default_enable.then_some(PresetAction::Enable));
        let Some(action) = action else {
            continue;
        };
        let action = match (mode, action) {
            ("enable-only", PresetAction::Disable) => PresetAction::Ignore,
            ("disable-only", PresetAction::Enable) => PresetAction::Ignore,
            (_, action) => action,
        };
        let command = match action {
            PresetAction::Enable => "enable",
            PresetAction::Disable => "disable",
            PresetAction::Ignore => continue,
        };
        let code = invoke_units(command, std::slice::from_ref(unit), false, quiet, wait)?;
        result = result.max(code);
    }
    Ok(result)
}

fn expand_preset_units(requested: &[String]) -> Result<Vec<String>, String> {
    let directories = unit_directories();
    let available = discover_unit_file_names(&directories)?;
    let mut units = Vec::new();
    for requested_unit in requested {
        if has_wildcard(requested_unit) {
            units.extend(
                available
                    .iter()
                    .filter(|unit| wildcard_match(requested_unit, unit))
                    .cloned(),
            );
        } else {
            units.push(requested_unit.clone());
        }
    }
    units.sort();
    units.dedup();
    Ok(units)
}

fn is_presettable_unit_name(name: &str) -> bool {
    [
        ".automount",
        ".busname",
        ".device",
        ".mount",
        ".path",
        ".scope",
        ".service",
        ".slice",
        ".socket",
        ".swap",
        ".target",
        ".timer",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn preset_rule_matches(pattern: &str, unit: &str) -> bool {
    if wildcard_match(pattern, unit) {
        return true;
    }
    let Some(prefix) = pattern.strip_suffix("@.service") else {
        return false;
    };
    unit.strip_prefix(&format!("{prefix}@"))
        .is_some_and(|instance| !instance.is_empty() && instance.ends_with(".service"))
}

fn load_preset_rules(global: bool) -> Result<(Vec<PresetRule>, bool), String> {
    let directories = preset_directories(global);
    let mut selected = BTreeMap::<String, Option<PathBuf>>::new();
    for directory in directories {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read preset directory {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot enumerate preset directory {}: {error}",
                    directory.display()
                )
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if !name.ends_with(".preset") {
                continue;
            }
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                format!("cannot inspect preset file {}: {error}", path.display())
            })?;
            if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path).map_err(|error| {
                    format!("cannot read preset mask {}: {error}", path.display())
                })?;
                if target == std::path::Path::new("/dev/null") {
                    selected.insert(name.to_owned(), None);
                }
            } else if metadata.is_file() {
                selected.insert(name.to_owned(), Some(path));
            }
        }
    }

    let has_files = selected.values().any(Option::is_some);
    let mut rules = Vec::new();
    for path in selected.into_values().flatten() {
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read preset file {}: {error}", path.display()))?;
        for line in source.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let fields = line.split_whitespace().collect::<Vec<_>>();
            let Some(action) = fields.first().and_then(|value| match *value {
                "enable" => Some(PresetAction::Enable),
                "disable" => Some(PresetAction::Disable),
                "ignore" => Some(PresetAction::Ignore),
                _ => None,
            }) else {
                continue;
            };
            let Some(pattern) = fields.get(1) else {
                continue;
            };
            rules.push(PresetRule {
                action,
                pattern: (*pattern).to_owned(),
            });
        }
    }
    Ok((rules, has_files))
}

fn preset_directories(global: bool) -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_PRESET_DIR") {
        return vec![PathBuf::from(path)];
    }
    let suffix = if global {
        "user-preset"
    } else {
        "system-preset"
    };
    [
        PathBuf::from(format!("/usr/lib/systemd/{suffix}")),
        PathBuf::from(format!("/usr/local/lib/systemd/{suffix}")),
        PathBuf::from(format!("/run/systemd/{suffix}")),
        PathBuf::from(format!("/etc/systemd/{suffix}")),
    ]
    .to_vec()
}

const NEEDS_RESTART: &str = "needs-restart";
const NEEDS_RELOAD: &str = "needs-reload";

fn marker_directory() -> PathBuf {
    RuntimePaths::from_environment().directory.join("markers")
}

fn set_properties(arguments: &[String], _runtime: bool, quiet: bool) -> Result<u8, String> {
    if arguments.len() < 2 {
        return Err("set-property requires a unit and at least one assignment".to_owned());
    }
    let unit = &arguments[0];
    if unit.is_empty() || unit.contains('/') {
        return Err("set-property received an invalid unit name".to_owned());
    }
    let directory = marker_directory();
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create marker directory {}: {error}",
            directory.display()
        )
    })?;
    for assignment in &arguments[1..] {
        let (property, value) = assignment
            .split_once('=')
            .ok_or_else(|| format!("set-property assignment lacks '=': {assignment}"))?;
        if property != "Markers" {
            return Err(format!("unsupported runtime property {property}"));
        }
        if value.is_empty() {
            clear_marker(unit, NEEDS_RESTART)?;
            clear_marker(unit, NEEDS_RELOAD)?;
            continue;
        }
        for marker in value.split([',', ' ']).filter(|value| !value.is_empty()) {
            let (operation, marker) = match marker.as_bytes().first() {
                Some(b'+') => (true, &marker[1..]),
                Some(b'-') => (false, &marker[1..]),
                _ => (true, marker),
            };
            if !matches!(marker, NEEDS_RESTART | NEEDS_RELOAD) {
                return Err(format!("unsupported marker {marker}"));
            }
            if operation {
                write_marker(unit, marker)?;
            } else {
                clear_marker(unit, marker)?;
            }
        }
    }
    if !quiet {
        println!("set-property applied to {unit}");
    }
    Ok(0)
}

fn marker_path(unit: &str, marker: &str) -> PathBuf {
    marker_directory().join(format!("{unit}.{marker}"))
}

fn write_marker(unit: &str, marker: &str) -> Result<(), String> {
    fs::write(marker_path(unit, marker), b"1\n")
        .map_err(|error| format!("cannot write {marker} marker for {unit}: {error}"))
}

fn clear_marker(unit: &str, marker: &str) -> Result<(), String> {
    match fs::remove_file(marker_path(unit, marker)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot clear {marker} marker for {unit}: {error}")),
    }
}

fn marked_units() -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let directory = marker_directory();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(format!(
                "cannot read marker directory {}: {error}",
                directory.display()
            ));
        }
    };
    let mut units = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("cannot enumerate markers: {error}"))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some((unit, marker)) = name.rsplit_once('.') else {
            continue;
        };
        if !matches!(marker, NEEDS_RESTART | NEEDS_RELOAD) || unit.is_empty() || unit.contains('/')
        {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
            format!("cannot inspect marker {}: {error}", entry.path().display())
        })?;
        if !metadata.is_file() {
            continue;
        }
        units
            .entry(unit.to_owned())
            .or_insert_with(BTreeSet::new)
            .insert(marker.to_owned());
    }
    Ok(units)
}

fn apply_marked(
    requested: &[String],
    quiet: bool,
    no_block: bool,
    wait_requested: bool,
    try_only: bool,
) -> Result<u8, String> {
    let requested = requested.iter().cloned().collect::<BTreeSet<_>>();
    let marked = marked_units()?;
    let wait = should_wait("reload-or-restart", false, no_block, wait_requested);
    let mut result = 0;
    for (unit, markers) in marked {
        if !requested.is_empty() && !requested.contains(&unit) {
            continue;
        }
        if try_only && !service_is_active(&unit)? {
            continue;
        }
        let needs_restart = markers.contains(NEEDS_RESTART);
        let needs_reload = markers.contains(NEEDS_RELOAD);
        let mut success = true;
        if needs_restart {
            success =
                invoke_units("restart", std::slice::from_ref(&unit), false, quiet, wait)? == 0;
        } else if needs_reload {
            let reload = invoke_units("reload", std::slice::from_ref(&unit), false, quiet, wait)?;
            if reload != 0 {
                success =
                    invoke_units("restart", std::slice::from_ref(&unit), false, quiet, wait)? == 0;
            }
        }
        result = result.max(u8::from(!success));
        if success {
            clear_marker(&unit, NEEDS_RESTART)?;
            clear_marker(&unit, NEEDS_RELOAD)?;
        }
    }
    Ok(result)
}

fn invoke_units(
    command: &str,
    units: &[String],
    now: bool,
    quiet: bool,
    wait: bool,
) -> Result<u8, String> {
    let mut result = 0;
    let units = expand_units(units)?;
    for unit in &units {
        let active = if matches!(
            command,
            "try-restart" | "reload-or-restart" | "try-reload-or-restart"
        ) {
            service_is_active(unit)?
        } else {
            false
        };
        if matches!(command, "try-restart" | "try-reload-or-restart") && !active {
            continue;
        }
        let translated_command = match command {
            "try-restart" => "restart",
            "reload-or-restart" | "try-reload-or-restart" if active => "reload",
            "reload-or-restart" => "restart",
            other => other,
        };
        let mut arguments = vec![translated_command.to_owned()];
        if now {
            arguments.push("--now".to_owned());
        }
        arguments.push(unit.clone());
        let mut wait_operation = translated_command;
        let mut code = if translated_command == "reload" && wait {
            invoke_reload_and_wait(unit, quiet)?
        } else {
            invoke_fractalctl(&arguments, quiet)?
        };
        if code != 0
            && matches!(command, "reload-or-restart" | "try-reload-or-restart")
            && translated_command == "reload"
        {
            let restart = vec!["restart".to_owned(), unit.clone()];
            code = invoke_fractalctl(&restart, quiet)?;
            wait_operation = "restart";
        }
        result = result.max(code);
        if code == 0 && wait {
            wait_for_unit(unit, wait_operation)?;
        }
    }
    Ok(result)
}

fn invoke_reload_and_wait(unit: &str, quiet: bool) -> Result<u8, String> {
    let paths = RuntimePaths::from_environment();
    match paths.request(Request::ReloadService(unit.to_owned())) {
        Ok(Response::Accepted {
            id,
            operation,
            name,
        }) => {
            if !quiet {
                println!("{operation} requested for {name} (transaction {id})");
            }
            Ok(u8::from(!wait_for_transaction(id)?))
        }
        Ok(Response::Error { .. }) => Ok(1),
        Ok(response) => Err(format!("unexpected reload response: {response:?}")),
        Err(error) if is_not_running(&error) => Err("FractalD is not running".to_owned()),
        Err(error) => Err(format!("cannot contact FractalD: {error}")),
    }
}

fn wait_for_transaction(id: u64) -> Result<bool, String> {
    let paths = RuntimePaths::from_environment();
    for _ in 0..WAIT_ATTEMPTS {
        match paths.request(Request::TransactionStatus(id)) {
            Ok(Response::Transaction(status)) => match status.state.as_str() {
                "pending" => thread::sleep(Duration::from_millis(25)),
                "done" => return Ok(true),
                "failed" | "unknown" => return Ok(false),
                _ => return Ok(false),
            },
            Ok(Response::Error { message }) => return Err(message),
            Ok(response) => return Err(format!("unexpected transaction response: {response:?}")),
            Err(error) if is_not_running(&error) => {
                return Err("FractalD is not running".to_owned());
            }
            Err(error) => return Err(format!("cannot inspect transaction {id}: {error}")),
        }
    }
    Err(format!(
        "transaction {id} did not complete within 5 seconds"
    ))
}

fn has_wildcard(value: &str) -> bool {
    value
        .bytes()
        .any(|character| matches!(character, b'*' | b'?'))
}

fn expand_units(units: &[String]) -> Result<Vec<String>, String> {
    let paths = RuntimePaths::from_environment();
    let mut expanded = Vec::new();
    for unit in units {
        if !has_wildcard(unit) {
            expanded.push(unit.clone());
            continue;
        }
        match paths.request(Request::List) {
            Ok(Response::Services { names }) => {
                expanded.extend(names.into_iter().filter(|name| wildcard_match(unit, name)));
            }
            Ok(Response::Error { message }) => return Err(message),
            Ok(response) => return Err(format!("unexpected list response: {response:?}")),
            Err(error) if is_not_running(&error) => {
                return Err("FractalD is not running".to_owned());
            }
            Err(error) => return Err(format!("cannot list units: {error}")),
        }
    }
    expanded.sort();
    expanded.dedup();
    Ok(expanded)
}

fn list_unit_files(patterns: &[String], quiet: bool, no_legend: bool) -> Result<u8, String> {
    let state_paths = StatePaths::from_environment();
    let directories = unit_directories();
    let mut names = BTreeSet::new();
    names.extend(discover_unit_file_names(&directories)?);
    names.extend(
        state_paths
            .enabled_names()
            .map_err(|error| format!("cannot enumerate enabled units: {error}"))?,
    );

    match RuntimePaths::from_environment().request(Request::List) {
        Ok(Response::Services { names: loaded }) => names.extend(loaded),
        Ok(Response::Error { message }) => return Err(message),
        Ok(response) => return Err(format!("unexpected list response: {response:?}")),
        Err(error) if is_not_running(&error) => {}
        Err(error) => return Err(format!("cannot list unit files: {error}")),
    }

    if !patterns.is_empty() {
        names.retain(|name| patterns.iter().any(|pattern| wildcard_match(pattern, name)));
    }
    if quiet {
        return Ok(0);
    }
    if !no_legend {
        println!("UNIT FILE\tSTATE");
    }
    for name in names {
        let state = if state_paths
            .is_masked(&name)
            .map_err(|error| format!("cannot inspect mask for {name}: {error}"))?
            || unit_file_is_masked(&directories, &name)?
        {
            "masked"
        } else if state_paths
            .is_enabled(&name)
            .map_err(|error| format!("cannot inspect enablement for {name}: {error}"))?
            || unit_file_is_enabled(&directories, &name)?
        {
            "enabled"
        } else {
            "disabled"
        };
        println!("{name}\t{state}");
    }
    Ok(0)
}

fn unit_directories() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_SERVICE_DIR") {
        if env::var_os("FRACTALD_GENERATOR_DIR").is_some() {
            let generated = env::var_os("FRACTALD_GENERATED_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| RuntimePaths::from_environment().directory.join("generator"));
            let generated_parent = generated
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_owned();
            let generated_name = generated
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("generator")
                .to_owned();
            return vec![
                generated_parent.join(format!("{generated_name}.late")),
                PathBuf::from(path),
                generated,
                generated_parent.join(format!("{generated_name}.early")),
            ];
        }
        return vec![PathBuf::from(path)];
    }
    let generated = env::var_os("FRACTALD_GENERATED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| RuntimePaths::from_environment().directory.join("generator"));
    let generated_parent = generated
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .to_owned();
    let generated_name = generated
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("generator");
    let generated_early = generated_parent.join(format!("{generated_name}.early"));
    let generated_late = generated_parent.join(format!("{generated_name}.late"));
    if fractald_platform::is_root() {
        vec![
            generated_late,
            PathBuf::from("/usr/lib/systemd/system"),
            PathBuf::from("/usr/local/lib/systemd/system"),
            PathBuf::from("/lib/systemd/system"),
            PathBuf::from("/run/systemd/system"),
            generated,
            PathBuf::from("/etc/systemd/system"),
            PathBuf::from("/etc/fractald/services"),
            generated_early,
        ]
    } else {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", fractald_platform::effective_uid()))
            });
        let mut directories = vec![
            generated_late,
            PathBuf::from("/usr/lib/systemd/user"),
            PathBuf::from("/usr/local/lib/systemd/user"),
            runtime.join("systemd/user"),
            generated,
        ];
        if let Some(config_home) = config_home {
            directories.push(config_home.join("systemd/user"));
            directories.push(config_home.join("fractald/services"));
        }
        directories.push(generated_early);
        directories
    }
}

fn discover_unit_file_names(directories: &[PathBuf]) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read unit directory {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot enumerate unit directory {}: {error}",
                    directory.display()
                )
            })?;
            let path = entry.path();
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if is_unit_file_name(name) {
                let metadata = fs::symlink_metadata(&path);
                if metadata.as_ref().is_ok_and(|metadata| {
                    metadata.file_type().is_symlink() || metadata.file_type().is_file()
                }) {
                    names.insert(name.to_owned());
                }
            }
            if let Some(base) = name.strip_suffix(".d") {
                if is_unit_file_name(base) && path.is_dir() {
                    names.insert(base.to_owned());
                }
            }
        }
    }
    Ok(names)
}

fn is_unit_file_name(name: &str) -> bool {
    [
        ".automount",
        ".busname",
        ".container",
        ".device",
        ".mount",
        ".netdev",
        ".network",
        ".path",
        ".scope",
        ".service",
        ".slice",
        ".socket",
        ".swap",
        ".target",
        ".timer",
        ".volume",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn unit_file_is_masked(directories: &[PathBuf], name: &str) -> Result<bool, String> {
    let candidates = unit_name_candidates(name);
    for directory in directories.iter().rev() {
        for candidate in &candidates {
            let path = directory.join(candidate);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let target = fs::read_link(&path).map_err(|error| {
                        format!("cannot read unit mask {}: {error}", path.display())
                    })?;
                    if target == std::path::Path::new("/dev/null") {
                        return Ok(true);
                    }
                    let resolved = if target.is_absolute() {
                        target
                    } else {
                        path.parent()
                            .unwrap_or_else(|| std::path::Path::new("."))
                            .join(target)
                    };
                    return match fs::canonicalize(&resolved) {
                        Ok(resolved) => Ok(resolved == std::path::Path::new("/dev/null")),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                        Err(error) => Err(format!(
                            "cannot resolve unit mask {}: {error}",
                            path.display()
                        )),
                    };
                }
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("cannot inspect {}: {error}", path.display()));
                }
            }
        }
    }
    Ok(false)
}

fn is_enabled_units(units: &[String], quiet: bool) -> Result<u8, String> {
    let units = expand_units(units)?;
    let state_paths = StatePaths::from_environment();
    let directories = unit_directories();
    let mut result = 0;
    for unit in units {
        let masked = state_paths
            .is_masked(&unit)
            .map_err(|error| format!("cannot inspect masked state for {unit}: {error}"))?
            || unit_file_is_masked(&directories, &unit)?;
        let enabled = !masked
            && (state_paths
                .is_enabled(&unit)
                .map_err(|error| format!("cannot inspect enabled state for {unit}: {error}"))?
                || unit_file_is_enabled(&directories, &unit)?);
        if !quiet {
            println!(
                "{}",
                if masked {
                    "masked"
                } else if enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            );
        }
        if !enabled {
            result = 1;
        }
    }
    Ok(result)
}

fn unit_file_is_enabled(directories: &[PathBuf], name: &str) -> Result<bool, String> {
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read unit directory {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let entry = entry.map_err(|error| {
                format!(
                    "cannot enumerate unit directory {}: {error}",
                    directory.display()
                )
            })?;
            let relationship = entry.file_name();
            let Some(relationship) = relationship.to_str() else {
                continue;
            };
            if !(relationship.ends_with(".wants") || relationship.ends_with(".requires")) {
                continue;
            }
            let relationship_path = entry.path();
            if !fs::metadata(&relationship_path).is_ok_and(|metadata| metadata.is_dir()) {
                continue;
            }
            for candidate in unit_name_candidates(name) {
                let path = relationship_path.join(&candidate);
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        let target = fs::read_link(&path).map_err(|error| {
                            format!("cannot read enablement link {}: {error}", path.display())
                        })?;
                        if target != std::path::Path::new("/dev/null") {
                            return Ok(true);
                        }
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!(
                            "cannot inspect enablement link {}: {error}",
                            path.display()
                        ));
                    }
                }
            }
        }
    }
    Ok(false)
}

fn unit_name_candidates(name: &str) -> Vec<String> {
    if name.ends_with(".service") {
        vec![
            name.to_owned(),
            name.trim_end_matches(".service").to_owned(),
        ]
    } else {
        vec![name.to_owned(), format!("{name}.service")]
    }
}

fn show_units(
    units: &[String],
    requested_properties: &[String],
    value_only: bool,
    quiet: bool,
) -> Result<u8, String> {
    if units.is_empty() {
        return show_manager(requested_properties, value_only, quiet);
    }
    let units = expand_units(units)?;
    let state_paths = StatePaths::from_environment();
    let directories = unit_directories();
    for (index, unit) in units.iter().enumerate() {
        let masked = state_paths
            .is_masked(unit)
            .map_err(|error| format!("cannot inspect masked state for {unit}: {error}"))?;
        let masked = masked || unit_file_is_masked(&directories, unit)?;
        let status =
            match RuntimePaths::from_environment().request(Request::ServiceStatus(unit.clone())) {
                Ok(Response::Service(status)) => status,
                Ok(Response::Error { .. }) if masked => ServiceStatus {
                    name: unit.clone(),
                    state: "defined".to_owned(),
                    pid: None,
                    generation: 0,
                    restart_count: 0,
                },
                Ok(Response::Error { message }) => return Err(message),
                Ok(response) => return Err(format!("unexpected show response: {response:?}")),
                Err(error) if is_not_running(&error) => {
                    return Err("FractalD is not running".to_owned());
                }
                Err(error) => return Err(format!("cannot inspect {unit}: {error}")),
            };
        if index > 0 && !value_only && !quiet {
            println!();
        }
        let unit_file_state = if masked {
            "masked".to_owned()
        } else if state_paths
            .is_enabled(&status.name)
            .map_err(|error| format!("cannot inspect enabled state for {}: {error}", status.name))?
            || unit_file_is_enabled(&directories, &status.name)?
        {
            "enabled".to_owned()
        } else {
            "disabled".to_owned()
        };
        print_properties(
            service_properties(&status, masked, &unit_file_state),
            requested_properties,
            value_only,
            quiet,
        );
    }
    Ok(0)
}

fn show_manager(
    requested_properties: &[String],
    value_only: bool,
    quiet: bool,
) -> Result<u8, String> {
    let response = RuntimePaths::from_environment()
        .request(Request::Status)
        .map_err(|error| {
            if is_not_running(&error) {
                "FractalD is not running".to_owned()
            } else {
                format!("cannot inspect FractalD: {error}")
            }
        })?;
    let Response::Status { pid, uptime_ms } = response else {
        return Err(format!("unexpected manager show response: {response:?}"));
    };
    let properties = vec![
        ("Id".to_owned(), "fractald.service".to_owned()),
        ("LoadState".to_owned(), "loaded".to_owned()),
        ("ActiveState".to_owned(), "active".to_owned()),
        ("SubState".to_owned(), "running".to_owned()),
        ("MainPID".to_owned(), pid.to_string()),
        ("UptimeUSec".to_owned(), (uptime_ms * 1_000).to_string()),
    ];
    print_properties(properties, requested_properties, value_only, quiet);
    Ok(0)
}

fn service_properties(
    status: &ServiceStatus,
    masked: bool,
    unit_file_state: &str,
) -> Vec<(String, String)> {
    let (active_state, sub_state) = active_states(&status.state);
    vec![
        ("Id".to_owned(), status.name.clone()),
        ("Names".to_owned(), status.name.clone()),
        (
            "LoadState".to_owned(),
            if masked { "masked" } else { "loaded" }.to_owned(),
        ),
        ("ActiveState".to_owned(), active_state.to_owned()),
        ("SubState".to_owned(), sub_state.to_owned()),
        ("UnitFileState".to_owned(), unit_file_state.to_owned()),
        (
            "MainPID".to_owned(),
            status
                .pid
                .map_or_else(|| "0".to_owned(), |pid| pid.to_string()),
        ),
        ("ControlPID".to_owned(), "0".to_owned()),
        ("NRestarts".to_owned(), status.restart_count.to_string()),
        (
            "Result".to_owned(),
            if status.state == "failed" {
                "failed"
            } else {
                "success"
            }
            .to_owned(),
        ),
        (
            "CanStart".to_owned(),
            if masked { "no" } else { "yes" }.to_owned(),
        ),
        ("CanStop".to_owned(), "yes".to_owned()),
    ]
}

fn active_states(state: &str) -> (&'static str, &'static str) {
    match state {
        "starting" => ("activating", "start"),
        "running" => ("active", "running"),
        "active" => ("active", "exited"),
        "stopping" => ("deactivating", "stop"),
        "backoff" => ("activating", "auto-restart"),
        "failed" => ("failed", "failed"),
        "defined" | "exited" | "skipped" => ("inactive", "dead"),
        _ => ("inactive", "dead"),
    }
}

fn print_properties(
    properties: Vec<(String, String)>,
    requested_properties: &[String],
    value_only: bool,
    quiet: bool,
) {
    if quiet {
        return;
    }
    if requested_properties.is_empty() {
        for (name, value) in properties {
            print_property(&name, &value, value_only);
        }
        return;
    }
    for requested in requested_properties {
        let value = properties
            .iter()
            .find(|(name, _)| name == requested)
            .map_or_else(String::new, |(_, value)| value.clone());
        print_property(requested, &value, value_only);
    }
}

fn print_property(name: &str, value: &str, value_only: bool) {
    if value_only {
        println!("{value}");
    } else {
        println!("{name}={value}");
    }
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut states = vec![false; value.len() + 1];
    states[0] = true;
    for &character in pattern {
        let mut next = vec![false; value.len() + 1];
        match character {
            b'*' => {
                let mut reachable = false;
                for index in 0..=value.len() {
                    reachable |= states[index];
                    next[index] = reachable;
                }
            }
            b'?' => {
                for index in 0..value.len() {
                    next[index + 1] = states[index];
                }
            }
            literal => {
                for index in 0..value.len() {
                    if value[index] == literal {
                        next[index + 1] = states[index];
                    }
                }
            }
        }
        states = next;
    }
    states[value.len()]
}

fn should_wait(command: &str, now: bool, no_block: bool, wait_requested: bool) -> bool {
    let waitable = matches!(
        command,
        "start"
            | "stop"
            | "isolate"
            | "restart"
            | "try-restart"
            | "reload-or-restart"
            | "try-reload-or-restart"
    ) || (now && matches!(command, "enable" | "disable"));
    if no_block || !waitable {
        return false;
    }
    wait_requested || waitable
}

fn forwarded_len(arguments: &[String]) -> usize {
    arguments
        .iter()
        .skip(1)
        .filter(|argument| !argument.starts_with('-'))
        .count()
}

fn wait_for_unit(unit: &str, operation: &str) -> Result<(), String> {
    let paths = RuntimePaths::from_environment();
    for _ in 0..WAIT_ATTEMPTS {
        match paths.request(Request::ServiceStatus(unit.to_owned())) {
            Ok(Response::Service(status)) => {
                let state = status.state.as_str();
                let complete = match operation {
                    "start" | "restart" | "isolate" | "enable" => {
                        matches!(state, "running" | "active" | "exited" | "skipped")
                    }
                    "stop" | "disable" => {
                        matches!(state, "defined" | "exited" | "skipped" | "failed")
                    }
                    _ => true,
                };
                if complete {
                    if matches!(operation, "start" | "restart" | "isolate" | "enable")
                        && state == "failed"
                    {
                        return Err(format!("{unit} failed to start"));
                    }
                    return Ok(());
                }
                if state == "failed" {
                    return Err(format!("{unit} failed during {operation}"));
                }
            }
            Ok(Response::Error { message }) => return Err(message),
            Ok(response) => return Err(format!("unexpected service response: {response:?}")),
            Err(error) if is_not_running(&error) => {
                return Err("FractalD is not running".to_owned());
            }
            Err(error) => return Err(format!("cannot inspect {unit}: {error}")),
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!(
        "{unit} did not reach a stable state within 5 seconds"
    ))
}

fn service_is_active(unit: &str) -> Result<bool, String> {
    match RuntimePaths::from_environment().request(Request::ServiceStatus(unit.to_owned())) {
        Ok(Response::Service(status)) => Ok(matches!(
            status.state.as_str(),
            "starting" | "running" | "active"
        )),
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected service response: {response:?}")),
        Err(error) if is_not_running(&error) => Err("FractalD is not running".to_owned()),
        Err(error) => Err(format!("cannot inspect {unit}: {error}")),
    }
}

fn is_not_running(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotFound
    )
}

fn invoke_fractalctl(arguments: &[String], quiet: bool) -> Result<u8, String> {
    let binary = if let Some(path) = env::var_os("FRACTALCTL_BIN") {
        PathBuf::from(path)
    } else {
        let current =
            env::current_exe().map_err(|error| format!("cannot locate systemctl: {error}"))?;
        current
            .parent()
            .map(|directory| directory.join("fractalctl"))
            .ok_or_else(|| "cannot locate sibling fractalctl".to_owned())?
    };
    let mut child = Command::new(&binary);
    child.args(arguments);
    if quiet {
        child.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = child
        .status()
        .map_err(|error| format!("cannot launch {}: {error}", binary.display()))?;
    Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
}

fn print_help() {
    println!(
        "FractalD systemctl compatibility\n\nSupported commands:\n  start, stop, restart, status SERVICE\n  isolate TARGET\n  try-restart SERVICE\n  reload-or-restart SERVICE\n  enable, disable [--now] SERVICE\n  preset, preset-all [--preset-mode=MODE]\n  set-property SERVICE Markers=+needs-restart|+needs-reload\n  reload-or-restart --marked [SERVICE]\n  mask, unmask SERVICE\n  is-active, is-enabled, is-failed SERVICE\n  reset-failed [SERVICE]\n  show [-p PROPERTY] [--value] [SERVICE]\n  daemon-reload, daemon-reexec\n  list-units, list-unit-files\n  is-system-running\n  poweroff, reboot, halt\n\nstart/stop/restart/isolate wait for a stable state by default; use --no-block to return immediately."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_fractald_states_to_systemd_properties() {
        assert_eq!(active_states("running"), ("active", "running"));
        assert_eq!(active_states("active"), ("active", "exited"));
        assert_eq!(active_states("backoff"), ("activating", "auto-restart"));
        assert_eq!(active_states("failed"), ("failed", "failed"));
    }

    #[test]
    fn adds_comma_separated_properties_in_order() {
        let mut properties = Vec::new();
        add_properties(&mut properties, " Id, ActiveState,,MainPID ");
        assert_eq!(properties, ["Id", "ActiveState", "MainPID"]);
    }

    #[test]
    fn isolation_waits_for_the_target_to_reach_a_stable_state() {
        assert!(should_wait("isolate", false, false, false));
        assert!(!should_wait("isolate", false, true, false));
    }

    #[cfg(unix)]
    #[test]
    fn recognizes_standard_wants_links_with_or_without_service_suffix() {
        use std::os::unix::fs::symlink;

        let root =
            env::temp_dir().join(format!("fractald-systemctl-enable-{}", std::process::id()));
        let wants = root.join("multi-user.target.wants");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&wants).expect("create wants directory");
        symlink("../demo.service", wants.join("demo.service")).expect("create wants link");

        assert!(unit_file_is_enabled(&[root.clone()], "demo.service").expect("inspect link"));
        assert!(unit_file_is_enabled(&[root.clone()], "demo").expect("inspect suffix link"));

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[cfg(unix)]
    #[test]
    fn ignores_dev_null_enablement_links() {
        use std::os::unix::fs::symlink;

        let root = env::temp_dir().join(format!(
            "fractald-systemctl-disabled-{}",
            std::process::id()
        ));
        let wants = root.join("multi-user.target.wants");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&wants).expect("create wants directory");
        symlink("/dev/null", wants.join("demo.service")).expect("create disabled link");

        assert!(!unit_file_is_enabled(&[root.clone()], "demo.service").expect("inspect link"));

        fs::remove_dir_all(root).expect("remove test directory");
    }

    #[test]
    fn preset_rules_match_wildcards_and_template_instances() {
        assert!(preset_rule_matches("cups.*", "cups.socket"));
        assert!(preset_rule_matches(
            "worker@.service",
            "worker@alpha.service"
        ));
        assert!(!preset_rule_matches("worker@.service", "worker.service"));
    }

    #[test]
    fn preset_rules_use_the_first_matching_policy() {
        let rules = vec![
            PresetRule {
                action: PresetAction::Enable,
                pattern: "demo.service".to_owned(),
            },
            PresetRule {
                action: PresetAction::Disable,
                pattern: "*.service".to_owned(),
            },
        ];
        let action = rules
            .iter()
            .find(|rule| preset_rule_matches(&rule.pattern, "demo.service"))
            .map(|rule| rule.action);
        assert_eq!(action, Some(PresetAction::Enable));
    }
}
