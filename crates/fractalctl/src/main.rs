use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::Duration;

use fractald_control::{Request, Response, RuntimePaths, StatePaths};
use fractald_config::parse_service_file;

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
        Some("doctor") | Some("verify-pid1") if !now && operands.is_empty() => doctor(),
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
        | Some("daemon-reload")
        | Some("doctor")
        | Some("verify-pid1") => {
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
    let state = StatePaths::from_environment();
    state
        .enable("boot")
        .map_err(|error| format!("cannot enable the native boot profile: {error}"))?;
    let path = boot_profile_path()?;
    let contents = "profile=boot\ninit=/usr/bin/fractald\n";
    write_if_changed(&path, contents.as_bytes())?;
    println!(
        "enabled the native FractalD PID1 profile at {}",
        path.display()
    );
    if now { start_daemon() } else { Ok(0) }
}

fn disable(now: bool) -> Result<u8, String> {
    StatePaths::from_environment()
        .disable("boot")
        .map_err(|error| format!("cannot disable the native boot profile: {error}"))?;
    println!("disabled the native FractalD PID1 profile");
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
    let runtime = RuntimePaths::from_environment();
    if current_status(&runtime)?.is_none() {
        start_daemon()?;
    }
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
    if masked {
        println!("masked");
        return Ok(0);
    }
    let enabled = state
        .is_enabled(name)
        .map_err(|error| format!("cannot inspect enabled state for {name}: {error}"))?;
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

struct DoctorReport {
    failures: usize,
    warnings: usize,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            failures: 0,
            warnings: 0,
        }
    }

    fn pass(&self, label: &str, detail: impl AsRef<str>) {
        println!("PASS {label}: {}", detail.as_ref());
    }

    fn fail(&mut self, label: &str, detail: impl AsRef<str>) {
        self.failures += 1;
        println!("FAIL {label}: {}", detail.as_ref());
    }

    fn warn(&mut self, label: &str, detail: impl AsRef<str>) {
        self.warnings += 1;
        println!("WARN {label}: {}", detail.as_ref());
    }
}

fn doctor() -> Result<u8, String> {
    let root = env::var_os("FRACTALD_DOCTOR_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    if !root.is_dir() {
        return Err(format!(
            "doctor root is not a directory: {}",
            root.display()
        ));
    }

    let mut report = DoctorReport::new();
    println!("FractalD PID1 preflight ({})", root.display());
    doctor_binaries(&root, &mut report);
    let services = doctor_services(&root, &mut report);
    doctor_toolbox(&root, &mut report);
    doctor_boot_selection(&root, &mut report);
    doctor_runtime(&root, &services, &mut report);

    if report.failures == 0 {
        if report.warnings == 0 {
            println!("FractalD doctor: PASS");
        } else {
            println!(
                "FractalD doctor: PASS ({} warning(s); review before reboot)",
                report.warnings
            );
        }
        Ok(0)
    } else {
        println!(
            "FractalD doctor: FAIL ({} failure(s), {} warning(s))",
            report.failures, report.warnings
        );
        Ok(1)
    }
}

fn doctor_binaries(root: &Path, report: &mut DoctorReport) {
    for relative in ["/usr/bin/fractald", "/usr/bin/fractalctl", "/usr/lib/fractald/init"] {
        let path = rooted_path(root, Path::new(relative));
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && is_executable(&metadata) => {
                report.pass("binary", format!("{} is executable", path.display()));
            }
            Ok(_) => report.fail(
                "binary",
                format!("{} is not an executable regular file", path.display()),
            ),
            Err(error) => report.fail(
                "binary",
                format!("cannot inspect {}: {error}", path.display()),
            ),
        }
    }
}

fn doctor_services(root: &Path, report: &mut DoctorReport) -> BTreeMap<String, PathBuf> {
    let directories = doctor_service_directories(root);
    let mut selected = BTreeMap::new();
    for directory in directories {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.fail(
                    "services",
                    format!("cannot read {}: {error}", directory.display()),
                );
                continue;
            }
        };
        for entry in entries {
            let path = match entry {
                Ok(entry) => entry.path(),
                Err(error) => {
                    report.fail(
                        "services",
                        format!("cannot enumerate {}: {error}", directory.display()),
                    );
                    continue;
                }
            };
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(".svc") else {
                continue;
            };
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    report.fail(
                        "services",
                        format!("cannot inspect {}: {error}", path.display()),
                    );
                    continue;
                }
            };
            if metadata.file_type().is_symlink() {
                report.fail(
                    "services",
                    format!("{} is a symlink; native descriptors must be regular files", path.display()),
                );
            } else if !metadata.file_type().is_file() {
                report.fail(
                    "services",
                    format!("{} is not a regular file", path.display()),
                );
            } else {
                selected.insert(name.to_owned(), path);
            }
        }
    }

    if selected.is_empty() {
        report.fail(
            "services",
            "no native .svc descriptors were found in the system service roots",
        );
        return selected;
    }

    let mut invalid = false;
    for (name, path) in &selected {
        match parse_service_file(path) {
            Ok(spec) if spec.name == *name => {}
            Ok(spec) => {
                invalid = true;
                report.fail(
                    "services",
                    format!("{} declares unexpected service name {}", path.display(), spec.name),
                );
            }
            Err(error) => {
                invalid = true;
                report.fail(
                    "services",
                    format!("cannot parse {}: {error}", path.display()),
                );
            }
        }
    }
    if !selected.contains_key("boot") {
        report.fail("services", "boot.svc is missing");
    } else if !invalid {
        report.pass(
            "services",
            format!("{} native descriptor(s) validate", selected.len()),
        );
    }
    selected
}

fn doctor_service_directories(root: &Path) -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_SERVICE_DIR") {
        return vec![PathBuf::from(path)];
    }
    [
        "/usr/lib/fractald/services",
        "/usr/libexec/fractald/services",
        "/usr/local/lib/fractald/services",
        "/usr/local/libexec/fractald/services",
        "/run/fractald/services",
        "/etc/fractald/services",
    ]
    .into_iter()
    .map(|path| rooted_path(root, Path::new(path)))
    .collect()
}

fn doctor_toolbox(root: &Path, report: &mut DoctorReport) {
    let failures_before = report.failures;
    let directories = [
        "/usr/lib/fractald/toolbox",
        "/usr/libexec/fractald/toolbox",
        "/usr/local/lib/fractald/toolbox",
        "/usr/local/libexec/fractald/toolbox",
    ];
    let Some(directory) = directories
        .into_iter()
        .map(|path| rooted_path(root, Path::new(path)))
        .find(|path| path.is_dir())
    else {
        report.fail("toolbox", "no native RustyBox toolbox directory exists");
        return;
    };
    for applet in ["mount", "umount", "mkdir", "mv", "rm", "rmdir", "swapon", "swapoff"] {
        let path = directory.join(applet);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() && is_executable(&metadata) => {}
            Ok(_) => report.fail("toolbox", format!("{} is not executable", path.display())),
            Err(error) => report.fail(
                "toolbox",
                format!("cannot inspect {}: {error}", path.display()),
            ),
        }
    }
    if report.failures == failures_before {
        report.pass(
            "toolbox",
            format!("{} provides native storage applets", directory.display()),
        );
    }
}

fn doctor_boot_selection(root: &Path, report: &mut DoctorReport) {
    let mut selected = Vec::new();
    let mut sources = vec![rooted_path(root, Path::new("/etc/kernel/cmdline"))];
    sources.push(rooted_path(root, Path::new("/etc/default/grub")));
    let entries = rooted_path(root, Path::new("/boot/loader/entries"));
    if let Ok(directory) = fs::read_dir(&entries) {
        for entry in directory.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) == Some("conf") {
                sources.push(path);
            }
        }
    }
    for source in sources {
        if let Ok(contents) = fs::read_to_string(&source) {
            selected.extend(init_arguments(&contents));
        }
    }
    let init_links = ["/sbin/init", "/usr/sbin/init"];
    let link_selection = init_links.into_iter().find_map(|path| {
        let path = rooted_path(root, Path::new(path));
        fs::canonicalize(&path)
            .ok()
            .filter(|target| is_known_fractald_binary(root, target))
            .map(|target| (path, target))
    });
    let selected_argument = selected
        .iter()
        .find(|value| is_known_fractald_path(root, value));
    if let Some(value) = selected_argument {
        report.pass("boot", format!("kernel selection contains init={value}"));
    } else if let Some((path, target)) = link_selection {
        report.pass(
            "boot",
            format!("{} resolves to {}", path.display(), target.display()),
        );
    } else {
        report.fail(
            "boot",
            "no kernel/initramfs selection points to FractalD; configure init=/usr/bin/fractald or /sbin/init",
        );
    }

    let boot_profile = rooted_path(root, Path::new("/etc/fractald/boot.conf"));
    match fs::read_to_string(&boot_profile) {
        Ok(contents) => {
            let profile = contents.lines().find_map(|line| {
                let (key, value) = line.trim().split_once('=')?;
                (key == "profile" && !value.trim().is_empty()).then(|| value.trim())
            });
            match profile {
                Some(profile) => report.pass("boot-profile", format!("profile={profile}")),
                None => report.fail(
                    "boot-profile",
                    format!("{} does not define profile=", boot_profile.display()),
                ),
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => report.warn(
            "boot-profile",
            format!("{} is absent; PID1 will use the built-in boot profile", boot_profile.display()),
        ),
        Err(error) => report.fail(
            "boot-profile",
            format!("cannot read {}: {error}", boot_profile.display()),
        ),
    }
}

fn init_arguments(contents: &str) -> Vec<String> {
    contents
        .split_whitespace()
        .filter_map(|word| word.strip_prefix("init="))
        .map(|value| value.trim_matches(['"', '\'']))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_known_fractald_path(root: &Path, value: &str) -> bool {
    let path = Path::new(value);
    if !path.is_absolute() {
        return false;
    }
    let candidate = rooted_path(root, path);
    fs::canonicalize(&candidate)
        .ok()
        .is_some_and(|target| is_known_fractald_binary(root, &target))
}

fn is_known_fractald_binary(root: &Path, path: &Path) -> bool {
    ["/usr/bin/fractald", "/usr/lib/fractald/init"]
        .into_iter()
        .map(|candidate| rooted_path(root, Path::new(candidate)))
        .filter_map(|candidate| fs::canonicalize(candidate).ok())
        .any(|candidate| candidate == path)
}

fn doctor_runtime(
    root: &Path,
    services: &BTreeMap<String, PathBuf>,
    report: &mut DoctorReport,
) {
    let proc_root = env::var_os("FRACTALD_DOCTOR_PROC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| rooted_path(root, Path::new("/proc")));
    let comm_path = proc_root.join("1/comm");
    let current_pid1 = match fs::read_to_string(&comm_path) {
        Ok(comm) if comm.trim() == "fractald" => {
            report.pass("pid1", "current process 1 is fractald");
            true
        }
        Ok(comm) => {
            report.fail("pid1", format!("current process 1 is {}", comm.trim()));
            false
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && root != Path::new("/") => {
            report.warn("pid1", "not checked for an alternate root without a proc fixture");
            false
        }
        Err(error) => {
            report.fail("pid1", format!("cannot inspect {}: {error}", comm_path.display()));
            false
        }
    };

    for relative in ["/proc", "/sys", "/dev", "/run"] {
        let path = rooted_path(root, Path::new(relative));
        if path.is_dir() {
            if current_pid1 {
                report.pass("mounts", format!("{} exists", path.display()));
            }
        } else if current_pid1 {
            report.fail("mounts", format!("{} is missing", path.display()));
        }
    }
    let controllers = rooted_path(root, Path::new("/sys/fs/cgroup/cgroup.controllers"));
    if controllers.is_file() {
        if current_pid1 {
            report.pass("cgroup", "cgroup v2 controllers are available");
        }
    } else if current_pid1 {
        report.fail("cgroup", "cgroup v2 controllers are unavailable");
    }

    if current_pid1 {
        match RuntimePaths::from_environment().request(Request::Status) {
            Ok(Response::Status { pid: 1, .. }) => report.pass("control", "PID1 control socket is ready"),
            Ok(Response::Status { pid, .. }) => report.fail(
                "control",
                format!("control socket reports daemon pid {pid}, not PID1"),
            ),
            Ok(response) => report.fail("control", format!("unexpected control response: {response:?}")),
            Err(error) => report.fail("control", format!("PID1 control socket is unavailable: {error}")),
        }
    } else if !services.is_empty() {
        report.warn("control", "runtime control socket not checked because FractalD is not current PID1");
    }
}

fn rooted_path(root: &Path, path: &Path) -> PathBuf {
    if root == Path::new("/") {
        path.to_owned()
    } else {
        root.join(path.strip_prefix("/").unwrap_or(path))
    }
}

fn is_executable(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        true
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

fn boot_profile_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("FRACTALD_BOOT_PROFILE_FILE") {
        return Ok(PathBuf::from(path));
    }
    if fractald_platform::is_root() {
        Ok(PathBuf::from("/etc/fractald/boot.conf"))
    } else {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .ok_or_else(|| "HOME or XDG_CONFIG_HOME is required".to_owned())?;
        Ok(config_home.join("fractald/boot.conf"))
    }
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

fn write_if_changed(path: &Path, contents: &[u8]) -> Result<(), String> {
    if let Ok(existing) = fs::read(path) {
        if existing == contents {
            return Ok(());
        }
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn print_help() {
    println!(
        "FractalD control\n\nCommands:\n  enable [SERVICE] [--now]   enable daemon or service\n  disable [SERVICE] [--now]  disable daemon or service\n  mask SERVICE               prevent a service from starting\n  unmask SERVICE             remove a service mask\n  start [SERVICE]            launch daemon or start a service\n  isolate PROFILE            activate a service profile and stop other services\n  stop [SERVICE]             gracefully stop daemon or a service\n  restart SERVICE            restart a service\n  reset-failed [SERVICE]     clear failed service state\n  reload SERVICE             reload a service\n  status [SERVICE]           show daemon or service status\n  is-enabled SERVICE         check persistent service enablement\n  is-active SERVICE          check service activity\n  is-failed SERVICE          check failed state\n  list [PATTERN]             list loaded services\n  events [SINCE] [--follow]  replay or subscribe to state events\n  reload                     reload service configuration"
    );
    println!("  transaction-status ID      inspect an asynchronous lifecycle transaction");
    println!("  doctor                     verify PID1 boot, native services, and runtime prerequisites");
    println!("  verify-pid1                alias for doctor");
}

fn usage() -> &'static str {
    "usage: fractalctl [--now] <enable|disable> [SERVICE] | <mask|unmask SERVICE> | <start|isolate|stop|status|reload> [SERVICE] | <restart SERVICE|reset-failed [SERVICE]|is-enabled SERVICE|is-active SERVICE|is-failed SERVICE|doctor|verify-pid1|transaction-status ID|list [PATTERN]|events [SINCE] [--follow]|reload>"
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
    fn accepts_now_before_or_after_enable() {
        let before =
            parse_args(["--now", "enable", "demo.svc"].map(OsString::from)).expect("arguments");
        let after =
            parse_args(["enable", "demo.svc", "--now"].map(OsString::from)).expect("arguments");
        assert_eq!(before, after);
        assert_eq!(
            before,
            ParsedArgs {
                command: Some("enable".to_owned()),
                now: true,
                follow: false,
                operands: vec!["demo.svc".to_owned()],
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
        assert!(wildcard_match("worker@*", "worker@1000"));
        assert!(wildcard_match("*", "demo"));
        assert!(wildcard_match("db?s", "dbus"));
        assert!(!wildcard_match("worker@*", "system"));
    }
}
