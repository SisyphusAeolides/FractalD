use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};

use fractald_journal::NativeJournal;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("systemd-cat: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut tag = env::var("FRACTALD_JOURNAL_UNIT").unwrap_or_else(|_| "systemd-cat".to_owned());
    let mut command = Vec::new();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "systemd-cat (FractalD)\n\nusage: systemd-cat [-t TAG] [COMMAND [ARGUMENT...]]"
                );
                return Ok(0);
            }
            "--version" => {
                println!("systemd-cat (FractalD) 0.1.0");
                return Ok(0);
            }
            "-t" | "--identifier" => {
                tag = arguments
                    .next()
                    .ok_or_else(|| format!("{argument} requires a tag"))?;
            }
            value if value.starts_with("--identifier=") => {
                tag = value.trim_start_matches("--identifier=").to_owned();
            }
            "-p" | "--priority" | "--level-prefix" => {
                let _ = arguments.next();
            }
            value if value.starts_with('-') => return Err(format!("unsupported option {value}")),
            value => {
                command.push(value.to_owned());
                command.extend(arguments);
                break;
            }
        }
    }

    let (stdout, stderr) = if command.is_empty() {
        let mut input = Vec::new();
        io::stdin()
            .read_to_end(&mut input)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        (input, Vec::new())
    } else {
        let output = Command::new(&command[0])
            .args(&command[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .map_err(|error| format!("cannot execute {}: {error}", command[0]))?;
        if !output.status.success() {
            let code = output.status.code().unwrap_or(1);
            write_log(&tag, &output.stdout, &output.stderr)?;
            return Ok(code as u8);
        }
        (output.stdout, output.stderr)
    };
    write_log(&tag, &stdout, &stderr)?;
    Ok(0)
}

fn write_log(tag: &str, stdout: &[u8], stderr: &[u8]) -> Result<(), String> {
    let directory = if let Some(path) = env::var_os("FRACTALD_LOG_DIR") {
        PathBuf::from(path)
    } else if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
        PathBuf::from(path).join("logs")
    } else if let Some(home) = env::var_os("HOME") {
        PathBuf::from(home).join(".local/state/fractald/logs")
    } else {
        PathBuf::from(format!(
            "/tmp/fractald-logs-{}",
            fractald_platform::effective_uid()
        ))
    };
    fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create journal directory: {error}"))?;
    let safe_tag = tag
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '@' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = directory.join(format!("{safe_tag}.stdout.log"));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("cannot open journal stream: {error}"))?;
    file.write_all(stdout)
        .and_then(|_| file.write_all(stderr))
        .map_err(|error| format!("cannot write journal stream: {error}"))?;
    forward_native(tag, "stdout", stdout);
    forward_native(tag, "stderr", stderr);
    Ok(())
}

fn forward_native(tag: &str, stream: &str, contents: &[u8]) {
    let Some(native) = NativeJournal::connect() else {
        return;
    };
    let pid = std::process::id();
    let lines = contents.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if index + 1 == lines.len() && line.is_empty() {
            continue;
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if native.send(tag, stream, pid, line).is_err() {
            break;
        }
    }
}
