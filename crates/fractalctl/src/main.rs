use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use fractald_control::{
    Request, Response, RuntimePaths, StatePaths, unit_file_directories, unit_file_is_enabled,
    unit_file_is_masked,
};

const NOT_RUNNING: u8 = 3;
const TRANSACTION_PENDING: u8 = 2;
const START_ATTEMPTS: usize = 100;
const STOP_ATTEMPTS: usize = 200;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("fractalctl: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let parsed = parse_args(env::args_os().skip(1))?;
    let command = parsed.command;
    let now = parsed.now;
    let follow = parsed.follow;
    let operands = parsed.operands;
    if follow && command.as_deref() != Some("events") {
        return Err("--follow is supported with events only".to_owned());
    }
    match command.as_deref() {
        Some("enable") if operands.is_empty() => enable(now),
        Some("enable") if !now && operands.len() == 1 => enable_service(&operands[0], false),
        Some("enable") if now && operands.len() == 1 => enable_service(&operands[0], true),
        Some("disable") if operands.is_empty() => disable(now),
        Some("disable") if !now && operands.len() == 1 => disable_service(&operands[0], false),
        Some("disable") if now && operands.len() == 1 => disable_service(&operands[0], true),
        Some("start") if !now && operands.is_empty() => start_daemon(),
        Some("start") if !now && operands.len() == 1 => start_service(&operands[0]),
        Some("isolate") if !now && operands.len() == 1 => isolate_service(&operands[0]),
        Some("stop") if !now && operands.is_empty() => stop_daemon(),
        Some("stop") if !now && operands.len() == 1 => stop_service(&operands[0]),
        Some("status") if !now && operands.is_empty() => daemon_status(),
        Some("status") if !now && operands.len() == 1 => service_status(&operands[0]),
        Some("restart") if !now && operands.len() == 1 => restart_service(&operands[0]),
        Some("reset-failed") if !now && operands.len() <= 1 => {
            reset_failed(operands.first().map(String::as_str))
        }
        Some("list") if !now && operands.len() <= 1 => {
            list_services(operands.first().map(String::as_str))
        }
        Some("events") if !now && operands.len() <= 1 => {
            if follow {
                follow_events(operands.first().map(String::as_str))
            } else {
                show_events(operands.first().map(String::as_str))
            }
        }
        Some("reload") if !now && operands.len() == 1 => reload_service(&operands[0]),
        Some("reload") if !now && operands.is_empty() => reload_services(),
        Some("is-enabled") if !now && operands.len() == 1 => is_enabled(&operands[0]),
        Some("is-active") if !now && operands.len() == 1 => is_active(&operands[0]),
        Some("is-failed") if !now && operands.len() == 1 => is_failed(&operands[0]),
        Some("transaction-status") | Some("transaction") if !now && operands.len() == 1 => {
            transaction_status(&operands[0])
        }
        Some("daemon-reload") if !now && operands.is_empty() => reload_services(),
        Some("mask") if !now && operands.len() == 1 => mask_service(&operands[0]),
        Some("unmask") if !now && operands.len() == 1 => unmask_service(&operands[0]),
        Some("enable") | Some("disable") if now || !operands.is_empty() => {
            Err(format!("invalid enable/disable arguments\n\n{}", usage()))
        }
        Some("mask") | Some("unmask") => Err(format!("invalid mask arguments\n\n{}", usage())),
        Some("start")
        | Some("isolate")
        | Some("stop")
        | Some("status")
        | Some("restart")
        | Some("list")
        | Some("events")
        | Some("reload")
        | Some("is-enabled")
        | Some("is-active")
        | Some("is-failed")
        | Some("transaction-status")
        | Some("transaction")
        | Some("daemon-reload") => {
            if now {
                Err("--now is supported with enable and disable only".to_owned())
            } else {
                Err(format!("invalid arguments\n\n{}", usage()))
            }
        }
        Some("help") | Some("--help") if !now && operands.is_empty() => {
            print_help();
            Ok(0)
        }
        None if !now && operands.is_empty() => {
            print_help();
            Ok(0)
        }
        Some("help") | Some("--help") | None => Err(format!("invalid arguments\n\n{}", usage())),
        Some(command) => Err(format!(
            "unknown command or arguments {command:?}\n\n{}",
            usage()
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedArgs {
    command: Option<String>,
    now: bool,
    follow: bool,
    operands: Vec<String>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ParsedArgs, String> {
    let mut command = None;
    let mut now = false;
    let mut follow = false;
    let mut operands = Vec::new();
    for value in args {
        let value = value
            .into_string()
            .map_err(|_| format!("arguments must be valid UTF-8\n\n{}", usage()))?;
        if value == "--now" {
            now = true;
        } else if value == "--follow" {
            follow = true;
        } else if command.is_none() {
            command = Some(value);
        } else {
            operands.push(value);
        }
    }
    Ok(ParsedArgs {
        command,
        now,
        follow,
        operands,
    })
}

fn enable(now: bool) -> Result<u8, String> {
    let boot = BootPaths::from_environment()?;
    fs::create_dir_all(&boot.unit_directory)
        .map_err(|error| format!("cannot create {}: {error}", boot.unit_directory.display()))?;
    fs::create_dir_all(&boot.wants_directory)
        .map_err(|error| format!("cannot create {}: {error}", boot.wants_directory.display()))?;
    let unit = unit_contents(&boot)?;
    write_if_changed(&boot.unit_file, unit.as_bytes())?;
    ensure_enable_link(&boot)?;
    println!("enabled FractalD at {}", boot.unit_file.display());
    if now { start_daemon() } else { Ok(0) }
}

fn disable(now: bool) -> Result<u8, String> {
    let boot = BootPaths::from_environment()?;
    remove_enable_link(&boot)?;
    println!("disabled FractalD at boot");
    if now {
        stop_daemon().map(|code| if code == NOT_RUNNING { 0 } else { code })
    } else {
        Ok(0)
    }
}

fn enable_service(name: &str, now: bool) -> Result<u8, String> {
    let state = StatePaths::from_environment();
    let runtime = RuntimePaths::from_environment();
    if current_status(&runtime)?.is_some() {
        match service_request(Request::EnableService(name.to_owned()))? {
            Response::Enabled { .. } => {}
            response => return Err(format!("unexpected enable response: {response:?}")),
        }
    } else {
        state
            .enable(name)
            .map_err(|error| format!("cannot enable {name}: {error}"))?;
    }
    println!("enabled {name}");
    if now {
        if current_status(&runtime)?.is_none() {
            start_daemon()?;
        }
        start_service(name)
    } else {
        Ok(0)
    }
}

fn disable_service(name: &str, now: bool) -> Result<u8, String> {
    let runtime = RuntimePaths::from_environment();
    if now && current_status(&runtime)?.is_some() {
        stop_service(name)?;
    }
    let state = StatePaths::from_environment();
    if current_status(&runtime)?.is_some() {
        match service_request(Request::DisableService(name.to_owned()))? {
            Response::Disabled { .. } => {}
            response => return Err(format!("unexpected disable response: {response:?}")),
        }
    } else {
        state
            .disable(name)
            .map_err(|error| format!("cannot disable {name}: {error}"))?;
    }
    println!("disabled {name}");
    Ok(0)
}

fn start_daemon() -> Result<u8, String> {
    let paths = RuntimePaths::from_environment();
    if let Some(response) = current_status(&paths)? {
        print_status(response);
        return Ok(0);
    }

    let binary = fractald_binary()?;
    let mut child = Command::new(&binary)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("cannot launch {}: {error}", binary.display()))?;

    for _ in 0..START_ATTEMPTS {
        if let Some(response) = current_status(&paths)? {
            print_status(response);
            return Ok(0);
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot inspect daemon process: {error}"))?
        {
            return Err(format!("daemon exited before becoming ready: {status}"));
        }
        thread::sleep(Duration::from_millis(20));
    }

    let _ = child.kill();
    let _ = child.wait();
    Err("daemon did not become ready".to_owned())
}

fn stop_daemon() -> Result<u8, String> {
    let paths = RuntimePaths::from_environment();
    match paths.request(Request::Stop) {
        Ok(Response::Stopping) => {}
        Ok(Response::Error { message }) => return Err(message),
        Ok(response) => return Err(format!("unexpected stop response: {response:?}")),
        Err(error) if is_not_running(&error) => {
            println!("FractalD is not running");
            return Ok(NOT_RUNNING);
        }
        Err(error) => return Err(format!("cannot contact daemon: {error}")),
    }

    for _ in 0..STOP_ATTEMPTS {
        match paths.request(Request::Status) {
            Ok(Response::Status { .. }) => thread::sleep(Duration::from_millis(25)),
            Ok(Response::Error { message }) => return Err(message),
            Ok(_) => thread::sleep(Duration::from_millis(25)),
            Err(error) if is_not_running(&error) => {
                println!("FractalD stopped");
                return Ok(0);
            }
            Err(error) => return Err(format!("cannot confirm daemon shutdown: {error}")),
        }
    }
    Err("daemon did not stop within 5 seconds".to_owned())
}

fn daemon_status() -> Result<u8, String> {
    let paths = RuntimePaths::from_environment();
    match paths.request(Request::Status) {
        Ok(Response::Status { pid, uptime_ms }) => {
            print_status(Response::Status { pid, uptime_ms });
            Ok(0)
        }
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected status response: {response:?}")),
        Err(error) if is_not_running(&error) => {
            println!("FractalD is not running");
            Ok(NOT_RUNNING)
        }
        Err(error) => Err(format!("cannot contact daemon: {error}")),
    }
}

fn start_service(name: &str) -> Result<u8, String> {
    match service_request(Request::StartService(name.to_owned()))? {
        Response::Accepted {
            id,
            operation,
            name,
        } => {
            println!("{operation} requested for {name} (transaction {id})");
            Ok(0)
        }
        response => Err(format!("unexpected start response: {response:?}")),
    }
}

fn isolate_service(name: &str) -> Result<u8, String> {
    match service_request(Request::IsolateService(name.to_owned()))? {
        Response::Accepted {
            id,
            operation,
            name,
        } => {
            println!("{operation} requested for {name} (transaction {id})");
            Ok(0)
        }
        response => Err(format!("unexpected isolate response: {response:?}")),
    }
}

fn stop_service(name: &str) -> Result<u8, String> {
    match service_request(Request::StopService(name.to_owned()))? {
        Response::Accepted {
            id,
            operation,
            name,
        } => {
            println!("{operation} requested for {name} (transaction {id})");
        }
        response => return Err(format!("unexpected stop response: {response:?}")),
    }

    let paths = RuntimePaths::from_environment();
    for _ in 0..STOP_ATTEMPTS {
        match paths.request(Request::ServiceStatus(name.to_owned())) {
            Ok(Response::Service(snapshot))
                if snapshot.pid.is_none()
                    && matches!(
                        snapshot.state.as_str(),
                        "defined" | "exited" | "skipped" | "failed"
                    ) =>
            {
                println!("{name} stopped");
                return Ok(0);
            }
            Ok(Response::Error { message }) => return Err(message),
            Ok(_) => thread::sleep(Duration::from_millis(25)),
            Err(error) if is_not_running(&error) => {
                return Err(format!("daemon stopped while stopping {name}"));
            }
            Err(error) => return Err(format!("cannot inspect {name}: {error}")),
        }
    }
    Err(format!("{name} did not stop within 5 seconds"))
}

fn service_status(name: &str) -> Result<u8, String> {
    match service_request(Request::ServiceStatus(name.to_owned()))? {
        Response::Service(snapshot) => {
            print_service_status(&snapshot);
            Ok(0)
        }
        response => Err(format!("unexpected service status response: {response:?}")),
    }
}

fn restart_service(name: &str) -> Result<u8, String> {
    match service_request(Request::RestartService(name.to_owned()))? {
        Response::Accepted {
            id,
            operation,
            name,
        } => {
            println!("{operation} requested for {name} (transaction {id})");
            Ok(0)
        }
        response => Err(format!("unexpected restart response: {response:?}")),
    }
}

fn reset_failed(name: Option<&str>) -> Result<u8, String> {
    match service_request(Request::ResetFailed(name.map(str::to_owned)))? {
        Response::Reset => {
            println!("failed state reset");
            Ok(0)
        }
        response => Err(format!("unexpected reset response: {response:?}")),
    }
}

fn list_services(pattern: Option<&str>) -> Result<u8, String> {
    match service_request(Request::List)? {
        Response::Services { names } => {
            let names = names
                .into_iter()
                .filter(|name| pattern.map_or(true, |pattern| wildcard_match(pattern, name)))
                .collect::<Vec<_>>();
            if names.is_empty() {
                println!("no services loaded");
            } else {
                for name in names {
                    println!("{name}");
                }
            }
            Ok(0)
        }
        response => Err(format!("unexpected list response: {response:?}")),
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

fn show_events(since: Option<&str>) -> Result<u8, String> {
    let since = parse_event_since(since)?;
    let path = StatePaths::from_environment().events;
    let contents = match fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(format!("cannot read event log {}: {error}", path.display())),
    };
    for line in contents.lines() {
        let Some(sequence) = event_sequence(line) else {
            continue;
        };
        if since.map_or(true, |minimum| sequence >= minimum) {
            println!("{line}");
        }
    }
    Ok(0)
}

fn follow_events(since: Option<&str>) -> Result<u8, String> {
    let since = parse_event_since(since)?;
    let stream = RuntimePaths::from_environment()
        .subscribe(since)
        .map_err(|error| {
            if is_not_running(&error) {
                "FractalD is not running".to_owned()
            } else {
                format!("cannot subscribe to FractalD events: {error}")
            }
        })?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|error| format!("cannot read FractalD event stream: {error}"))?;
        if bytes == 0 {
            return Ok(0);
        }
        match Response::parse(&line)
            .map_err(|error| format!("invalid FractalD event response: {error}"))?
        {
            Response::Subscribed => {}
            Response::Event { line } => println!("{line}"),
            Response::Error { message } => return Err(message),
            response => return Err(format!("unexpected event response: {response:?}")),
        }
    }
}

fn parse_event_since(since: Option<&str>) -> Result<Option<u64>, String> {
    since
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("event sequence must be an unsigned integer: {value}"))
        })
        .transpose()
}

fn event_sequence(line: &str) -> Option<u64> {
    line.split_whitespace()
        .next()
        .and_then(|field| field.strip_prefix("seq="))
        .and_then(|value| value.parse().ok())
}

fn reload_services() -> Result<u8, String> {
    match service_request(Request::Reload)? {
        Response::Reloaded => {
            println!("configuration reloaded");
            Ok(0)
        }
        Response::Error { message } => Err(message),
        response => Err(format!("unexpected reload response: {response:?}")),
    }
}

fn reload_service(name: &str) -> Result<u8, String> {
    match service_request(Request::ReloadService(name.to_owned()))? {
        Response::Accepted {
            id,
            operation,
            name,
        } => {
            println!("{operation} requested for {name} (transaction {id})");
            Ok(0)
        }
        response => Err(format!("unexpected reload response: {response:?}")),
    }
}

fn is_enabled(name: &str) -> Result<u8, String> {
    let state = StatePaths::from_environment();
    let masked = state
        .is_masked(name)
        .map_err(|error| format!("cannot inspect masked state for {name}: {error}"))?;
    let directories = unit_file_directories();
    let masked = masked
        || unit_file_is_masked(&directories, name)
            .map_err(|error| format!("cannot inspect unit mask for {name}: {error}"))?;
    if masked {
        println!("masked");
        return Ok(0);
    }
    let enabled = state
        .is_enabled(name)
        .map_err(|error| format!("cannot inspect enabled state for {name}: {error}"))?;
    let enabled = enabled
        || unit_file_is_enabled(&directories, name)
            .map_err(|error| format!("cannot inspect unit enablement for {name}: {error}"))?;
    if enabled {
        println!("enabled");
        Ok(0)
    } else {
        println!("disabled");
        Ok(1)
    }
}

fn mask_service(name: &str) -> Result<u8, String> {
    let runtime = RuntimePaths::from_environment();
    if current_status(&runtime)?.is_some() {
        match runtime.request(Request::ServiceStatus(name.to_owned())) {
            Ok(Response::Service(status))
                if matches!(
                    status.state.as_str(),
                    "starting" | "running" | "active" | "stopping" | "backoff"
                ) =>
            {
                stop_service(name)?;
            }
            Ok(Response::Service(_)) => {}
            Ok(Response::Error { message })
                if message.starts_with("unknown service") || message.contains(" is masked") => {}
            Ok(response) => return Err(format!("unexpected service response: {response:?}")),
            Err(error) if is_not_running(&error) => {}
            Err(error) => return Err(format!("cannot contact daemon: {error}")),
        }
    }

    let state = StatePaths::from_environment();
    state
        .mask(name)
        .map_err(|error| format!("cannot mask {name}: {error}"))?;
    if current_status(&runtime)?.is_some() {
        reload_services()?;
    }
    println!("masked {name}");
    Ok(0)
}

fn unmask_service(name: &str) -> Result<u8, String> {
    let state = StatePaths::from_environment();
    state
        .unmask(name)
        .map_err(|error| format!("cannot unmask {name}: {error}"))?;
    let runtime = RuntimePaths::from_environment();
    if current_status(&runtime)?.is_some() {
        reload_services()?;
    }
    println!("unmasked {name}");
    Ok(0)
}

fn is_active(name: &str) -> Result<u8, String> {
    match service_request(Request::ServiceStatus(name.to_owned()))? {
        Response::Service(snapshot) => {
            let active = matches!(snapshot.state.as_str(), "running" | "starting" | "active");
            println!("{}", if active { "active" } else { "inactive" });
            Ok(if active { 0 } else { NOT_RUNNING })
        }
        response => Err(format!("unexpected active response: {response:?}")),
    }
}

fn is_failed(name: &str) -> Result<u8, String> {
    match service_request(Request::ServiceStatus(name.to_owned()))? {
        Response::Service(snapshot) => {
            let failed = snapshot.state == "failed";
            println!("{}", if failed { "failed" } else { "not-failed" });
            Ok(if failed { 0 } else { 1 })
        }
        response => Err(format!("unexpected failed-state response: {response:?}")),
    }
}

fn transaction_status(value: &str) -> Result<u8, String> {
    let id = value
        .parse::<u64>()
        .map_err(|_| format!("transaction id must be an unsigned integer: {value}"))?;
    if id == 0 {
        return Err("transaction id must be non-zero".to_owned());
    }
    match service_request(Request::TransactionStatus(id))? {
        Response::Transaction(status) => {
            if status.state == "unknown" {
                println!("transaction {}: unknown", status.id);
                return Ok(NOT_RUNNING);
            }
            println!(
                "transaction {}: {} {} {}",
                status.id, status.state, status.operation, status.name
            );
            match status.state.as_str() {
                "done" => Ok(0),
                "failed" => Ok(1),
                "pending" => Ok(TRANSACTION_PENDING),
                _ => Err(format!("unknown transaction state {}", status.state)),
            }
        }
        response => Err(format!("unexpected transaction response: {response:?}")),
    }
}

fn service_request(request: Request) -> Result<Response, String> {
    RuntimePaths::from_environment()
        .request(request)
        .map_err(|error| {
            if is_not_running(&error) {
                "FractalD is not running".to_owned()
            } else {
                format!("cannot contact daemon: {error}")
            }
        })
        .and_then(|response| match response {
            Response::Error { message } => Err(message),
            response => Ok(response),
        })
}

fn print_service_status(status: &fractald_control::ServiceStatus) {
    let pid = status
        .pid
        .map_or_else(|| "no process".to_owned(), |pid| format!("pid {pid}"));
    println!(
        "{}: {} ({pid}, generation {}, restarts {})",
        status.name, status.state, status.generation, status.restart_count
    );
}

fn current_status(paths: &RuntimePaths) -> Result<Option<Response>, String> {
    match paths.request(Request::Status) {
        Ok(Response::Status { pid, uptime_ms }) => Ok(Some(Response::Status { pid, uptime_ms })),
        Ok(Response::Error { message }) => Err(message),
        Ok(response) => Err(format!("unexpected status response: {response:?}")),
        Err(error) if is_not_running(&error) => Ok(None),
        Err(error) => Err(format!("cannot contact daemon: {error}")),
    }
}

fn print_status(response: Response) {
    if let Response::Status { pid, uptime_ms } = response {
        println!(
            "FractalD is running (pid {pid}, uptime {})",
            format_uptime(uptime_ms)
        );
    }
}

fn format_uptime(milliseconds: u64) -> String {
    let mut seconds = milliseconds / 1_000;
    let days = seconds / 86_400;
    seconds %= 86_400;
    let hours = seconds / 3_600;
    seconds %= 3_600;
    let minutes = seconds / 60;
    seconds %= 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m {seconds}s")
    } else if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn is_not_running(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotFound
    )
}

struct BootPaths {
    unit_directory: PathBuf,
    wants_directory: PathBuf,
    unit_file: PathBuf,
    enable_link: PathBuf,
    target: String,
}

impl BootPaths {
    fn from_environment() -> Result<Self, String> {
        let (unit_directory, default_target) = if let Some(path) = env::var_os("FRACTALD_UNIT_DIR")
        {
            (PathBuf::from(path), "default.target".to_owned())
        } else if fractald_platform::is_root() {
            (
                PathBuf::from("/etc/systemd/system"),
                "multi-user.target".to_owned(),
            )
        } else {
            let config_home = env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
                .ok_or_else(|| "HOME or XDG_CONFIG_HOME is required".to_owned())?;
            (
                config_home.join("systemd/user"),
                "default.target".to_owned(),
            )
        };
        let target = env::var("FRACTALD_BOOT_TARGET").unwrap_or(default_target);
        let wants_directory = unit_directory.join(format!("{target}.wants"));
        let unit_file = unit_directory.join("fractald.service");
        let enable_link = wants_directory.join("fractald.service");
        Ok(Self {
            unit_directory,
            wants_directory,
            unit_file,
            enable_link,
            target,
        })
    }
}

fn unit_contents(boot: &BootPaths) -> Result<String, String> {
    let binary = fractald_binary()?;
    let escaped_binary = escape_systemd_word(&binary);
    let target = &boot.target;
    Ok(format!(
        "[Unit]\nDescription=FractalD service supervisor\n\n[Service]\nType=simple\nEnvironment=FRACTALD_BOOT_TARGET={target}\nExecStart={escaped_binary} daemon\nRestart=on-failure\nRestartSec=1s\n\n[Install]\nWantedBy={target}\n"
    ))
}

fn fractald_binary() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("FRACTALD_BIN") {
        return Ok(PathBuf::from(path));
    }
    let current =
        env::current_exe().map_err(|error| format!("cannot locate fractalctl: {error}"))?;
    current
        .parent()
        .map(|directory| directory.join("fractald"))
        .ok_or_else(|| "cannot locate sibling fractald binary".to_owned())
}

fn escape_systemd_word(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace(' ', "\\x20")
        .replace('\t', "\\x09")
}

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Ok(existing) = fs::read(path) {
        if existing == contents {
            return Ok(());
        }
    }
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn ensure_enable_link(boot: &BootPaths) -> Result<(), String> {
    match fs::symlink_metadata(&boot.enable_link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(&boot.enable_link).map_err(|error| {
                format!("cannot inspect {}: {error}", boot.enable_link.display())
            })?;
            if target != Path::new("../fractald.service") {
                return Err(format!(
                    "{} already points to {}",
                    boot.enable_link.display(),
                    target.display()
                ));
            }
            Ok(())
        }
        Ok(_) => Err(format!("{} is not a symlink", boot.enable_link.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            symlink("../fractald.service", &boot.enable_link)
                .map_err(|error| format!("cannot enable {}: {error}", boot.enable_link.display()))
        }
        Err(error) => Err(format!(
            "cannot inspect {}: {error}",
            boot.enable_link.display()
        )),
    }
}

fn remove_enable_link(boot: &BootPaths) -> Result<(), String> {
    match fs::symlink_metadata(&boot.enable_link) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target = fs::read_link(&boot.enable_link).map_err(|error| {
                format!("cannot inspect {}: {error}", boot.enable_link.display())
            })?;
            if target != Path::new("../fractald.service") {
                return Err(format!(
                    "{} points to {} and was left intact",
                    boot.enable_link.display(),
                    target.display()
                ));
            }
            fs::remove_file(&boot.enable_link)
                .map_err(|error| format!("cannot disable {}: {error}", boot.enable_link.display()))
        }
        Ok(_) => Err(format!("{} is not a symlink", boot.enable_link.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "cannot inspect {}: {error}",
            boot.enable_link.display()
        )),
    }
}

fn print_help() {
    println!(
        "FractalD control\n\nCommands:\n  enable [SERVICE] [--now]   enable daemon or service\n  disable [SERVICE] [--now]  disable daemon or service\n  mask SERVICE               prevent a service from starting\n  unmask SERVICE             remove a service mask\n  start [SERVICE]            launch daemon or start a service\n  isolate TARGET             activate a target and stop other units\n  stop [SERVICE]             gracefully stop daemon or a service\n  restart SERVICE            restart a service\n  reset-failed [SERVICE]     clear failed service state\n  reload SERVICE             reload a service\n  status [SERVICE]           show daemon or service status\n  is-enabled SERVICE         check persistent service enablement\n  is-active SERVICE          check service activity\n  is-failed SERVICE          check failed state\n  list [PATTERN]             list loaded services\n  events [SINCE] [--follow]  replay or subscribe to state events\n  reload                     reload service configuration"
    );
    println!("  transaction-status ID      inspect an asynchronous lifecycle transaction");
}

fn usage() -> &'static str {
    "usage: fractalctl [--now] <enable|disable> [SERVICE] | <mask|unmask SERVICE> | <start|isolate|stop|status|reload> [SERVICE] | <restart SERVICE|reset-failed [SERVICE]|is-enabled SERVICE|is-active SERVICE|is-failed SERVICE|transaction-status ID|list [PATTERN]|events [SINCE] [--follow]|reload>"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_uptime_at_each_unit() {
        assert_eq!(format_uptime(0), "0s");
        assert_eq!(format_uptime(61_000), "1m 1s");
        assert_eq!(format_uptime(3_661_000), "1h 1m 1s");
        assert_eq!(
            format_uptime(86_400_000 + 3_600_000 + 60_000 + 1_000),
            "1d 1h 1m 1s"
        );
    }

    #[test]
    fn escapes_systemd_path_words() {
        let path = Path::new("/tmp/FractalD data\\bin");
        assert_eq!(escape_systemd_word(path), "/tmp/FractalD\\x20data\\\\bin");
    }

    #[test]
    fn accepts_now_before_or_after_enable() {
        let before =
            parse_args(["--now", "enable", "demo.service"].map(OsString::from)).expect("arguments");
        let after =
            parse_args(["enable", "demo.service", "--now"].map(OsString::from)).expect("arguments");
        assert_eq!(before, after);
        assert_eq!(
            before,
            ParsedArgs {
                command: Some("enable".to_owned()),
                now: true,
                follow: false,
                operands: vec!["demo.service".to_owned()],
            }
        );
    }

    #[test]
    fn accepts_follow_before_or_after_event_sequence() {
        let before =
            parse_args(["--follow", "events", "4"].map(OsString::from)).expect("arguments");
        let after = parse_args(["events", "4", "--follow"].map(OsString::from)).expect("arguments");
        assert_eq!(before, after);
        assert_eq!(before.command.as_deref(), Some("events"));
        assert!(before.follow);
        assert_eq!(before.operands, vec!["4"]);
    }

    #[test]
    fn matches_list_patterns() {
        assert!(wildcard_match("user@*", "user@1000.service"));
        assert!(wildcard_match("*.service", "demo.service"));
        assert!(wildcard_match("db?s.service", "dbus.service"));
        assert!(!wildcard_match("user@*", "system.service"));
    }
}
