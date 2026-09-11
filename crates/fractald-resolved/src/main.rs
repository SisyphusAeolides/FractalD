use fractald_resolver::{
    DiscoveryLease, DiscoveryRecord, default_control_path, default_discovery_path,
};
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, UdpSocket};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() -> ExitCode {
    if let Err(error) = fractald_platform::install_shutdown_handlers() {
        eprintln!("fractald-resolved: cannot install shutdown handlers: {error}");
        return ExitCode::from(1);
    }
    let config = match ResolverConfig::from_environment() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("fractald-resolved: {error}");
            return ExitCode::from(1);
        }
    };
    if let Err(error) = serve(config) {
        eprintln!("fractald-resolved: {error}");
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[derive(Clone, Debug)]
struct ResolverConfig {
    listen: SocketAddr,
    upstreams: Vec<SocketAddr>,
    timeout: Duration,
    max_cache_entries: usize,
    fault: ResolverFault,
    discovery: PathBuf,
    control: PathBuf,
}

impl ResolverConfig {
    fn from_environment() -> Result<Self, String> {
        let listen = env::var("FRACTALD_RESOLVED_LISTEN")
            .unwrap_or_else(|_| "127.0.0.53:53".to_owned())
            .parse()
            .map_err(|error| format!("invalid FRACTALD_RESOLVED_LISTEN: {error}"))?;
        let discovery = env::var_os("FRACTALD_RESOLVED_DISCOVERY")
            .map(PathBuf::from)
            .unwrap_or_else(default_discovery_path);
        let control = env::var_os("FRACTALD_RESOLVED_CONTROL")
            .map(PathBuf::from)
            .unwrap_or_else(|| default_control_path(&discovery));
        let upstreams = if let Ok(value) = env::var("FRACTALD_RESOLVED_UPSTREAM") {
            parse_upstreams(&value)?
        } else {
            read_resolv_conf(
                &env::var("FRACTALD_RESOLV_CONF").unwrap_or_else(|_| "/etc/resolv.conf".to_owned()),
            )?
        };
        if upstreams.is_empty() {
            return Err("no DNS upstreams are configured".to_owned());
        }
        Ok(Self {
            listen,
            upstreams,
            timeout: Duration::from_millis(
                env::var("FRACTALD_RESOLVED_TIMEOUT_MS")
                    .ok()
                    .map(|value| value.parse::<u64>())
                    .transpose()
                    .map_err(|error| format!("invalid timeout: {error}"))?
                    .unwrap_or(1_500),
            ),
            max_cache_entries: env::var("FRACTALD_RESOLVED_CACHE_ENTRIES")
                .ok()
                .map(|value| value.parse::<usize>())
                .transpose()
                .map_err(|error| format!("invalid cache size: {error}"))?
                .unwrap_or(512)
                .max(1),
            fault: match env::var("FRACTALD_RESOLVED_FAULT") {
                Ok(value) => parse_fault(&value)?,
                Err(env::VarError::NotPresent) => ResolverFault::None,
                Err(error) => return Err(format!("cannot read resolver fault setting: {error}")),
            },
            discovery,
            control,
        })
    }
}

fn serve(config: ResolverConfig) -> Result<(), String> {
    let socket = UdpSocket::bind(config.listen)
        .map_err(|error| format!("cannot bind {}: {error}", config.listen))?;
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(|error| format!("cannot configure resolver listener timeout: {error}"))?;
    let listen = socket
        .local_addr()
        .map_err(|error| format!("cannot determine resolver listen address: {error}"))?;
    let control = bind_control(&config.control)?;
    let _control_lease = ControlLease(config.control.clone());
    let resolver = Arc::new(Mutex::new(Resolver::with_fault(
        config.upstreams,
        config.timeout,
        config.max_cache_entries,
        config.fault,
    )));
    let record = DiscoveryRecord::new(
        std::process::id(),
        listen,
        config.control.clone(),
        resolver
            .lock()
            .map_err(|_| "resolver state lock is poisoned".to_owned())?
            .upstreams
            .clone(),
        config.max_cache_entries,
    );
    let _discovery = DiscoveryLease::publish(&config.discovery, &record)
        .map_err(|error| format!("cannot publish resolver discovery: {error}"))?;
    let control_resolver = Arc::clone(&resolver);
    std::thread::Builder::new()
        .name("fractald-resolved-control".to_owned())
        .spawn(move || control_loop(control, control_resolver, record))
        .map_err(|error| format!("cannot start resolver control thread: {error}"))?;
    let mut request = [0_u8; 4_096];
    loop {
        let (length, client) = match socket.recv_from(&mut request) {
            Ok(value) => value,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if fractald_platform::shutdown_requested() {
                    return Ok(());
                }
                continue;
            }
            Err(_error) if fractald_platform::shutdown_requested() => return Ok(()),
            Err(error) => return Err(format!("cannot receive DNS request: {error}")),
        };
        let response = resolver
            .lock()
            .map_err(|_| "resolver state lock is poisoned".to_owned())?
            .resolve(&request[..length]);
        if !response.is_empty() {
            socket
                .send_to(&response, client)
                .map_err(|error| format!("cannot send DNS response: {error}"))?;
        }
    }
}

struct ControlLease(PathBuf);

impl Drop for ControlLease {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn bind_control(path: &PathBuf) -> Result<UnixListener, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create resolver control directory: {error}"))?;
    }
    if path.exists() {
        match UnixStream::connect(path) {
            Ok(_) => {
                return Err(format!(
                    "resolver control socket is already in use: {}",
                    path.display()
                ));
            }
            Err(_) => fs::remove_file(path)
                .map_err(|error| format!("cannot remove stale resolver control socket: {error}"))?,
        }
    }
    let listener = UnixListener::bind(path).map_err(|error| {
        format!(
            "cannot bind resolver control socket {}: {error}",
            path.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("cannot secure resolver control socket: {error}"))?;
    }
    Ok(listener)
}

fn control_loop(
    listener: UnixListener,
    resolver: Arc<Mutex<Resolver>>,
    mut record: DiscoveryRecord,
) {
    for stream in listener.incoming() {
        let Ok(mut stream) = stream else {
            continue;
        };
        let mut request = String::new();
        if stream.read_to_string(&mut request).is_err() {
            continue;
        }
        let request = request.trim();
        let response = match request {
            "PING" => "OK\n".to_owned(),
            "STATUS" => {
                let Ok(resolver) = resolver.lock() else {
                    continue;
                };
                record.cache_entries = resolver.cache.len();
                record.cache_limit = resolver.max_cache_entries;
                record.upstreams = resolver.upstreams.clone();
                format_status(&record)
            }
            "STATISTICS" => {
                let Ok(resolver) = resolver.lock() else {
                    continue;
                };
                format!(
                    "cache_entries={}\ncache_limit={}\nupstreams={}\n",
                    resolver.cache.len(),
                    resolver.max_cache_entries,
                    resolver.upstreams.len()
                )
            }
            "FLUSH-CACHES" => {
                let Ok(mut resolver) = resolver.lock() else {
                    continue;
                };
                resolver.cache.clear();
                record.cache_entries = 0;
                "OK\n".to_owned()
            }
            value if value.starts_with("SET-UPSTREAMS ") => {
                let parsed = value
                    .trim_start_matches("SET-UPSTREAMS ")
                    .split_whitespace()
                    .map(|address| {
                        address
                            .parse::<SocketAddr>()
                            .map_err(|error| format!("invalid upstream {address}: {error}"))
                    })
                    .collect::<Result<Vec<_>, _>>();
                match parsed {
                    Ok(upstreams) if !upstreams.is_empty() => {
                        let Ok(mut resolver) = resolver.lock() else {
                            continue;
                        };
                        resolver.upstreams = upstreams.clone();
                        resolver.cache.clear();
                        record.upstreams = upstreams;
                        "OK\n".to_owned()
                    }
                    Ok(_) => "ERROR=no-upstreams\n".to_owned(),
                    Err(error) => format!("ERROR={error}\n"),
                }
            }
            _ => "ERROR=unknown-request\n".to_owned(),
        };
        let _ = stream.write_all(response.as_bytes());
    }
}

fn format_status(record: &DiscoveryRecord) -> String {
    let mut output = format!(
        "version={}\npid={}\nlisten={}\ncontrol={}\ncache_entries={}\ncache_limit={}\n",
        record.version,
        record.pid,
        record.listen,
        record.control.display(),
        record.cache_entries,
        record.cache_limit
    );
    for upstream in &record.upstreams {
        output.push_str(&format!("upstream={}\n", upstream));
    }
    output
}

struct Resolver {
    upstreams: Vec<std::net::SocketAddr>,
    timeout: Duration,
    max_cache_entries: usize,
    cache: Vec<CacheEntry>,
    fault: ResolverFault,
}

struct CacheEntry {
    key: Vec<u8>,
    response: Vec<u8>,
    expires: std::time::Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolverFault {
    None,
    Drop,
    Servfail,
    Delay(Duration),
}

impl Resolver {
    fn with_fault(
        upstreams: Vec<std::net::SocketAddr>,
        timeout: Duration,
        max_cache_entries: usize,
        fault: ResolverFault,
    ) -> Self {
        Self {
            upstreams,
            timeout,
            max_cache_entries,
            cache: Vec::new(),
            fault,
        }
    }

    fn resolve(&mut self, query: &[u8]) -> Vec<u8> {
        let Ok(question_end) = validate_query(query) else {
            return error_response(query, 1);
        };
        match self.fault {
            ResolverFault::None => {}
            ResolverFault::Drop => return Vec::new(),
            ResolverFault::Servfail => return error_response(query, 2),
            ResolverFault::Delay(delay) => std::thread::sleep(delay),
        }
        let key = query.get(2..).unwrap_or_default().to_vec();
        let now = std::time::Instant::now();
        self.cache.retain(|entry| entry.expires > now);
        if let Some(entry) = self.cache.iter().find(|entry| entry.key == key) {
            return with_id(&entry.response, query);
        }

        for upstream in &self.upstreams {
            let Ok(forwarder) = UdpSocket::bind("0.0.0.0:0") else {
                continue;
            };
            if forwarder.set_read_timeout(Some(self.timeout)).is_err()
                || forwarder.send_to(query, upstream).is_err()
            {
                continue;
            }
            let mut response = [0_u8; 4_096];
            let Ok((length, source)) = forwarder.recv_from(&mut response) else {
                continue;
            };
            if source != *upstream || response[..length].get(0..2) != query.get(0..2) {
                continue;
            }
            let response = response[..length].to_vec();
            if let Some(ttl) = response_ttl(&response, question_end) {
                if ttl > Duration::ZERO {
                    self.cache.push(CacheEntry {
                        key,
                        response: response.clone(),
                        expires: now + ttl.min(Duration::from_secs(300)),
                    });
                    while self.cache.len() > self.max_cache_entries {
                        self.cache.remove(0);
                    }
                }
            }
            return response;
        }
        error_response(query, 2)
    }
}

fn parse_fault(value: &str) -> Result<ResolverFault, String> {
    match value.trim() {
        "" | "none" => Ok(ResolverFault::None),
        "drop" => Ok(ResolverFault::Drop),
        "servfail" => Ok(ResolverFault::Servfail),
        value => value
            .strip_prefix("delay:")
            .ok_or_else(|| {
                format!(
                    "invalid FRACTALD_RESOLVED_FAULT={value}; expected none, drop, servfail, or delay:MILLISECONDS"
                )
            })?
            .parse::<u64>()
            .map(|milliseconds| ResolverFault::Delay(Duration::from_millis(milliseconds)))
            .map_err(|error| format!("invalid resolver fault delay: {error}")),
    }
}

fn parse_upstreams(value: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let address = if value.parse::<std::net::SocketAddr>().is_ok() {
                value.to_owned()
            } else if value.contains(':') && !value.starts_with('[') {
                format!("[{value}]:53")
            } else {
                format!("{value}:53")
            };
            address
                .parse()
                .map_err(|error| format!("invalid upstream {value}: {error}"))
        })
        .collect()
}

fn read_resolv_conf(path: &str) -> Result<Vec<std::net::SocketAddr>, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|error| format!("cannot read resolver configuration {path}: {error}"))?;
    let mut values = Vec::new();
    for line in source.lines() {
        let mut fields = line.split_whitespace();
        if fields.next() == Some("nameserver") {
            if let Some(address) = fields.next() {
                values.extend(parse_upstreams(address)?);
            }
        }
    }
    Ok(values)
}

fn validate_query(query: &[u8]) -> Result<usize, ()> {
    if query.len() < 12 || u16::from_be_bytes([query[4], query[5]]) != 1 {
        return Err(());
    }
    let question_end = skip_name(query, 12)?;
    if question_end + 4 > query.len() {
        return Err(());
    }
    Ok(question_end + 4)
}

fn skip_name(packet: &[u8], mut offset: usize) -> Result<usize, ()> {
    let mut jumps = 0;
    loop {
        let length = *packet.get(offset).ok_or(())?;
        if length & 0xc0 == 0xc0 {
            if packet.get(offset + 1).is_none() || jumps > 16 {
                return Err(());
            }
            return Ok(offset + 2);
        }
        if length & 0xc0 != 0 || length > 63 {
            return Err(());
        }
        offset += 1;
        if length == 0 {
            return Ok(offset);
        }
        offset += usize::from(length);
        if offset > packet.len() {
            return Err(());
        }
        jumps += 1;
    }
}

fn response_ttl(response: &[u8], question_end: usize) -> Option<Duration> {
    if response.len() < 12 || question_end > response.len() {
        return None;
    }
    let answer_count = usize::from(u16::from_be_bytes([response[6], response[7]]));
    let authority_count = usize::from(u16::from_be_bytes([response[8], response[9]]));
    let additional_count = usize::from(u16::from_be_bytes([response[10], response[11]]));
    let mut offset = question_end;
    let mut minimum = None;
    for count in [answer_count, authority_count, additional_count] {
        for _ in 0..count {
            offset = skip_name(response, offset).ok()?;
            if offset + 10 > response.len() {
                return None;
            }
            let ttl = u32::from_be_bytes([
                response[offset + 4],
                response[offset + 5],
                response[offset + 6],
                response[offset + 7],
            ]);
            let data_length = usize::from(u16::from_be_bytes([
                response[offset + 8],
                response[offset + 9],
            ]));
            minimum = Some(minimum.map_or(ttl, |current: u32| current.min(ttl)));
            offset += 10 + data_length;
            if offset > response.len() {
                return None;
            }
        }
    }
    minimum.map(|seconds| Duration::from_secs(u64::from(seconds)))
}

fn with_id(response: &[u8], query: &[u8]) -> Vec<u8> {
    let mut response = response.to_vec();
    if response.len() >= 2 && query.len() >= 2 {
        response[0] = query[0];
        response[1] = query[1];
    }
    response
}

fn error_response(query: &[u8], code: u8) -> Vec<u8> {
    if query.len() < 12 {
        return Vec::new();
    }
    let (valid_query, question_end) = match validate_query(query) {
        Ok(end) => (true, end),
        Err(()) => (false, 12),
    };
    let mut response = query[..question_end.min(query.len())].to_vec();
    response[2] = (response[2] | 0x80) & 0xfb;
    response[3] = (response[3] & 0xf0) | (code & 0x0f);
    response[4] = 0;
    response[5] = if valid_query { 1 } else { 0 };
    for byte in &mut response[6..12] {
        *byte = 0;
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::thread;

    fn query(id: u16) -> Vec<u8> {
        let packet = vec![
            (id >> 8) as u8,
            id as u8,
            1,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            7,
            b'e',
            b'x',
            b'a',
            b'm',
            b'p',
            b'l',
            b'e',
            3,
            b'c',
            b'o',
            b'm',
            0,
            0,
            1,
            0,
            1,
        ];
        packet
    }

    #[test]
    fn parses_upstream_addresses() {
        let values = parse_upstreams("1.1.1.1, [::1]:5353").expect("upstreams");
        assert_eq!(
            values[0],
            "1.1.1.1:53".parse::<SocketAddr>().expect("address")
        );
        assert_eq!(
            values[1],
            "[::1]:5353".parse::<SocketAddr>().expect("address")
        );
    }

    #[test]
    fn rejects_truncated_queries() {
        assert!(validate_query(&[0; 11]).is_err());
    }

    #[test]
    fn parses_resolver_fault_modes() {
        assert_eq!(parse_fault("none"), Ok(ResolverFault::None));
        assert_eq!(parse_fault("drop"), Ok(ResolverFault::Drop));
        assert_eq!(parse_fault("servfail"), Ok(ResolverFault::Servfail));
        assert_eq!(
            parse_fault("delay:25"),
            Ok(ResolverFault::Delay(Duration::from_millis(25)))
        );
        assert!(parse_fault("unknown").is_err());
    }

    #[test]
    fn resolver_faults_are_deterministic() {
        let request = query(0x1234);
        let mut drop =
            Resolver::with_fault(Vec::new(), Duration::from_secs(1), 8, ResolverFault::Drop);
        assert!(drop.resolve(&request).is_empty());

        let mut servfail = Resolver::with_fault(
            Vec::new(),
            Duration::from_secs(1),
            8,
            ResolverFault::Servfail,
        );
        let response = servfail.resolve(&request);
        assert_eq!(&response[..2], &request[..2]);
        assert_eq!(response[3] & 0x0f, 2);
    }

    #[test]
    fn forwards_and_caches_a_response() {
        let upstream = UdpSocket::bind("127.0.0.1:0").expect("upstream socket");
        let address = upstream.local_addr().expect("upstream address");
        let thread = thread::spawn(move || {
            let mut packet = [0_u8; 512];
            let (length, peer) = upstream.recv_from(&mut packet).expect("query");
            let mut response = packet[..length].to_vec();
            response[2] |= 0x80;
            response[6] = 0;
            response[7] = 1;
            response.extend_from_slice(&[
                0xc0, 0x0c, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x1e, 0x00, 0x04, 127, 0, 0,
                1,
            ]);
            upstream.send_to(&response, peer).expect("response");
        });
        let mut resolver = Resolver::with_fault(
            vec![address],
            Duration::from_secs(1),
            8,
            ResolverFault::None,
        );
        let request = query(0x1234);
        let response = resolver.resolve(&request);
        assert_eq!(&response[..2], &request[..2]);
        assert_eq!(resolver.cache.len(), 1);
        thread.join().expect("upstream thread");
    }
}
