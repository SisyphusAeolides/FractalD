use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io;
use std::os::fd::FromRawFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fractald-journald: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    for argument in env::args().skip(1) {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "fractald-journald (FractalD)\n\nUsage: fractald-journald [--system|--user]\n\nReceives native journal datagrams and writes FractalD's local per-unit sink."
                );
                return Ok(());
            }
            "--version" => {
                println!("fractald-journald (FractalD) {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--system" | "--user" | "--unit=" | "--namespace=" => {}
            value if value.starts_with("--unit=") || value.starts_with("--namespace=") => {}
            value => return Err(format!("unsupported option {value}")),
        }
    }

    let endpoint = env::var_os("FRACTALD_JOURNAL_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/fractald/journal/socket"));
    let sockets = match inherited_sockets()? {
        Some(sockets) => sockets,
        None => vec![bind_endpoint(&endpoint)?],
    };
    let log_directory = log_directory();
    fs::create_dir_all(&log_directory)
        .map_err(|error| format!("cannot create {}: {error}", log_directory.display()))?;
    let mut sockets = sockets.into_iter();
    let primary = sockets
        .next()
        .ok_or_else(|| "journal receiver has no sockets".to_owned())?;
    for socket in sockets {
        let log_directory = log_directory.clone();
        std::thread::spawn(move || {
            let _ = receive_loop(socket, &log_directory);
        });
    }
    receive_loop(primary, &log_directory)
}

fn inherited_sockets() -> Result<Option<Vec<UnixDatagram>>, String> {
    let Some(value) = env::var_os("LISTEN_FDS") else {
        return Ok(None);
    };
    let count = value
        .to_string_lossy()
        .parse::<usize>()
        .map_err(|_| "LISTEN_FDS is not a valid descriptor count".to_owned())?;
    if count == 0 {
        return Ok(None);
    }
    if count > 32 {
        return Err("LISTEN_FDS exceeds the journal receiver limit".to_owned());
    }
    if let Some(pid) = env::var_os("LISTEN_PID") {
        let pid = pid
            .to_string_lossy()
            .parse::<u32>()
            .map_err(|_| "LISTEN_PID is not a valid process id".to_owned())?;
        if pid != std::process::id() {
            return Ok(None);
        }
    }
    let sockets = (0..count)
        .map(|index| {
            // The supervisor's activation boundary places descriptors at the
            // conventional fd 3 base and clears close-on-exec on them.
            Ok(unsafe { UnixDatagram::from_raw_fd(3 + index as i32) })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(Some(sockets))
}

fn bind_endpoint(endpoint: &Path) -> Result<UnixDatagram, String> {
    let filesystem_endpoint = endpoint.as_os_str().as_bytes().first() != Some(&b'@');
    if filesystem_endpoint {
        if let Some(parent) = endpoint.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        match fs::symlink_metadata(&endpoint) {
            Ok(metadata) if metadata.is_dir() => {
                return Err(format!(
                    "journal endpoint is a directory: {}",
                    endpoint.display()
                ));
            }
            Ok(_) => fs::remove_file(&endpoint)
                .map_err(|error| format!("cannot replace {}: {error}", endpoint.display()))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("cannot inspect {}: {error}", endpoint.display()));
            }
        }
    }
    if filesystem_endpoint {
        UnixDatagram::bind(&endpoint)
            .map_err(|error| format!("cannot bind {}: {error}", endpoint.display()))
    } else {
        let address = SocketAddr::from_abstract_name(&endpoint.as_os_str().as_bytes()[1..])
            .map_err(|error| format!("invalid abstract journal endpoint: {error}"))?;
        UnixDatagram::bind_addr(&address)
            .map_err(|error| format!("cannot bind abstract journal endpoint: {error}"))
    }
}

fn receive_loop(socket: UnixDatagram, log_directory: &Path) -> Result<(), String> {
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let length = match socket.recv(&mut buffer) {
            Ok(length) => length,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }
            Err(error) => return Err(format!("cannot receive journal datagram: {error}")),
        };
        let fields = parse_fields(&buffer[..length]);
        let unit = fields
            .get("FRACTALD_SERVICE")
            .map(String::as_str)
            .unwrap_or("fractald-journald");
        let stream = fields
            .get("_STREAM")
            .map(String::as_str)
            .unwrap_or("stdout");
        let message = fields.get("MESSAGE").map(String::as_str).unwrap_or("");
        append_record(log_directory, unit, stream, message)?;
    }
}

fn parse_fields(payload: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8_lossy(payload)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

fn append_record(directory: &Path, unit: &str, stream: &str, message: &str) -> Result<(), String> {
    let unit = safe_component(unit, "fractald-journald");
    let stream = safe_component(stream, "stdout");
    let path = directory.join(format!("{unit}.{stream}.log"));
    let mut file = OpenOptions::new();
    file.create(true).append(true).write(true).mode(0o600);
    let mut file = file
        .open(&path)
        .map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    use std::io::Write;
    file.write_all(message.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(())
}

fn safe_component(value: &str, fallback: &str) -> String {
    let value = value
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '@' | '_' | '-')
        })
        .take(200)
        .collect::<String>();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value
    }
}

fn log_directory() -> PathBuf {
    if let Some(path) = env::var_os("FRACTALD_LOG_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
        return PathBuf::from(path).join("logs");
    }
    env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".local/state/fractald/logs"))
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "/tmp/fractald-logs-{}",
                fractald_platform::effective_uid()
            ))
        })
}
