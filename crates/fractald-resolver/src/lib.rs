use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const DISCOVERY_VERSION: u32 = 1;
pub const DEFAULT_DISCOVERY_FILE: &str = "resolved.endpoint";
pub const DEFAULT_CONTROL_FILE: &str = "resolved.control";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryRecord {
    pub version: u32,
    pub pid: u32,
    pub listen: SocketAddr,
    pub control: PathBuf,
    pub upstreams: Vec<SocketAddr>,
    pub cache_entries: usize,
    pub cache_limit: usize,
}

impl DiscoveryRecord {
    pub fn new(
        pid: u32,
        listen: SocketAddr,
        control: PathBuf,
        upstreams: Vec<SocketAddr>,
        cache_limit: usize,
    ) -> Self {
        Self {
            version: DISCOVERY_VERSION,
            pid,
            listen,
            control,
            upstreams,
            cache_entries: 0,
            cache_limit,
        }
    }

    pub fn encode(&self) -> io::Result<Vec<u8>> {
        if self.version != DISCOVERY_VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unsupported resolver discovery version",
            ));
        }
        let control = self.control.to_str().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "resolver control path is not valid UTF-8",
            )
        })?;
        if control.contains(['\n', '\r', '=']) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "resolver control path contains a discovery delimiter",
            ));
        }
        let mut output = format!(
            "version={}\npid={}\nlisten={}\ncontrol={}\ncache_entries={}\ncache_limit={}\n",
            self.version, self.pid, self.listen, control, self.cache_entries, self.cache_limit
        );
        for upstream in &self.upstreams {
            output.push_str("upstream=");
            output.push_str(&upstream.to_string());
            output.push('\n');
        }
        Ok(output.into_bytes())
    }

    pub fn parse(contents: &[u8]) -> Result<Self, String> {
        let contents = std::str::from_utf8(contents)
            .map_err(|error| format!("resolver discovery is not UTF-8: {error}"))?;
        let mut version = None;
        let mut pid = None;
        let mut listen = None;
        let mut control = None;
        let mut upstreams = Vec::new();
        let mut cache_entries = 0;
        let mut cache_limit = 0;
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("invalid resolver discovery line: {line}"))?;
            match key {
                "version" => version = Some(parse_value(value, "version")?),
                "pid" => pid = Some(parse_value(value, "pid")?),
                "listen" => {
                    listen = Some(
                        value
                            .parse()
                            .map_err(|error| format!("invalid resolver listen address: {error}"))?,
                    )
                }
                "control" => control = Some(PathBuf::from(value)),
                "upstream" => upstreams.push(
                    value
                        .parse()
                        .map_err(|error| format!("invalid resolver upstream address: {error}"))?,
                ),
                "cache_entries" => cache_entries = parse_value(value, "cache_entries")?,
                "cache_limit" => cache_limit = parse_value(value, "cache_limit")?,
                _ => {}
            }
        }
        if version != Some(DISCOVERY_VERSION) {
            return Err("unsupported or missing resolver discovery version".to_owned());
        }
        let control = control.ok_or_else(|| "resolver discovery has no control path".to_owned())?;
        if control.as_os_str().is_empty() {
            return Err("resolver discovery has an empty control path".to_owned());
        }
        Ok(Self {
            version: DISCOVERY_VERSION,
            pid: pid.ok_or_else(|| "resolver discovery has no PID".to_owned())?,
            listen: listen.ok_or_else(|| "resolver discovery has no listen address".to_owned())?,
            control,
            upstreams,
            cache_entries,
            cache_limit,
        })
    }
}

fn parse_value<T>(value: &str, field: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    value
        .parse()
        .map_err(|error| format!("invalid resolver discovery {field}: {error}"))
}

pub fn read_discovery(path: &Path) -> io::Result<DiscoveryRecord> {
    let contents = fs::read(path)?;
    DiscoveryRecord::parse(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn default_discovery_path() -> PathBuf {
    if let Some(path) = env::var_os("FRACTALD_RESOLVED_DISCOVERY") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("FRACTALD_RUNTIME_DIR") {
        return PathBuf::from(path).join(DEFAULT_DISCOVERY_FILE);
    }
    if let Some(path) = env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(path)
            .join("fractald")
            .join(DEFAULT_DISCOVERY_FILE);
    }
    if fractald_platform::is_root() {
        PathBuf::from("/run/fractald").join(DEFAULT_DISCOVERY_FILE)
    } else {
        PathBuf::from(format!(
            "/tmp/fractald-{}/{}",
            fractald_platform::effective_uid(),
            DEFAULT_DISCOVERY_FILE
        ))
    }
}

pub fn default_control_path(discovery: &Path) -> PathBuf {
    if let Some(path) = env::var_os("FRACTALD_RESOLVED_CONTROL") {
        return PathBuf::from(path);
    }
    discovery
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(DEFAULT_CONTROL_FILE)
}

pub struct DiscoveryLease {
    path: PathBuf,
    contents: Vec<u8>,
}

impl DiscoveryLease {
    pub fn publish(path: impl Into<PathBuf>, record: &DiscoveryRecord) -> io::Result<Self> {
        let path = path.into();
        let contents = record.encode()?;
        write_atomic(&path, &contents)?;
        Ok(Self { path, contents })
    }

    pub fn update(&mut self, record: &DiscoveryRecord) -> io::Result<()> {
        let contents = record.encode()?;
        write_atomic(&self.path, &contents)?;
        self.contents = contents;
        Ok(())
    }
}

impl Drop for DiscoveryLease {
    fn drop(&mut self) {
        if let Ok(contents) = fs::read(&self.path) {
            if contents == self.contents {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("resolved"),
        std::process::id(),
        nonce
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

static QUERY_ID: AtomicU16 = AtomicU16::new(0xF001);

pub fn build_query(name: &str, record_type: u16) -> Result<Vec<u8>, String> {
    let name = name.trim_end_matches('.');
    if name.is_empty() {
        return Err("DNS name is empty".to_owned());
    }
    let mut packet = Vec::with_capacity(name.len() + 18);
    packet.extend_from_slice(&QUERY_ID.fetch_add(1, Ordering::Relaxed).to_be_bytes());
    packet.extend_from_slice(&0x0100_u16.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    packet.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
    let mut encoded = 0;
    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 || !label.is_ascii() {
            return Err(format!("invalid DNS label in {name}"));
        }
        packet.push(label.len() as u8);
        packet.extend_from_slice(label.as_bytes());
        encoded += label.len() + 1;
    }
    if encoded > 255 {
        return Err("DNS name is too long".to_owned());
    }
    packet.push(0);
    packet.extend_from_slice(&record_type.to_be_bytes());
    packet.extend_from_slice(&1_u16.to_be_bytes());
    Ok(packet)
}

pub fn send_query(endpoint: SocketAddr, query: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
    let bind_address = if endpoint.is_ipv6() {
        SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 0)
    } else {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0)
    };
    let socket = UdpSocket::bind(bind_address)?;
    socket.set_read_timeout(Some(timeout))?;
    socket.send_to(query, endpoint)?;
    let mut response = [0_u8; 4096];
    let (length, source) = socket.recv_from(&mut response)?;
    if source != endpoint {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DNS response came from an unexpected endpoint",
        ));
    }
    if length < 2 || query.get(0..2) != response.get(0..2) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "DNS response ID does not match the query",
        ));
    }
    Ok(response[..length].to_vec())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DnsAnswer {
    pub name: String,
    pub record_type: u16,
    pub class: u16,
    pub ttl: u32,
    pub data: String,
}

pub fn parse_answers(packet: &[u8]) -> Result<Vec<DnsAnswer>, String> {
    if packet.len() < 12 {
        return Err("DNS response is truncated".to_owned());
    }
    let response_code = packet[3] & 0x0f;
    if response_code != 0 {
        return Err(format!("DNS server returned response code {response_code}"));
    }
    let mut offset = 12;
    let questions = u16::from_be_bytes([packet[4], packet[5]]) as usize;
    for _ in 0..questions {
        let (_, end) = read_name(packet, offset, 0)?;
        offset = end
            .checked_add(4)
            .ok_or_else(|| "DNS question overflow".to_owned())?;
        if offset > packet.len() {
            return Err("DNS question is truncated".to_owned());
        }
    }
    let answers = u16::from_be_bytes([packet[6], packet[7]]) as usize;
    let mut output = Vec::with_capacity(answers);
    for _ in 0..answers {
        let (name, name_end) = read_name(packet, offset, 0)?;
        offset = name_end;
        if offset + 10 > packet.len() {
            return Err("DNS answer header is truncated".to_owned());
        }
        let record_type = u16::from_be_bytes([packet[offset], packet[offset + 1]]);
        let class = u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]);
        let ttl = u32::from_be_bytes([
            packet[offset + 4],
            packet[offset + 5],
            packet[offset + 6],
            packet[offset + 7],
        ]);
        let data_length = u16::from_be_bytes([packet[offset + 8], packet[offset + 9]]) as usize;
        let data_start = offset + 10;
        let data_end = data_start
            .checked_add(data_length)
            .ok_or_else(|| "DNS answer overflow".to_owned())?;
        if data_end > packet.len() {
            return Err("DNS answer data is truncated".to_owned());
        }
        let data = format_rdata(packet, data_start, data_length, record_type)?;
        output.push(DnsAnswer {
            name,
            record_type,
            class,
            ttl,
            data,
        });
        offset = data_end;
    }
    Ok(output)
}

fn format_rdata(
    packet: &[u8],
    start: usize,
    length: usize,
    record_type: u16,
) -> Result<String, String> {
    let end = start
        .checked_add(length)
        .ok_or_else(|| "DNS record overflow".to_owned())?;
    let data = packet
        .get(start..end)
        .ok_or_else(|| "DNS record is truncated".to_owned())?;
    match record_type {
        1 if length == 4 => Ok(Ipv4Addr::new(data[0], data[1], data[2], data[3]).to_string()),
        28 if length == 16 => {
            let mut bytes = [0_u8; 16];
            bytes.copy_from_slice(data);
            Ok(Ipv6Addr::from(bytes).to_string())
        }
        2 | 5 | 12 => {
            let (name, _) = read_name(packet, start, 0)?;
            Ok(name)
        }
        16 => Ok(String::from_utf8_lossy(data).into_owned()),
        _ => Ok(data.iter().map(|byte| format!("{byte:02x}")).collect()),
    }
}

fn read_name(packet: &[u8], start: usize, depth: usize) -> Result<(String, usize), String> {
    if depth > 16 || start >= packet.len() {
        return Err("invalid or cyclic DNS name".to_owned());
    }
    let mut offset = start;
    let mut labels = Vec::new();
    loop {
        let length = *packet
            .get(offset)
            .ok_or_else(|| "DNS name is truncated".to_owned())?;
        if length & 0xc0 == 0xc0 {
            let second = *packet
                .get(offset + 1)
                .ok_or_else(|| "DNS name pointer is truncated".to_owned())?;
            let pointer = (((length as usize & 0x3f) << 8) | second as usize) as usize;
            let (suffix, _) = read_name(packet, pointer, depth + 1)?;
            if !suffix.is_empty() {
                labels.push(suffix);
            }
            return Ok((labels.join("."), offset + 2));
        }
        if length & 0xc0 != 0 || length > 63 {
            return Err("invalid DNS label length".to_owned());
        }
        offset += 1;
        if length == 0 {
            return Ok((labels.join("."), offset));
        }
        let end = offset
            .checked_add(length as usize)
            .ok_or_else(|| "DNS label overflow".to_owned())?;
        let label = packet
            .get(offset..end)
            .ok_or_else(|| "DNS label is truncated".to_owned())?;
        labels.push(String::from_utf8_lossy(label).into_owned());
        offset = end;
    }
}

pub fn parse_record_type(value: &str) -> Result<u16, String> {
    match value.to_ascii_uppercase().as_str() {
        "A" => Ok(1),
        "AAAA" => Ok(28),
        "CNAME" => Ok(5),
        "PTR" => Ok(12),
        "TXT" => Ok(16),
        "NS" => Ok(2),
        "ANY" => Ok(255),
        value => value
            .parse()
            .map_err(|error| format!("unknown DNS record type {value}: {error}")),
    }
}

pub fn read_to_string<R: Read>(reader: &mut R) -> io::Result<String> {
    let mut value = String::new();
    reader.read_to_string(&mut value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn record(path: &Path) -> DiscoveryRecord {
        DiscoveryRecord::new(
            123,
            "127.0.0.53:5353".parse().expect("listen"),
            path.to_owned(),
            vec!["1.1.1.1:53".parse().expect("upstream")],
            64,
        )
    }

    #[test]
    fn discovery_round_trip_preserves_endpoint() {
        let path = std::env::temp_dir().join(format!(
            "fractald-resolver-test-{}-endpoint",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let value = record(Path::new("/tmp/fractald/resolved.control"));
        let lease = DiscoveryLease::publish(&path, &value).expect("publish");
        assert_eq!(read_discovery(&path).expect("read"), value);
        drop(lease);
        assert!(!path.exists());
    }

    #[test]
    fn query_encodes_labels_and_record_type() {
        let packet = build_query("example.com.", 28).expect("query");
        assert_eq!(packet[2..4], [1, 0]);
        assert_eq!(packet[12], 7);
        assert_eq!(&packet[13..20], b"example");
        assert_eq!(&packet[25..27], &[0, 28]);
    }

    #[test]
    fn parses_compressed_a_answer() {
        let mut packet = build_query("example.com", 1).expect("query");
        packet[2] = 0x81;
        packet[3] = 0x80;
        packet[6] = 0;
        packet[7] = 1;
        packet.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 192, 0, 2, 10]);
        let answers = parse_answers(&packet).expect("answers");
        assert_eq!(answers[0].data, "192.0.2.10");
        assert_eq!(answers[0].ttl, 30);
    }

    #[test]
    fn rejects_invalid_dns_names() {
        assert!(build_query("example..com", 1).is_err());
        assert!(build_query("éxample.com", 1).is_err());
    }
}
