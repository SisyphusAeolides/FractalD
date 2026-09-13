use std::env;
use std::io;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::PathBuf;

#[derive(Debug)]
pub struct NativeJournal {
    socket: UnixDatagram,
}

impl NativeJournal {
    pub fn connect() -> Option<Self> {
        let endpoint = env::var_os("FRACTALD_JOURNAL_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/run/fractald/journal/socket"));
        let socket = UnixDatagram::unbound().ok()?;
        if endpoint.as_os_str().as_bytes().first() == Some(&b'@') {
            let address =
                SocketAddr::from_abstract_name(&endpoint.as_os_str().as_bytes()[1..]).ok()?;
            socket.connect_addr(&address).ok()?;
        } else {
            socket.connect(&endpoint).ok()?;
        }
        Some(Self { socket })
    }

    pub fn send(&self, unit: &str, stream: &str, pid: u32, message: &[u8]) -> io::Result<()> {
        self.socket
            .send(&encode_entry(unit, stream, pid, message))
            .map(|_| ())
    }
}

pub fn encode_entry(unit: &str, stream: &str, pid: u32, message: &[u8]) -> Vec<u8> {
    let message = message
        .iter()
        .copied()
        .filter(|byte| *byte != b'\0' && *byte != b'\n')
        .collect::<Vec<_>>();
    let mut payload = Vec::with_capacity(message.len() + unit.len() + stream.len() + 80);
    payload.extend_from_slice(b"MESSAGE=");
    payload.extend_from_slice(&message);
    payload.extend_from_slice(b"\nFRACTALD_SERVICE=");
    payload.extend_from_slice(unit.as_bytes());
    payload.extend_from_slice(b"\n_PID=");
    payload.extend_from_slice(pid.to_string().as_bytes());
    payload.extend_from_slice(b"\n_STREAM=");
    payload.extend_from_slice(stream.as_bytes());
    payload.extend_from_slice(b"\n_TRANSPORT=stdout\n");
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_identity_and_sanitizes_message_boundaries() {
        let payload = encode_entry("example.svc", "stderr", 1234, b"hello\0world\n");
        let payload = String::from_utf8(payload).expect("payload");
        assert_eq!(
            payload,
            "MESSAGE=helloworld\nFRACTALD_SERVICE=example.svc\n_PID=1234\n_STREAM=stderr\n_TRANSPORT=stdout\n"
        );
    }

    #[test]
    fn rejects_missing_native_socket_without_affecting_local_mode() {
        let previous = env::var_os("FRACTALD_JOURNAL_SOCKET");
        unsafe {
            env::set_var(
                "FRACTALD_JOURNAL_SOCKET",
                format!("/tmp/fractald-missing-journal-{}", std::process::id()),
            );
        }
        assert!(NativeJournal::connect().is_none());
        match previous {
            Some(value) => unsafe { env::set_var("FRACTALD_JOURNAL_SOCKET", value) },
            None => unsafe { env::remove_var("FRACTALD_JOURNAL_SOCKET") },
        }
    }
}
