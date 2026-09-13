use std::env;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

pub const RUNTIME_DIRECTORY: &str = "fractald";
pub const SOCKET_FILE: &str = "control.sock";
pub const PID_FILE: &str = "pid";
pub const STATE_DIRECTORY: &str = "fractald";
pub const ENABLED_DIRECTORY: &str = "enabled";
pub const MASKED_DIRECTORY: &str = "masked";
pub const EVENT_LOG: &str = "events.log";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimePaths {
    pub directory: PathBuf,
    pub socket: PathBuf,
    pub pid: PathBuf,
}

impl RuntimePaths {
    pub fn from_environment() -> Self {
        let directory = if let Some(path) = env::var_os("FRACTALD_RUNTIME_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
            PathBuf::from(path).join(RUNTIME_DIRECTORY)
        } else if fractald_platform::is_root() {
            PathBuf::from("/run").join(RUNTIME_DIRECTORY)
        } else {
            PathBuf::from(format!(
                "/tmp/fractald-{}",
                fractald_platform::effective_uid()
            ))
        };
        Self::new(directory)
    }

    pub fn new(directory: impl Into<PathBuf>) -> Self {
        let directory = directory.into();
        Self {
            socket: directory.join(SOCKET_FILE),
            pid: directory.join(PID_FILE),
            directory,
        }
    }

    pub fn ensure_directory(&self) -> io::Result<()> {
        fs::create_dir_all(&self.directory)?;
        let mut permissions = fs::metadata(&self.directory)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
            fs::set_permissions(&self.directory, permissions)?;
        }
        Ok(())
    }

    pub fn connect(&self) -> io::Result<UnixStream> {
        UnixStream::connect(&self.socket)
    }

    pub fn request(&self, request: Request) -> io::Result<Response> {
        let mut stream = self.connect()?;
        stream.write_all(&request.as_bytes())?;
        stream.shutdown(Shutdown::Write)?;
        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        Response::parse(&response)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn subscribe(&self, since: Option<u64>) -> io::Result<UnixStream> {
        let mut stream = self.connect()?;
        stream.write_all(&Request::Subscribe(since).as_bytes())?;
        stream.shutdown(Shutdown::Write)?;
        Ok(stream)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatePaths {
    pub directory: PathBuf,
    pub enabled: PathBuf,
    pub masked: PathBuf,
    pub events: PathBuf,
}

impl StatePaths {
    pub fn from_environment() -> Self {
        let directory = if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join(STATE_DIRECTORY)
        } else if fractald_platform::is_root() {
            PathBuf::from("/var/lib").join(STATE_DIRECTORY)
        } else if let Some(path) = env::var_os("HOME") {
            PathBuf::from(path)
                .join(".local/state")
                .join(STATE_DIRECTORY)
        } else {
            PathBuf::from(format!(
                "/tmp/fractald-state-{}",
                fractald_platform::effective_uid()
            ))
        };
        Self {
            enabled: directory.join(ENABLED_DIRECTORY),
            masked: directory.join(MASKED_DIRECTORY),
            events: directory.join(EVENT_LOG),
            directory,
        }
    }

    pub fn ensure_directory(&self) -> io::Result<()> {
        fs::create_dir_all(&self.enabled)?;
        fs::create_dir_all(&self.masked)?;
        let mut permissions = fs::metadata(&self.directory)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(0o700);
            fs::set_permissions(&self.directory, permissions)?;
        }
        Ok(())
    }

    pub fn enable(&self, name: &str) -> io::Result<()> {
        validate_service_name(name)?;
        self.ensure_directory()?;
        let name = native_service_name(name);
        let path = self.enabled.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "enabled marker is a symlink",
            )),
            Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "enabled marker is not a regular file",
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::write(path, b"enabled\n"),
            Err(error) => Err(error),
        }
    }

    pub fn disable(&self, name: &str) -> io::Result<()> {
        validate_service_name(name)?;
        for candidate in service_name_candidates(name) {
            match fs::remove_file(self.enabled.join(candidate)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn is_enabled(&self, name: &str) -> io::Result<bool> {
        validate_service_name(name)?;
        for candidate in service_name_candidates(name) {
            match fs::symlink_metadata(self.enabled.join(candidate)) {
                Ok(metadata) if metadata.file_type().is_file() => return Ok(true),
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "enabled marker is a symlink",
                    ));
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "enabled marker is not a regular file",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    pub fn enabled_names(&self) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(&self.enabled) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str() {
                if validate_service_name(name).is_ok() {
                    names.push(name.to_owned());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    pub fn mask(&self, name: &str) -> io::Result<()> {
        validate_service_name(name)?;
        self.ensure_directory()?;
        let name = native_service_name(name);
        let path = self.masked.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "masked marker is a symlink",
            )),
            Ok(metadata) if !metadata.file_type().is_file() => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "masked marker is not a regular file",
            )),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::write(path, b"masked\n"),
            Err(error) => Err(error),
        }
    }

    pub fn unmask(&self, name: &str) -> io::Result<()> {
        validate_service_name(name)?;
        for candidate in service_name_candidates(name) {
            match fs::remove_file(self.masked.join(candidate)) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub fn is_masked(&self, name: &str) -> io::Result<bool> {
        validate_service_name(name)?;
        for candidate in service_name_candidates(name) {
            match fs::symlink_metadata(self.masked.join(candidate)) {
                Ok(metadata) if metadata.file_type().is_file() => return Ok(true),
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "masked marker is a symlink",
                    ));
                }
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "masked marker is not a regular file",
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }
}

pub fn service_directories() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_SERVICE_DIR") {
        return vec![PathBuf::from(path)];
    }
    if fractald_platform::is_root() {
        vec![
            PathBuf::from("/usr/lib/fractald/services"),
            PathBuf::from("/usr/local/lib/fractald/services"),
            PathBuf::from("/run/fractald/services"),
            PathBuf::from("/etc/fractald/services"),
        ]
    } else {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!(
                    "/tmp/fractald-runtime-{}",
                    fractald_platform::effective_uid()
                ))
            });
        let mut directories = Vec::new();
        if let Some(config_home) = config_home {
            directories.push(config_home.join("fractald/services"));
        }
        directories.push(runtime.join("fractald/services"));
        directories
    }
}

pub fn service_is_enabled(directories: &[PathBuf], name: &str) -> io::Result<bool> {
    let name = native_service_name(name);
    validate_service_name(&name)?;
    for directory in directories {
        let path = directory.join(format!("{name}.enabled"));
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => return Ok(true),
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

pub fn service_enabled_names(directories: &[PathBuf]) -> io::Result<Vec<String>> {
    let mut names = std::collections::BTreeSet::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        for entry in entries {
            let entry = entry?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(name) = file_name.strip_suffix(".enabled") else {
                continue;
            };
            let metadata = fs::symlink_metadata(entry.path())?;
            if validate_service_name(name).is_ok() && metadata.file_type().is_file() {
                names.insert(name.to_owned());
            }
        }
    }
    Ok(names.into_iter().collect())
}

pub fn service_is_masked(directories: &[PathBuf], name: &str) -> io::Result<bool> {
    let name = native_service_name(name);
    validate_service_name(&name)?;
    for directory in directories.iter().rev() {
        let path = directory.join(format!("{name}.masked"));
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => return Ok(true),
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

fn validate_service_name(name: &str) -> io::Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains('/')
        || name
            .chars()
            .any(|character| character == '\0' || character.is_whitespace())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid service name",
        ));
    }
    Ok(())
}

fn native_service_name(name: &str) -> String {
    name.strip_suffix(".svc").unwrap_or(name).to_owned()
}

fn service_name_candidates(name: &str) -> Vec<String> {
    vec![native_service_name(name)]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShutdownAction {
    Stop,
    Poweroff,
    Reboot,
    Halt,
}

impl ShutdownAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Poweroff => "poweroff",
            Self::Reboot => "reboot",
            Self::Halt => "halt",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "stop" => Some(Self::Stop),
            "poweroff" => Some(Self::Poweroff),
            "reboot" => Some(Self::Reboot),
            "halt" => Some(Self::Halt),
            _ => None,
        }
    }
}

#[cfg(test)]
mod service_name_tests {
    use super::validate_service_name;

    #[test]
    fn accepts_native_service_names() {
        assert!(validate_service_name("network-online").is_ok());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Status,
    Stop,
    Ping,
    List,
    Reload,
    EnableService(String),
    DisableService(String),
    IsEnabled(String),
    ReloadService(String),
    ResetFailed(Option<String>),
    StartService(String),
    IsolateService(String),
    StopService(String),
    RestartService(String),
    ServiceStatus(String),
    TransactionStatus(u64),
    Subscribe(Option<u64>),
    Shutdown(ShutdownAction),
}

impl Request {
    pub fn as_line(&self) -> String {
        match self {
            Self::Status => "STATUS\n".to_owned(),
            Self::Stop => "STOP\n".to_owned(),
            Self::Ping => "PING\n".to_owned(),
            Self::List => "LIST\n".to_owned(),
            Self::Reload => "RELOAD\n".to_owned(),
            Self::EnableService(name) => format!("ENABLE_SERVICE {name}\n"),
            Self::DisableService(name) => format!("DISABLE_SERVICE {name}\n"),
            Self::IsEnabled(name) => format!("IS_ENABLED {name}\n"),
            Self::ReloadService(name) => format!("RELOAD_SERVICE {name}\n"),
            Self::ResetFailed(name) => name.as_ref().map_or_else(
                || "RESET_FAILED\n".to_owned(),
                |name| format!("RESET_FAILED {name}\n"),
            ),
            Self::StartService(name) => format!("START_SERVICE {name}\n"),
            Self::IsolateService(name) => format!("ISOLATE_SERVICE {name}\n"),
            Self::StopService(name) => format!("STOP_SERVICE {name}\n"),
            Self::RestartService(name) => format!("RESTART_SERVICE {name}\n"),
            Self::ServiceStatus(name) => format!("SERVICE_STATUS {name}\n"),
            Self::TransactionStatus(id) => format!("TRANSACTION_STATUS {id}\n"),
            Self::Subscribe(since) => since.map_or_else(
                || "SUBSCRIBE\n".to_owned(),
                |since| format!("SUBSCRIBE {since}\n"),
            ),
            Self::Shutdown(action) => format!("SHUTDOWN {}\n", action.as_str()),
        }
    }

    pub fn as_bytes(&self) -> Vec<u8> {
        self.as_line().into_bytes()
    }

    pub fn parse(value: &str) -> Result<Self, ProtocolError> {
        let mut fields = value.split_whitespace();
        let command = fields
            .next()
            .ok_or_else(|| ProtocolError::UnknownRequest(value.trim().to_owned()))?;
        let request = match command {
            "STATUS" => Self::Status,
            "STOP" => Self::Stop,
            "PING" => Self::Ping,
            "LIST" => Self::List,
            "RELOAD" => Self::Reload,
            "ENABLE_SERVICE" => Self::EnableService(parse_service_name(&mut fields)?),
            "DISABLE_SERVICE" => Self::DisableService(parse_service_name(&mut fields)?),
            "IS_ENABLED" => Self::IsEnabled(parse_service_name(&mut fields)?),
            "RELOAD_SERVICE" => Self::ReloadService(parse_service_name(&mut fields)?),
            "RESET_FAILED" => Self::ResetFailed(fields.next().map(str::to_owned)),
            "START_SERVICE" => Self::StartService(parse_service_name(&mut fields)?),
            "ISOLATE_SERVICE" => Self::IsolateService(parse_service_name(&mut fields)?),
            "STOP_SERVICE" => Self::StopService(parse_service_name(&mut fields)?),
            "RESTART_SERVICE" => Self::RestartService(parse_service_name(&mut fields)?),
            "SERVICE_STATUS" => Self::ServiceStatus(parse_service_name(&mut fields)?),
            "TRANSACTION_STATUS" => Self::TransactionStatus(parse_transaction_id(&mut fields)?),
            "SUBSCRIBE" => Self::Subscribe(parse_optional_sequence(&mut fields)?),
            "SHUTDOWN" => Self::Shutdown(parse_shutdown_action(&mut fields)?),
            other => return Err(ProtocolError::UnknownRequest(other.to_owned())),
        };
        if fields.next().is_some() {
            return Err(ProtocolError::MalformedRequest(value.trim().to_owned()));
        }
        Ok(request)
    }
}

fn parse_shutdown_action<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
) -> Result<ShutdownAction, ProtocolError> {
    let value = fields
        .next()
        .ok_or_else(|| ProtocolError::MalformedRequest("shutdown action is required".to_owned()))?;
    ShutdownAction::parse(value).ok_or_else(|| {
        ProtocolError::MalformedRequest(format!("unsupported shutdown action {value}"))
    })
}

fn parse_optional_sequence<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
) -> Result<Option<u64>, ProtocolError> {
    fields
        .next()
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| ProtocolError::MalformedRequest("invalid event sequence".to_owned()))
        })
        .transpose()
}

fn parse_service_name<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
) -> Result<String, ProtocolError> {
    let name = fields
        .next()
        .ok_or_else(|| ProtocolError::MalformedRequest("service name is required".to_owned()))?;
    if name.is_empty() || name.contains(['\n', '\r']) {
        return Err(ProtocolError::MalformedRequest(
            "invalid service name".to_owned(),
        ));
    }
    Ok(name.to_owned())
}

fn parse_transaction_id<'a>(
    fields: &mut impl Iterator<Item = &'a str>,
) -> Result<u64, ProtocolError> {
    let value = fields
        .next()
        .ok_or_else(|| ProtocolError::MalformedRequest("transaction id is required".to_owned()))?;
    let id = value
        .parse::<u64>()
        .map_err(|_| ProtocolError::MalformedRequest("invalid transaction id".to_owned()))?;
    if id == 0 {
        return Err(ProtocolError::MalformedRequest(
            "transaction id must be non-zero".to_owned(),
        ));
    }
    Ok(id)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceStatus {
    pub name: String,
    pub state: String,
    pub pid: Option<u32>,
    pub generation: u64,
    pub restart_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionStatus {
    pub id: u64,
    pub operation: String,
    pub name: String,
    pub state: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Status {
        pid: u32,
        uptime_ms: u64,
    },
    Stopping,
    Pong,
    Subscribed,
    Event {
        line: String,
    },
    Services {
        names: Vec<String>,
    },
    Service(ServiceStatus),
    Transaction(TransactionStatus),
    Accepted {
        id: u64,
        operation: String,
        name: String,
    },
    Enabled {
        name: String,
    },
    Disabled {
        name: String,
    },
    EnabledStatus {
        name: String,
        enabled: bool,
    },
    Reloaded,
    Reset,
    Error {
        message: String,
    },
}

impl Response {
    pub fn as_line(&self) -> String {
        match self {
            Self::Status { pid, uptime_ms } => {
                format!("OK STATUS pid={pid} uptime_ms={uptime_ms}\n")
            }
            Self::Stopping => "OK STOPPING\n".to_owned(),
            Self::Pong => "OK PONG\n".to_owned(),
            Self::Subscribed => "OK SUBSCRIBED\n".to_owned(),
            Self::Event { line } => {
                format!(
                    "OK EVENT {}\n",
                    one_line(line.trim_end_matches(['\r', '\n']))
                )
            }
            Self::Services { names } => format!("OK SERVICES names={}\n", names.join(",")),
            Self::Service(status) => format!(
                "OK SERVICE name={} state={} pid={} generation={} restarts={}\n",
                status.name,
                status.state,
                status
                    .pid
                    .map_or_else(|| "-".to_owned(), |pid| pid.to_string()),
                status.generation,
                status.restart_count,
            ),
            Self::Transaction(status) => format!(
                "OK TRANSACTION id={} operation={} name={} state={}\n",
                status.id, status.operation, status.name, status.state
            ),
            Self::Accepted {
                id,
                operation,
                name,
            } => {
                format!("OK ACCEPTED id={id} operation={operation} name={name}\n")
            }
            Self::Enabled { name } => format!("OK ENABLED name={name}\n"),
            Self::Disabled { name } => format!("OK DISABLED name={name}\n"),
            Self::EnabledStatus { name, enabled } => {
                format!(
                    "OK ENABLED_STATUS name={name} enabled={}\n",
                    u8::from(*enabled)
                )
            }
            Self::Reloaded => "OK RELOADED\n".to_owned(),
            Self::Reset => "OK RESET_FAILED\n".to_owned(),
            Self::Error { message } => format!("ERR {}\n", one_line(message)),
        }
    }

    pub fn parse(value: &str) -> Result<Self, ProtocolError> {
        let line = value.trim();
        if let Some(rest) = line.strip_prefix("ERR ") {
            return Ok(Self::Error {
                message: rest.to_owned(),
            });
        }
        if line == "OK STOPPING" {
            return Ok(Self::Stopping);
        }
        if line == "OK PONG" {
            return Ok(Self::Pong);
        }
        if line == "OK SUBSCRIBED" {
            return Ok(Self::Subscribed);
        }
        if let Some(event) = line.strip_prefix("OK EVENT ") {
            if event.is_empty() {
                return Err(ProtocolError::MalformedResponse(line.to_owned()));
            }
            return Ok(Self::Event {
                line: event.to_owned(),
            });
        }
        if line == "OK RELOADED" {
            return Ok(Self::Reloaded);
        }
        if line == "OK RESET_FAILED" {
            return Ok(Self::Reset);
        }
        let mut fields = line.split_whitespace();
        if fields.next() != Some("OK") {
            return Err(ProtocolError::MalformedResponse(line.to_owned()));
        }
        match fields.next() {
            Some("STATUS") => {
                let pid = parse_field(&mut fields, "pid")?;
                let uptime_ms = parse_field(&mut fields, "uptime_ms")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::Status { pid, uptime_ms })
            }
            Some("SERVICES") => {
                let names = parse_field::<String>(&mut fields, "names")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                let names = if names.is_empty() {
                    Vec::new()
                } else {
                    names.split(',').map(str::to_owned).collect()
                };
                Ok(Self::Services { names })
            }
            Some("SERVICE") => {
                let name = parse_field(&mut fields, "name")?;
                let state = parse_field(&mut fields, "state")?;
                let pid = parse_field::<String>(&mut fields, "pid")?;
                let generation = parse_field(&mut fields, "generation")?;
                let restart_count = parse_field(&mut fields, "restarts")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                let pid = if pid == "-" {
                    None
                } else {
                    Some(
                        pid.parse()
                            .map_err(|_| ProtocolError::MalformedResponse(line.to_owned()))?,
                    )
                };
                Ok(Self::Service(ServiceStatus {
                    name,
                    state,
                    pid,
                    generation,
                    restart_count,
                }))
            }
            Some("TRANSACTION") => {
                let id = parse_field(&mut fields, "id")?;
                let operation = parse_field(&mut fields, "operation")?;
                let name = parse_field(&mut fields, "name")?;
                let state = parse_field(&mut fields, "state")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::Transaction(TransactionStatus {
                    id,
                    operation,
                    name,
                    state,
                }))
            }
            Some("ACCEPTED") => {
                let id = parse_field(&mut fields, "id")?;
                let operation = parse_field(&mut fields, "operation")?;
                let name = parse_field(&mut fields, "name")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::Accepted {
                    id,
                    operation,
                    name,
                })
            }
            Some("ENABLED") => {
                let name = parse_field(&mut fields, "name")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::Enabled { name })
            }
            Some("DISABLED") => {
                let name = parse_field(&mut fields, "name")?;
                if fields.next().is_some() {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::Disabled { name })
            }
            Some("ENABLED_STATUS") => {
                let name = parse_field(&mut fields, "name")?;
                let enabled = parse_field::<u8>(&mut fields, "enabled")?;
                if fields.next().is_some() || enabled > 1 {
                    return Err(ProtocolError::MalformedResponse(line.to_owned()));
                }
                Ok(Self::EnabledStatus {
                    name,
                    enabled: enabled == 1,
                })
            }
            _ => Err(ProtocolError::MalformedResponse(line.to_owned())),
        }
    }

    pub fn write_to(&self, stream: &mut UnixStream) -> io::Result<()> {
        stream.write_all(self.as_line().as_bytes())
    }
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' | '\0' => ' ',
            other => other,
        })
        .collect()
}

fn parse_field<'a, T>(
    fields: &mut impl Iterator<Item = &'a str>,
    name: &str,
) -> Result<T, ProtocolError>
where
    T: std::str::FromStr,
{
    let field = fields
        .next()
        .ok_or_else(|| ProtocolError::MalformedResponse(format!("missing {name}")))?;
    let (field_name, value) = field
        .split_once('=')
        .ok_or_else(|| ProtocolError::MalformedResponse(field.to_owned()))?;
    if field_name != name {
        return Err(ProtocolError::MalformedResponse(field.to_owned()));
    }
    value
        .parse()
        .map_err(|_| ProtocolError::MalformedResponse(field.to_owned()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    UnknownRequest(String),
    MalformedRequest(String),
    MalformedResponse(String),
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRequest(value) => write!(formatter, "unknown request {value:?}"),
            Self::MalformedRequest(value) => write!(formatter, "malformed request {value:?}"),
            Self::MalformedResponse(value) => write!(formatter, "malformed response {value:?}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

pub fn remove_stale_socket(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trips_status() {
        let response = Response::Status {
            pid: 123,
            uptime_ms: 456,
        };
        assert_eq!(Response::parse(&response.as_line()), Ok(response));
    }

    #[test]
    fn protocol_rejects_unknown_requests() {
        assert_eq!(
            Request::parse("BOGUS"),
            Err(ProtocolError::UnknownRequest("BOGUS".to_owned()))
        );
    }

    #[test]
    fn service_requests_round_trip() {
        let requests = [
            Request::StartService("web.svc".to_owned()),
            Request::IsolateService("boot.profile".to_owned()),
            Request::StopService("web.svc".to_owned()),
            Request::RestartService("web.svc".to_owned()),
            Request::ResetFailed(None),
            Request::ResetFailed(Some("web.svc".to_owned())),
            Request::ServiceStatus("web.svc".to_owned()),
            Request::TransactionStatus(41),
            Request::Subscribe(None),
            Request::Subscribe(Some(0)),
            Request::Shutdown(ShutdownAction::Poweroff),
            Request::Shutdown(ShutdownAction::Reboot),
            Request::Shutdown(ShutdownAction::Halt),
        ];
        for request in requests {
            assert_eq!(Request::parse(&request.as_line()), Ok(request));
        }
    }

    #[test]
    fn reset_response_round_trips() {
        assert_eq!(
            Response::parse(&Response::Reset.as_line()),
            Ok(Response::Reset)
        );
    }

    #[test]
    fn accepted_response_round_trips_a_transaction_id() {
        let response = Response::Accepted {
            id: 41,
            operation: "start".to_owned(),
            name: "web.svc".to_owned(),
        };
        assert_eq!(Response::parse(&response.as_line()), Ok(response));
    }

    #[test]
    fn transaction_status_response_round_trips() {
        let response = Response::Transaction(TransactionStatus {
            id: 41,
            operation: "start".to_owned(),
            name: "web.svc".to_owned(),
            state: "pending".to_owned(),
        });
        assert_eq!(Response::parse(&response.as_line()), Ok(response));
    }

    #[test]
    fn event_subscription_responses_round_trip() {
        assert_eq!(
            Response::parse(&Response::Subscribed.as_line()),
            Ok(Response::Subscribed)
        );
        let response = Response::Event {
            line: "seq=4 service=web.svc state=running".to_owned(),
        };
        assert_eq!(Response::parse(&response.as_line()), Ok(response));
    }

    #[test]
    fn service_status_round_trips_without_a_pid() {
        let response = Response::Service(ServiceStatus {
            name: "web.svc".to_owned(),
            state: "defined".to_owned(),
            pid: None,
            generation: 2,
            restart_count: 0,
        });
        assert_eq!(Response::parse(&response.as_line()), Ok(response));
    }

    #[test]
    fn runtime_paths_are_consistent() {
        let paths = RuntimePaths::new("/run/example");
        assert_eq!(paths.directory, PathBuf::from("/run/example"));
        assert_eq!(paths.socket, PathBuf::from("/run/example/control.sock"));
        assert_eq!(paths.pid, PathBuf::from("/run/example/pid"));
    }

    #[test]
    fn service_enablement_accepts_the_native_suffix() {
        let directory = PathBuf::from(format!("/tmp/fractald-state-test-{}", std::process::id()));
        let paths = StatePaths {
            enabled: directory.join("enabled"),
            masked: directory.join("masked"),
            events: directory.join("events.log"),
            directory,
        };
        paths.enable("demo").expect("enable service");
        assert!(paths.is_enabled("demo.svc").expect("inspect service"));
        paths.disable("demo.svc").expect("disable service");
        assert!(!paths.is_enabled("demo").expect("inspect disabled service"));
        paths.mask("demo").expect("mask service");
        assert!(paths.is_masked("demo.svc").expect("inspect masked service"));
        paths.unmask("demo.svc").expect("unmask service");
        assert!(!paths.is_masked("demo").expect("inspect unmasked service"));
        let _ = fs::remove_dir_all(paths.directory);
    }
}
