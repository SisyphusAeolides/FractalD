use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("journalctl: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Default)]
struct Options {
    units: Vec<String>,
    lines: Option<usize>,
    follow: bool,
    list_boots: bool,
    disk_usage: bool,
    rotate: bool,
    grep: Option<String>,
    exit: bool,
}

fn run() -> Result<(), String> {
    let options = parse_args()?;
    if options.exit {
        return Ok(());
    }
    if options.list_boots {
        println!("-0 current");
        return Ok(());
    }
    let directory = log_directory();
    if options.disk_usage {
        return print_disk_usage(&directory);
    }
    if options.rotate {
        return Ok(());
    }
    if options.follow {
        return follow(&directory, &options);
    }
    let mut lines = read_lines(&directory, &options.units)?;
    if let Some(pattern) = &options.grep {
        lines.retain(|line| line.contains(pattern));
    }
    if let Some(count) = options.lines {
        let first = lines.len().saturating_sub(count);
        lines = lines.split_off(first);
    }
    for line in lines {
        println!("{line}");
    }
    Ok(())
}

fn parse_args() -> Result<Options, String> {
    let mut options = Options::default();
    let mut arguments = env::args().skip(1).peekable();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                options.exit = true;
                return Ok(options);
            }
            "--version" => {
                println!("journalctl (FractalD) 0.1.0");
                options.exit = true;
                return Ok(options);
            }
            "-f" | "--follow" => options.follow = true,
            "--list-boots" => options.list_boots = true,
            "--disk-usage" => options.disk_usage = true,
            "--rotate" => options.rotate = true,
            "--no-pager" | "--no-hostname" | "--utc" | "--quiet" | "--all" | "--catalog" => {}
            "-u" | "--unit" => options.units.push(
                arguments
                    .next()
                    .ok_or_else(|| format!("{argument} requires a unit name"))?,
            ),
            value if value.starts_with("--unit=") => {
                options
                    .units
                    .push(value.trim_start_matches("--unit=").to_owned());
            }
            "-n" | "--lines" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| format!("{argument} requires a count"))?;
                options.lines = Some(parse_lines_count(&value)?);
            }
            value if value.starts_with("-n") && value.len() > 2 => {
                options.lines = Some(parse_lines_count(&value[2..])?);
            }
            value if value.starts_with("--lines=") => {
                options.lines = Some(parse_lines_count(value.trim_start_matches("--lines="))?);
            }
            "--grep" => {
                options.grep = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--grep requires a pattern".to_owned())?,
                );
            }
            value if value.starts_with("--grep=") => {
                options.grep = Some(value.trim_start_matches("--grep=").to_owned());
            }
            "-b" | "--boot" | "--since" | "--until" | "--identifier" | "--priority"
            | "--facility" | "--output" | "-o" => {
                if !matches!(argument.as_str(), "-b" | "--boot") {
                    let _ = arguments.next();
                }
            }
            value
                if value.starts_with("--since=")
                    || value.starts_with("--until=")
                    || value.starts_with("--output=")
                    || value.starts_with("--identifier=")
                    || value.starts_with("--priority=")
                    || value.starts_with("--facility=") => {}
            value if value.starts_with('-') => {
                return Err(format!("unsupported option {value}"));
            }
            value => options.units.push(value.to_owned()),
        }
    }
    Ok(options)
}

fn parse_lines_count(value: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|error| format!("invalid line count {value}: {error}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct JournalFile {
    path: PathBuf,
    unit: String,
    generation: u8,
    stream: String,
}

fn journal_files(directory: &Path, units: &[String]) -> io::Result<Vec<JournalFile>> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(files),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let (base, generation) = if let Some(base) = name.strip_suffix(".1") {
            (base, 0)
        } else {
            (name, 1)
        };
        let (unit, stream) = if let Some(unit) = base.strip_suffix(".stdout.log") {
            (unit, "stdout")
        } else if let Some(unit) = base.strip_suffix(".stderr.log") {
            (unit, "stderr")
        } else {
            continue;
        };
        if !units.is_empty() && !units.iter().any(|value| value == unit) {
            continue;
        }
        files.push(JournalFile {
            path: entry.path(),
            unit: unit.to_owned(),
            generation,
            stream: stream.to_owned(),
        });
    }
    files.sort_by(|left, right| {
        left.unit
            .cmp(&right.unit)
            .then(left.generation.cmp(&right.generation))
            .then(left.stream.cmp(&right.stream))
    });
    Ok(files)
}

fn read_lines(directory: &Path, units: &[String]) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    for file in journal_files(directory, units).map_err(|error| {
        format!(
            "cannot inspect journal directory {}: {error}",
            directory.display()
        )
    })? {
        let contents = fs::read_to_string(&file.path)
            .map_err(|error| format!("cannot read {}: {error}", file.path.display()))?;
        for line in contents.lines() {
            lines.push(format!("{}[{}]: {line}", file.unit, file.stream));
        }
    }
    Ok(lines)
}

fn follow(directory: &Path, options: &Options) -> Result<(), String> {
    let mut offsets = BTreeMap::<PathBuf, u64>::new();
    loop {
        let files = journal_files(directory, &options.units)
            .map_err(|error| format!("cannot inspect journal directory: {error}"))?;
        for file in files {
            let contents = fs::read_to_string(&file.path)
                .map_err(|error| format!("cannot read {}: {error}", file.path.display()))?;
            let offset = offsets.entry(file.path.clone()).or_default();
            let bytes = contents.as_bytes();
            if (*offset as usize) < bytes.len() {
                let update = String::from_utf8_lossy(&bytes[*offset as usize..]);
                for line in update.lines() {
                    if options
                        .grep
                        .as_ref()
                        .is_none_or(|pattern| line.contains(pattern))
                    {
                        println!("{}[{}]: {line}", file.unit, file.stream);
                    }
                }
                *offset = bytes.len() as u64;
                io::stdout()
                    .flush()
                    .map_err(|error| format!("cannot flush journal output: {error}"))?;
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn print_disk_usage(directory: &Path) -> Result<(), String> {
    let bytes = journal_files(directory, &[])
        .map_err(|error| format!("cannot inspect journal directory: {error}"))?
        .into_iter()
        .filter_map(|file| fs::metadata(file.path).ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    println!("Archived and active journals take {bytes} bytes in the file system.");
    Ok(())
}

fn log_directory() -> PathBuf {
    if let Some(path) = env::var_os("FRACTALD_LOG_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
        return PathBuf::from(path).join("logs");
    }
    if fractald_platform::is_root() {
        PathBuf::from("/var/log/fractald")
    } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
        PathBuf::from(path).join("fractald/logs")
    } else if let Some(home) = env::var_os("HOME") {
        PathBuf::from(home).join(".local/state/fractald/logs")
    } else {
        PathBuf::from(format!(
            "/tmp/fractald-logs-{}",
            fractald_platform::effective_uid()
        ))
    }
}

fn print_help() {
    println!(
        "journalctl (FractalD)\n\nOptions:\n  -u, --unit UNIT       filter a service unit\n  -n, --lines N         show the last N lines\n  -f, --follow          follow new output\n      --disk-usage      show local journal size\n      --list-boots       list the current manager boot\n      --rotate           complete a local rotation request\n      --grep PATTERN     filter output\n      --no-pager         write directly to stdout"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn discovers_current_and_rotated_streams() {
        let root =
            std::env::temp_dir().join(format!("fractald-journal-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("journal directory");
        fs::write(root.join("demo.service.stdout.log.1"), "old\n").expect("old log");
        fs::write(root.join("demo.service.stdout.log"), "new\n").expect("new log");
        let files = journal_files(&root, &[]).expect("journal files");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].generation, 0);
        assert_eq!(files[1].generation, 1);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn tails_and_filters_lines() {
        let root =
            std::env::temp_dir().join(format!("fractald-journal-lines-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("journal directory");
        fs::write(
            root.join("demo.service.stdout.log"),
            "first\nsecond\nthird\n",
        )
        .expect("log");
        let mut lines = read_lines(&root, &["demo.service".to_owned()]).expect("lines");
        lines.retain(|line| line.contains("second") || line.contains("third"));
        assert_eq!(lines.len(), 2);
        fs::remove_dir_all(root).expect("cleanup");
    }
}
