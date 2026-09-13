use std::env;
use std::ffi::CString;
use std::ffi::OsString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::process::ExitCode;

use fractald_control::{Request, Response, RuntimePaths};

fn main() -> ExitCode {
    match run(env::args_os().skip(1)) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("fractald-notify: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct NotifyOptions {
    fields: Vec<String>,
    booted: bool,
    quiet: bool,
    help: bool,
    version: bool,
}

fn run(args: impl IntoIterator<Item = OsString>) -> Result<u8, String> {
    let options = parse_args(args)?;
    if options.help {
        print_help();
        return Ok(0);
    }
    if options.version {
        println!("fractald-notify (FractalD) {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if options.booted && !manager_is_booted() {
        return Ok(1);
    }
    if options.fields.is_empty() {
        return Ok(0);
    }
    let socket = env::var_os("FRACTALD_NOTIFY_SOCKET")
        .ok_or_else(|| "FRACTALD_NOTIFY_SOCKET is not set".to_owned())?;
    let payload = options.fields.join("\n") + "\n";
    send_notification(socket.as_os_str().as_bytes(), payload.as_bytes())
        .map_err(|error| format!("cannot send notification: {error}"))?;
    Ok(0)
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<NotifyOptions, String> {
    let mut options = NotifyOptions::default();
    let mut values = args.into_iter().peekable();
    while let Some(value) = values.next() {
        let value = value
            .into_string()
            .map_err(|_| "arguments must be valid UTF-8".to_owned())?;
        match value.as_str() {
            "--ready" => options.fields.push("READY=1".to_owned()),
            "--reloading" => options.fields.push("RELOADING=1".to_owned()),
            "--stopping" => options.fields.push("STOPPING=1".to_owned()),
            "--watchdog" => options.fields.push("WATCHDOG=1".to_owned()),
            "--booted" => options.booted = true,
            "--quiet" => options.quiet = true,
            "--no-block" | "--wait" => {}
            "--help" | "-h" => options.help = true,
            "--version" => options.version = true,
            "--status" => {
                let status = values
                    .next()
                    .ok_or_else(|| "--status requires a value".to_owned())?
                    .into_string()
                    .map_err(|_| "--status must be valid UTF-8".to_owned())?;
                options.fields.push(format!("STATUS={status}"));
            }
            value if value.starts_with("--status=") => {
                options.fields.push(format!("STATUS={}", &value[9..]));
            }
            "--pid" => {
                let pid = values
                    .next()
                    .ok_or_else(|| "--pid requires a value".to_owned())?
                    .into_string()
                    .map_err(|_| "--pid must be valid UTF-8".to_owned())?;
                validate_pid(&pid)?;
                options.fields.push(format!("MAINPID={pid}"));
            }
            value if value.starts_with("--pid=") => {
                let pid = &value[6..];
                validate_pid(pid)?;
                options.fields.push(format!("MAINPID={pid}"));
            }
            "--uid" | "--gid" | "--fd" | "--fdname" | "--machine" => {
                let _ = values
                    .next()
                    .ok_or_else(|| format!("{value} requires a value"))?;
            }
            value
                if value.starts_with("--uid=")
                    || value.starts_with("--gid=")
                    || value.starts_with("--fd=")
                    || value.starts_with("--fdname=")
                    || value.starts_with("--machine=") => {}
            "--" => {
                for value in values {
                    let value = value
                        .into_string()
                        .map_err(|_| "notification fields must be valid UTF-8".to_owned())?;
                    validate_field(&value)?;
                    options.fields.push(value);
                }
                break;
            }
            value if value.contains('=') => {
                validate_field(value)?;
                options.fields.push(value.to_owned());
            }
            other => return Err(format!("unsupported option or field {other:?}")),
        }
    }
    if options.quiet {
        // The option only suppresses diagnostics; notifications remain unchanged.
    }
    Ok(options)
}

fn validate_pid(value: &str) -> Result<(), String> {
    let pid = value
        .parse::<u32>()
        .map_err(|_| format!("invalid PID {value}"))?;
    if pid == 0 {
        Err(format!("invalid PID {value}"))
    } else {
        Ok(())
    }
}

fn validate_field(value: &str) -> Result<(), String> {
    let Some((key, _)) = value.split_once('=') else {
        return Err(format!("notification field {value:?} must use KEY=VALUE"));
    };
    if key.is_empty()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(format!("invalid notification field name {key:?}"));
    }
    Ok(())
}

fn send_notification(address: &[u8], payload: &[u8]) -> io::Result<()> {
    let address = CString::new(address).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "FRACTALD_NOTIFY_SOCKET contains NUL",
        )
    })?;
    fractald_platform::send_unix_datagram(&address, payload)
}

fn manager_is_booted() -> bool {
    if std::process::id() == 1 {
        return true;
    }
    matches!(
        RuntimePaths::from_environment().request(Request::Status),
        Ok(Response::Status { .. })
    )
}

fn print_help() {
    println!(
        "fractald-notify (FractalD)\n\nUsage: fractald-notify [OPTIONS] [KEY=VALUE ...]\n\n  --ready              report service readiness\n  --status STATUS      report service status\n  --watchdog           report a watchdog heartbeat\n  --reloading          report configuration reload\n  --stopping           report service shutdown\n  --pid PID            report the main process ID\n  --booted             test whether FractalD is running\n  --no-block, --wait   accept standard compatibility options\n  --help               show this help\n  --version            show the version"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram;
    use std::time::Duration;

    #[test]
    fn parses_common_notify_fields() {
        let options = parse_args([
            OsString::from("--ready"),
            OsString::from("--status=ready"),
            OsString::from("--pid"),
            OsString::from("42"),
            OsString::from("EXTEND_TIMEOUT_USEC=500000"),
        ])
        .expect("notify arguments");
        assert_eq!(
            options.fields,
            [
                "READY=1",
                "STATUS=ready",
                "MAINPID=42",
                "EXTEND_TIMEOUT_USEC=500000"
            ]
        );
    }

    #[test]
    fn rejects_invalid_notification_fields() {
        assert!(parse_args([OsString::from("bad field")]).is_err());
        assert!(parse_args([OsString::from("--pid=0")]).is_err());
    }

    #[test]
    fn sends_notifications_to_filesystem_sockets() {
        let path =
            std::env::temp_dir().join(format!("fractald-notify-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let receiver = UnixDatagram::bind(&path).expect("notify socket");
        receiver
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("notify timeout");
        send_notification(path.as_os_str().as_bytes(), b"READY=1\nSTATUS=ready\n")
            .expect("send notification");
        let mut payload = [0_u8; 128];
        let length = receiver.recv(&mut payload).expect("receive notification");
        assert_eq!(&payload[..length], b"READY=1\nSTATUS=ready\n");
        let _ = std::fs::remove_file(path);
    }
}
