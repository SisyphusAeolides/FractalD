use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("{}: {error}", program_name());
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    if program_name() == "rc-status" {
        return invoke(&["list"]);
    }
    let mut args = env::args().skip(1).filter(|value| {
        !matches!(
            value.as_str(),
            "--ifstarted" | "--ifnotstarted" | "--exists" | "-e" | "-i"
        )
    });
    let service = args
        .next()
        .ok_or_else(|| "usage: rc-service SERVICE <start|stop|restart|reload|status>".to_owned())?;
    let action = args
        .next()
        .ok_or_else(|| "usage: rc-service SERVICE <start|stop|restart|reload|status>".to_owned())?;
    if args.next().is_some() {
        return Err("too many arguments".to_owned());
    }
    if !matches!(
        action.as_str(),
        "start" | "stop" | "restart" | "reload" | "status"
    ) {
        return Err(format!("unsupported action {action}"));
    }
    invoke(&[action.as_str(), service.as_str()])
}

fn invoke(arguments: &[&str]) -> Result<u8, String> {
    let binary = if let Some(path) = env::var_os("FRACTALCTL_BIN") {
        PathBuf::from(path)
    } else {
        let current =
            env::current_exe().map_err(|error| format!("cannot locate rc-service: {error}"))?;
        current
            .parent()
            .map(|directory| directory.join("fractalctl"))
            .ok_or_else(|| "cannot locate sibling fractalctl".to_owned())?
    };
    let status = Command::new(&binary)
        .args(arguments)
        .status()
        .map_err(|error| format!("cannot launch {}: {error}", binary.display()))?;
    Ok(status.code().unwrap_or(1).clamp(0, 255) as u8)
}

fn program_name() -> String {
    env::args()
        .next()
        .and_then(|path| {
            PathBuf::from(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "rc-service".to_owned())
}
