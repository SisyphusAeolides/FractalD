use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::process::ExitCode;

use fractald_control::{RuntimePaths, StatePaths};

const NEEDS_RESTART: &str = "needs-restart";
const NEEDS_RELOAD: &str = "needs-reload";

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-update-helper: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let command = args
        .next()
        .ok_or_else(|| "a helper operation is required".to_owned())?;
    let units = args.collect::<Vec<_>>();
    match command.as_str() {
        "mark-restart-system-units" | "mark-restart-user-units" => {
            mark_units(&units, NEEDS_RESTART)
        }
        "mark-reload-system-units" | "mark-reload-user-units" => mark_units(&units, NEEDS_RELOAD),
        "install-system-units" => install_units(&units, false),
        "install-user-units" => install_units(&units, true),
        "remove-system-units" => remove_units(&units, false),
        "remove-user-units" => remove_units(&units, true),
        "system-reload-restart" | "system-reload" | "system-restart" => {
            manager_transaction(&command, &units, false)
        }
        "user-reload-restart" | "user-reload" | "user-restart" | "user-reexec" => {
            manager_transaction(&command, &units, true)
        }
        "--help" | "help" => {
            print_help();
            Ok(())
        }
        "--version" | "version" => {
            println!(
                "systemd-update-helper (FractalD) {}",
                env!("CARGO_PKG_VERSION")
            );
            Ok(())
        }
        other => Err(format!("unsupported helper operation {other}")),
    }
}

fn mark_units(units: &[String], marker: &str) -> Result<(), String> {
    if units.is_empty() {
        return Err("the marker operation requires at least one unit".to_owned());
    }
    let directory = RuntimePaths::from_environment().directory.join("markers");
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "cannot create marker directory {}: {error}",
            directory.display()
        )
    })?;
    for unit in units {
        validate_unit(unit)?;
        fs::write(directory.join(format!("{unit}.{marker}")), b"1\n")
            .map_err(|error| format!("cannot mark {unit}: {error}"))?;
    }
    Ok(())
}

fn install_units(units: &[String], user: bool) -> Result<(), String> {
    if units.is_empty() {
        return Err("install-system-units requires at least one unit".to_owned());
    }
    for unit in units {
        validate_unit(unit)?;
    }
    let mut arguments = vec!["--no-reload".to_owned()];
    if user {
        arguments.push("--global".to_owned());
    }
    arguments.push("preset".to_owned());
    arguments.extend(units.iter().cloned());
    if invoke_systemctl(&arguments)? {
        return Ok(());
    }

    let state = StatePaths::from_environment();
    for unit in units {
        state
            .enable(unit)
            .map_err(|error| format!("cannot persist enablement for {unit}: {error}"))?;
    }
    Ok(())
}

fn remove_units(units: &[String], user: bool) -> Result<(), String> {
    if units.is_empty() {
        return Err("remove-system-units requires at least one unit".to_owned());
    }
    for unit in units {
        validate_unit(unit)?;
    }
    let mut arguments = vec!["--no-reload".to_owned()];
    if user {
        arguments.push("--global".to_owned());
    }
    arguments.extend(["disable".to_owned(), "--no-warn".to_owned()]);
    if RuntimePaths::from_environment().socket.exists() {
        arguments.push("--now".to_owned());
    }
    arguments.extend(units.iter().cloned());
    if invoke_systemctl(&arguments)? {
        return Ok(());
    }

    let state = StatePaths::from_environment();
    for unit in units {
        state
            .disable(unit)
            .map_err(|error| format!("cannot remove enablement for {unit}: {error}"))?;
    }
    Ok(())
}

fn manager_transaction(command: &str, units: &[String], user: bool) -> Result<(), String> {
    if !units.is_empty() {
        return Err(format!("{command} does not accept arguments"));
    }
    if !RuntimePaths::from_environment().socket.exists() {
        return Ok(());
    }

    let invoke = |arguments: &[&str]| -> Result<(), String> {
        let arguments = arguments
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>();
        invoke_systemctl(&arguments).map(|_| ())
    };

    match (user, command) {
        (false, "system-reload-restart") => {
            invoke(&["daemon-reload"])?;
            invoke(&["reload-or-restart", "--marked"])?;
        }
        (false, "system-reload") => invoke(&["daemon-reload"])?,
        (false, "system-restart") => invoke(&["reload-or-restart", "--marked"])?,
        (true, "user-reload-restart") => {
            invoke(&["--user", "reload", "user@*.service"])?;
            invoke(&["--user", "reload-or-restart", "--marked"])?;
        }
        (true, "user-reload") | (true, "user-reexec") => {
            invoke(&["--user", "reload", "user@*.service"])?;
        }
        (true, "user-restart") => invoke(&["--user", "reload-or-restart", "--marked"])?,
        _ => return Err(format!("unsupported manager transaction {command}")),
    }
    Ok(())
}

fn invoke_systemctl(arguments: &[String]) -> Result<bool, String> {
    let Some(binary) = systemctl_binary() else {
        return Ok(false);
    };
    let status = Command::new(&binary)
        .args(arguments)
        .status()
        .map_err(|error| format!("cannot launch {}: {error}", binary.display()))?;
    if !status.success() {
        return Err(format!("{} exited with {status}", binary.display()));
    }
    Ok(true)
}

fn systemctl_binary() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_SYSTEMCTL_BIN") {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let mut candidates = Vec::new();
    if let Ok(current) = env::current_exe() {
        if let Some(directory) = current.parent() {
            candidates.push(directory.join("systemctl"));
        }
    }
    candidates.push(PathBuf::from("/usr/bin/systemctl"));
    candidates.push(PathBuf::from("/bin/systemctl"));
    candidates.into_iter().find(|path| path.is_file())
}

fn validate_unit(unit: &str) -> Result<(), String> {
    if unit.is_empty()
        || unit == "."
        || unit == ".."
        || unit.starts_with('-')
        || unit.contains('/')
        || unit
            .chars()
            .any(|character| character == '\0' || character.is_whitespace())
    {
        return Err(format!("invalid unit name {unit:?}"));
    }
    Ok(())
}

fn print_help() {
    println!(
        "systemd-update-helper (FractalD)\n\nUsage: systemd-update-helper OPERATION [UNIT...]\n\n  install-system-units       persist package unit enablement\n  remove-system-units        remove package unit enablement\n  mark-restart-system-units  record restart markers\n  mark-reload-system-units   record reload markers\n  install/remove/mark operations also accept the -user-units forms\n  system-reload[-restart]    refresh system units and marked services\n  system-restart             restart marked system services\n  user-reload[-restart]      refresh user managers and marked services\n  user-restart               restart marked user services\n  user-reexec                refresh user managers"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_like_unit_names() {
        assert!(validate_unit("demo.service").is_ok());
        assert!(validate_unit("/tmp/demo.service").is_err());
        assert!(validate_unit("--bad.service").is_err());
    }

    #[test]
    fn accepts_update_operations() {
        assert!(run(["user-reexec".to_owned()].into_iter()).is_ok());
    }
}
