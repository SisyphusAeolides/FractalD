use fractald_resolver::{
    DiscoveryRecord, build_query, default_discovery_path, parse_answers, parse_record_type,
    read_discovery, read_to_string, send_query,
};
use std::env;
use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("resolvectl: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1).peekable();
    let mut command = None;
    let mut values = Vec::new();
    let mut requested_type = None;
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--version" => {
                println!("resolvectl (FractalD) 0.1.0");
                return Ok(());
            }
            "--no-pager" | "--no-legend" | "--legend=no" | "--json" | "--json=short" => {}
            "--type" => {
                requested_type = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--type requires a record type".to_owned())?,
                )
            }
            value if value.starts_with("--type=") => {
                requested_type = Some(value.trim_start_matches("--type=").to_owned())
            }
            value if value.starts_with('-') => {
                return Err(format!("unsupported option {value}"));
            }
            value => {
                if command.is_none() {
                    command = Some(value.to_owned());
                } else {
                    values.push(value.to_owned());
                }
            }
        }
    }
    match command.as_deref().unwrap_or("status") {
        "status" => status(),
        "query" => query(&values, requested_type.as_deref()),
        "flush-caches" => control_command("FLUSH-CACHES", "Caches flushed."),
        "statistics" => statistics(),
        "reset-server-features" => control_command("PING", "Server features reset."),
        "dns" => dns(&values),
        "revert" => Err("per-link resolver configuration is not available".to_owned()),
        "domain" => Err("per-link resolver domains are not available".to_owned()),
        "monitor" => Err("resolver monitor streaming is not available".to_owned()),
        value => Err(format!("unknown command {value}")),
    }
}

fn status() -> Result<(), String> {
    let record = discover()?;
    let response = control_request(&record, "STATUS")?;
    let values = parse_lines(&response);
    println!("Global");
    println!("       Protocols: -LLMNR -mDNS -DNSOverTLS");
    println!("resolvers:");
    if let Some(listen) = values.get("listen") {
        println!("       Stub Listener: {listen}");
    }
    if let Some(upstreams) = values.get("upstream") {
        println!("       DNS Servers: {upstreams}");
    } else if !record.upstreams.is_empty() {
        println!(
            "       DNS Servers: {}",
            record
                .upstreams
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    if let Some(cache_entries) = values.get("cache_entries") {
        println!("       Cache Entries: {cache_entries}");
    }
    Ok(())
}

fn statistics() -> Result<(), String> {
    let record = discover()?;
    let response = control_request(&record, "STATISTICS")?;
    for (key, value) in response.lines().filter_map(split_line) {
        println!("{key}={value}");
    }
    Ok(())
}

fn query(values: &[String], requested_type: Option<&str>) -> Result<(), String> {
    if values.is_empty() {
        return Err("query requires a DNS name".to_owned());
    }
    let name = &values[0];
    let record_type = if let Some(value) = requested_type {
        parse_record_type(value)?
    } else if values.len() > 1 {
        parse_record_type(&values[1])?
    } else {
        1
    };
    let record = discover()?;
    let packet = build_query(name, record_type)?;
    let response = send_query(record.listen, &packet, timeout())
        .map_err(|error| format!("DNS query failed: {error}"))?;
    let answers = parse_answers(&response)?;
    if answers.is_empty() {
        return Err(format!("no records found for {name}"));
    }
    for answer in answers {
        println!(
            "{} {} IN {} {}",
            answer.name,
            answer.ttl,
            type_name(answer.record_type),
            answer.data
        );
    }
    Ok(())
}

fn dns(values: &[String]) -> Result<(), String> {
    if values.is_empty() {
        return status();
    }
    let mut servers = values
        .iter()
        .filter_map(|value| parse_server(value).ok())
        .collect::<Vec<_>>();
    if servers.is_empty() && values.len() > 1 {
        servers = values[1..]
            .iter()
            .map(|value| parse_server(value))
            .collect::<Result<Vec<_>, _>>()?;
    }
    if servers.is_empty() {
        return Err("dns requires one or more server addresses".to_owned());
    }
    let record = discover()?;
    let request = format!(
        "SET-UPSTREAMS {}",
        servers
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ")
    );
    let response = control_request(&record, &request)?;
    if response.trim() != "OK" {
        return Err(response.trim().to_owned());
    }
    println!("DNS servers updated.");
    Ok(())
}

fn control_command(command: &str, success: &str) -> Result<(), String> {
    let record = discover()?;
    let response = control_request(&record, command)?;
    if response.trim() != "OK" {
        return Err(response.trim().to_owned());
    }
    println!("{success}");
    Ok(())
}

fn discover() -> Result<DiscoveryRecord, String> {
    let path = default_discovery_path();
    read_discovery(&path).map_err(|error| {
        format!(
            "resolver service is not discoverable at {}: {error}",
            path.display()
        )
    })
}

fn control_request(record: &DiscoveryRecord, request: &str) -> Result<String, String> {
    let mut last_error = None;
    for _ in 0..5 {
        match UnixStream::connect(&record.control) {
            Ok(mut stream) => {
                if let Err(error) = stream
                    .write_all(request.as_bytes())
                    .and_then(|_| stream.shutdown(std::net::Shutdown::Write))
                {
                    last_error = Some(format!("cannot send resolver control request: {error}"));
                } else {
                    return read_to_string(&mut stream).map_err(|error| {
                        format!("cannot read resolver control response: {error}")
                    });
                }
            }
            Err(error) => {
                last_error = Some(format!(
                    "cannot connect to resolver control socket: {error}"
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err(last_error.unwrap_or_else(|| "resolver control request failed".to_owned()))
}

fn parse_lines(response: &str) -> std::collections::BTreeMap<String, String> {
    let mut output = std::collections::BTreeMap::new();
    for line in response.lines() {
        if let Some((key, value)) = split_line(line) {
            output
                .entry(key.to_owned())
                .and_modify(|current: &mut String| {
                    current.push(' ');
                    current.push_str(value);
                })
                .or_insert_with(|| value.to_owned());
        }
    }
    output
}

fn split_line(line: &str) -> Option<(&str, &str)> {
    line.split_once('=')
}

fn parse_server(value: &str) -> Result<SocketAddr, String> {
    if let Ok(address) = value.parse() {
        return Ok(address);
    }
    let address = if value.contains(':') && !value.starts_with('[') {
        format!("[{value}]:53")
    } else {
        format!("{value}:53")
    };
    address
        .parse()
        .map_err(|error| format!("invalid DNS server {value}: {error}"))
}

fn timeout() -> Duration {
    env::var("FRACTALD_RESOLVED_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(2))
}

fn type_name(record_type: u16) -> String {
    match record_type {
        1 => "A".to_owned(),
        2 => "NS".to_owned(),
        5 => "CNAME".to_owned(),
        12 => "PTR".to_owned(),
        16 => "TXT".to_owned(),
        28 => "AAAA".to_owned(),
        255 => "ANY".to_owned(),
        value => format!("TYPE{value}"),
    }
}

fn print_help() {
    println!(
        "resolvectl (FractalD)\n\nCommands:\n  status\n  query NAME [TYPE]\n  dns [SERVER ...]\n  statistics\n  flush-caches\n  reset-server-features\n  revert\n  domain\n  monitor\n\nThe client discovers fractald-resolved through its runtime endpoint record."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_addresses() {
        assert_eq!(
            parse_server("1.1.1.1").expect("IPv4"),
            "1.1.1.1:53".parse().unwrap()
        );
        assert_eq!(
            parse_server("[::1]:5353").expect("IPv6"),
            "[::1]:5353".parse().unwrap()
        );
    }

    #[test]
    fn combines_repeated_status_fields() {
        let parsed = parse_lines("upstream=1.1.1.1:53\nupstream=8.8.8.8:53\n");
        assert_eq!(
            parsed.get("upstream"),
            Some(&"1.1.1.1:53 8.8.8.8:53".to_owned())
        );
    }
}
