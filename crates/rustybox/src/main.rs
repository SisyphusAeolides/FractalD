use std::collections::{BTreeMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use fractald_chaos::{
    Duffing, DuffingState, LogisticMap, Lorenz, Lyapunov, Mandelbrot, Rossler, SystemKind, Vec3,
};

mod sys;

const APPLETS: &[&str] = &[
    "cat",
    "chaos",
    "chroot",
    "dmesg",
    "echo",
    "env",
    "false",
    "findmnt",
    "init",
    "insmod",
    "kill",
    "ln",
    "ls",
    "mkdir",
    "modprobe",
    "mount",
    "mv",
    "pwd",
    "readlink",
    "rm",
    "rmdir",
    "sleep",
    "sort",
    "swapoff",
    "swapon",
    "switch_root",
    "sync",
    "tr",
    "true",
    "umount",
    "uname",
    "which",
];

const MS_RDONLY: u64 = 1;
const MS_NOSUID: u64 = 2;
const MS_NODEV: u64 = 4;
const MS_NOEXEC: u64 = 8;
const MS_SYNCHRONOUS: u64 = 16;
const MS_REMOUNT: u64 = 32;
const MS_MANDLOCK: u64 = 64;
const MS_DIRSYNC: u64 = 128;
const MS_NOATIME: u64 = 1024;
const MS_NODIRATIME: u64 = 2048;
const MS_BIND: u64 = 4096;
const MS_REC: u64 = 16384;
const MS_SILENT: u64 = 32768;
const MS_UNBINDABLE: u64 = 1 << 17;
const MS_PRIVATE: u64 = 1 << 18;
const MS_SLAVE: u64 = 1 << 19;
const MS_SHARED: u64 = 1 << 20;
const MS_RELATIME: u64 = 1 << 21;
const MS_STRICTATIME: u64 = 1 << 24;
const MS_LAZYTIME: u64 = 1 << 25;

const MNT_FORCE: i32 = 1;
const MNT_DETACH: i32 = 2;
const MNT_EXPIRE: i32 = 4;
const SWAP_FLAG_PREFER: i32 = 0x8000;
const SWAP_FLAG_PRIO_MASK: i32 = 0x7fff;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("rustybox: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut arguments = env::args_os();
    let invoked_as = arguments
        .next()
        .unwrap_or_else(|| OsString::from("rustybox"));
    let invoked_name = Path::new(&invoked_as)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("rustybox");
    let mut args = arguments.collect::<Vec<_>>();

    let applet = if matches!(invoked_name, "rustybox" | "rustybox.static") {
        args.first()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| "help".to_owned())
    } else {
        invoked_name.to_owned()
    };
    if matches!(applet.as_str(), "--list" | "list") {
        print_applets();
        return Ok(0);
    }
    if matches!(applet.as_str(), "--help" | "help") {
        print_help();
        return Ok(0);
    }
    if matches!(invoked_name, "rustybox" | "rustybox.static") {
        args.remove(0);
    }
    dispatch(&applet, &args)
}

fn dispatch(applet: &str, args: &[OsString]) -> Result<u8, String> {
    match applet {
        "cat" => run_cat(args),
        "chaos" => run_chaos(args),
        "chroot" => run_chroot(args),
        "dmesg" => run_dmesg(args),
        "echo" => run_echo(args),
        "env" => run_env(args),
        "false" => Ok(1),
        "findmnt" => run_findmnt(args),
        "init" => run_init(args),
        "insmod" => run_insmod(args),
        "kill" => run_kill(args),
        "ln" => run_ln(args),
        "ls" => run_ls(args),
        "mkdir" => run_mkdir(args),
        "modprobe" => run_modprobe(args),
        "mount" => run_mount(args),
        "mv" => run_mv(args),
        "pwd" => run_pwd(args),
        "readlink" => run_readlink(args),
        "rm" => run_rm(args),
        "rmdir" => run_rmdir(args),
        "sleep" => run_sleep(args),
        "sort" => run_sort(args),
        "swapon" => run_swapon(args),
        "swapoff" => run_swapoff(args),
        "switch_root" => run_switch_root(args),
        "sync" => run_sync(args),
        "tr" => run_tr(args),
        "true" => Ok(0),
        "umount" => run_umount(args),
        "uname" => run_uname(args),
        "which" => run_which(args),
        other => Err(format!("unknown applet {other}; use rustybox --list")),
    }
}

fn print_applets() {
    for applet in APPLETS {
        println!("{applet}");
    }
}

fn print_help() {
    println!(
        "RustyBox multi-call userland\n\nUsage:\n  rustybox <applet> [arguments...]\n  <applet> [arguments...]  when invoked through an applet link\n\nApplets:"
    );
    print_applets();
}

fn run_cat(args: &[OsString]) -> Result<u8, String> {
    if args.is_empty() {
        sys::copy_fd(0, 1).map_err(|error| format!("cannot read stdin: {error}"))?;
        return Ok(0);
    }
    for argument in args {
        if argument == "-" {
            sys::copy_fd(0, 1).map_err(|error| format!("cannot read stdin: {error}"))?;
        } else {
            let file = File::open(argument)
                .map_err(|error| format!("cannot open {}: {error}", display_os(argument)))?;
            sys::copy_fd(file.as_raw_fd(), 1)
                .map_err(|error| format!("cannot read {}: {error}", display_os(argument)))?;
        }
    }
    Ok(0)
}

fn run_dmesg(args: &[OsString]) -> Result<u8, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("Usage: dmesg");
        return Ok(0);
    }
    if !args.is_empty() {
        return Err("dmesg: this profile only reads the kernel log".to_owned());
    }

    // /dev/kmsg is a stream. Nonblocking reads let the applet drain the
    // records currently available without turning a fatal initramfs report
    // into an unbounded wait.
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(0x800)
        .open("/dev/kmsg")
        .map_err(|error| format!("dmesg: cannot open /dev/kmsg: {error}"))?;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => sys::write_all(1, &buffer[..length])
                .map_err(|error| format!("dmesg: cannot write output: {error}"))?,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("dmesg: cannot read /dev/kmsg: {error}")),
        }
    }
    Ok(0)
}

fn run_findmnt(args: &[OsString]) -> Result<u8, String> {
    let mut mountpoint = None;
    let mut fields = vec!["TARGET"];
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            operands.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-M"
            || argument == "--mountpoint"
            || argument == "-T"
            || argument == "--target"
        {
            index += 1;
            mountpoint = Some(
                args.get(index)
                    .ok_or_else(|| "findmnt: mountpoint is missing".to_owned())?
                    .clone(),
            );
        } else if let Some(value) = strip_os_prefix(argument, b"--mountpoint=")
            .or_else(|| strip_os_prefix(argument, b"--target="))
        {
            mountpoint = Some(value);
        } else if argument == "-n" || argument == "--noheadings" || argument == "--raw" {
        } else if argument == "-o" || argument == "--output" {
            index += 1;
            fields = parse_findmnt_fields(
                args.get(index)
                    .ok_or_else(|| "findmnt: output list is missing")?,
            )?;
        } else if let Some(value) = strip_os_prefix(argument, b"--output=") {
            fields = parse_findmnt_fields(&value)?;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "findmnt: unsupported option {}",
                display_os(argument)
            ));
        } else {
            operands.push(argument.clone());
        }
        index += 1;
    }

    let Some(mountpoint) = mountpoint.or_else(|| operands.first().cloned()) else {
        return run_cat(&[OsString::from("/proc/self/mounts")]);
    };
    let requested = normalize_mountpoint(Path::new(&mountpoint));
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("findmnt: cannot read /proc/self/mountinfo: {error}"))?;
    for line in mountinfo.lines() {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let Some(separator) = parts.iter().position(|part| *part == "-") else {
            continue;
        };
        if parts.len() <= separator + 2 || parts.len() < 6 {
            continue;
        }
        let candidate = decode_mountinfo_field(parts[4]);
        if normalize_mountpoint(Path::new(&candidate)) != requested {
            continue;
        }
        let filesystem = parts[separator + 1];
        let source = parts[separator + 2];
        let output = fields
            .iter()
            .map(|field| match *field {
                "TARGET" => candidate.as_str(),
                "FSTYPE" => filesystem,
                "SOURCE" => source,
                "OPTIONS" => parts[5],
                other => other,
            })
            .collect::<Vec<_>>()
            .join(" ");
        write_line(&output)?;
        return Ok(0);
    }
    Ok(1)
}

fn parse_findmnt_fields(value: &OsStr) -> Result<Vec<&'static str>, String> {
    let mut fields = Vec::new();
    for field in value.to_string_lossy().split(',') {
        let field = field.trim().to_ascii_uppercase();
        let field = match field.as_str() {
            "TARGET" | "FSTYPE" | "SOURCE" | "OPTIONS" => match field.as_str() {
                "TARGET" => "TARGET",
                "FSTYPE" => "FSTYPE",
                "SOURCE" => "SOURCE",
                "OPTIONS" => "OPTIONS",
                _ => unreachable!(),
            },
            other => return Err(format!("findmnt: unsupported output field {other}")),
        };
        fields.push(field);
    }
    if fields.is_empty() {
        return Err("findmnt: output list is empty".to_owned());
    }
    Ok(fields)
}

fn normalize_mountpoint(path: &Path) -> String {
    let value = path.to_string_lossy();
    if value == "/" {
        return "/".to_owned();
    }
    value.trim_end_matches('/').to_owned()
}

fn decode_mountinfo_field(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                let number = (digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + digits[2] - b'0';
                decoded.push(number);
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn run_echo(args: &[OsString]) -> Result<u8, String> {
    let mut newline = true;
    let mut index = 0;
    while args.get(index).is_some_and(|argument| argument == "-n") {
        newline = false;
        index += 1;
    }
    let mut output = args[index..]
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    if newline {
        output.push('\n');
    }
    sys::write_all(1, output.as_bytes())
        .map_err(|error| format!("cannot write stdout: {error}"))?;
    Ok(0)
}

fn run_env(args: &[OsString]) -> Result<u8, String> {
    let mut clear = false;
    let mut unsets = Vec::new();
    let mut assignments = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            index += 1;
            break;
        }
        if argument == "-i" || argument == "--ignore-environment" {
            clear = true;
            index += 1;
            continue;
        }
        if argument == "-u" || argument == "--unset" {
            let name = args
                .get(index + 1)
                .ok_or_else(|| "env: -u requires a variable name".to_owned())?;
            unsets.push(name.clone());
            index += 2;
            continue;
        }
        if let Some(name) = strip_os_prefix(argument, b"--unset=") {
            unsets.push(name.to_owned());
            index += 1;
            continue;
        }
        if let Some((name, value)) = split_assignment(argument) {
            assignments.push((name, value));
            index += 1;
            continue;
        }
        break;
    }

    if index == args.len() {
        let mut values = if clear {
            BTreeMap::new()
        } else {
            env::vars_os().collect::<BTreeMap<_, _>>()
        };
        for name in unsets {
            values.remove(&name);
        }
        for (name, value) in assignments {
            values.insert(name, value);
        }
        for (name, value) in values {
            let mut line = name;
            line.push("=");
            line.push(value);
            line.push("\n");
            sys::write_all(1, line.as_bytes())
                .map_err(|error| format!("cannot write env: {error}"))?;
        }
        return Ok(0);
    }

    let mut command = Command::new(&args[index]);
    command.args(&args[index + 1..]);
    if clear {
        command.env_clear();
    }
    for name in unsets {
        command.env_remove(name);
    }
    for (name, value) in assignments {
        command.env(name, value);
    }
    let status = command
        .status()
        .map_err(|error| format!("cannot execute {}: {error}", display_os(&args[index])))?;
    Ok(exit_status_code(status))
}

fn run_pwd(args: &[OsString]) -> Result<u8, String> {
    if !args.is_empty() {
        return Err("pwd does not accept arguments in this profile".to_owned());
    }
    let path = env::current_dir()
        .map_err(|error| format!("cannot determine current directory: {error}"))?;
    write_line(&path.to_string_lossy())
}

fn run_mkdir(args: &[OsString]) -> Result<u8, String> {
    let mut parents = false;
    let mut mode = 0o777;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-p" || argument == "--parents" {
            parents = true;
        } else if argument == "-m" || argument == "--mode" {
            index += 1;
            let value = args
                .get(index)
                .ok_or_else(|| "mkdir: --mode requires a mode".to_owned())?;
            mode = parse_mode(&value)?;
        } else if let Some(value) = strip_os_prefix(argument, b"--mode=") {
            mode = parse_mode(&value)?;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "mkdir: unsupported option {}",
                display_os(argument)
            ));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.is_empty() {
        return Err("mkdir: missing operand".to_owned());
    }
    for path in paths {
        mkdir_path(Path::new(&path), mode, parents)
            .map_err(|error| format!("mkdir: {}: {error}", display_os(&path)))?;
    }
    Ok(0)
}

fn mkdir_path(path: &Path, mode: u32, parents: bool) -> io::Result<()> {
    if !parents {
        sys::mkdir_one(path, mode)?;
        return Ok(());
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() => continue,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "path component is not a directory",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                sys::mkdir_one(&current, mode)?
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn run_rm(args: &[OsString]) -> Result<u8, String> {
    let mut force = false;
    let mut recursive = false;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-f" || argument == "--force" {
            force = true;
        } else if argument == "-r" || argument == "-R" || argument == "--recursive" {
            recursive = true;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!("rm: unsupported option {}", display_os(argument)));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.is_empty() {
        return if force {
            Ok(0)
        } else {
            Err("rm: missing operand".to_owned())
        };
    }
    for path in paths {
        let path = PathBuf::from(&path);
        match remove_path(&path, recursive) {
            Ok(()) => {}
            Err(error) if force && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("rm: {}: {error}", path.display())),
        }
    }
    Ok(0)
}

fn remove_path(path: &Path, recursive: bool) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        if !recursive {
            return Err(io::Error::new(
                io::ErrorKind::IsADirectory,
                "is a directory",
            ));
        }
        for entry in fs::read_dir(path)? {
            remove_path(&entry?.path(), true)?;
        }
        sys::remove_directory(path)
    } else {
        sys::remove_file(path)
    }
}

fn run_rmdir(args: &[OsString]) -> Result<u8, String> {
    let mut parents = false;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-p" || argument == "--parents" {
            parents = true;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "rmdir: unsupported option {}",
                display_os(argument)
            ));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.is_empty() {
        return Err("rmdir: missing operand".to_owned());
    }
    for path in paths {
        let mut current = PathBuf::from(&path);
        loop {
            sys::remove_directory(&current)
                .map_err(|error| format!("rmdir: {}: {error}", current.display()))?;
            if !parents {
                break;
            }
            let Some(parent) = current.parent() else {
                break;
            };
            if parent.as_os_str().is_empty() || parent == Path::new("/") || parent == Path::new(".")
            {
                break;
            }
            current = parent.to_path_buf();
        }
    }
    Ok(0)
}

fn run_mv(args: &[OsString]) -> Result<u8, String> {
    let paths = positional_args(args, "mv")?;
    if paths.len() < 2 {
        return Err("mv: missing destination".to_owned());
    }
    let destination = PathBuf::from(paths.last().expect("destination exists"));
    let sources = &paths[..paths.len() - 1];
    if sources.len() > 1 && !fs::metadata(&destination).is_ok_and(|metadata| metadata.is_dir()) {
        return Err("mv: last operand must be a directory".to_owned());
    }
    for source in sources {
        let source_path = PathBuf::from(source);
        let target = if sources.len() > 1 || fs::metadata(&destination).is_ok_and(|m| m.is_dir()) {
            destination.join(
                source_path
                    .file_name()
                    .ok_or_else(|| format!("mv: invalid source {}", source_path.display()))?,
            )
        } else {
            destination.clone()
        };
        sys::rename(&source_path, &target).map_err(|error| {
            format!(
                "mv: {} -> {}: {error}",
                source_path.display(),
                target.display()
            )
        })?;
    }
    Ok(0)
}

fn run_ln(args: &[OsString]) -> Result<u8, String> {
    let mut symbolic = false;
    let mut force = false;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-s" || argument == "--symbolic" {
            symbolic = true;
        } else if argument == "-f" || argument == "--force" {
            force = true;
        } else if argument == "-T" || argument == "--no-target-directory" {
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!("ln: unsupported option {}", display_os(argument)));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.len() != 2 {
        return Err("ln: this profile accepts one source and one destination".to_owned());
    }
    let source = &paths[0];
    let mut destination = PathBuf::from(&paths[1]);
    if fs::metadata(&destination).is_ok_and(|metadata| metadata.is_dir()) {
        destination.push(
            Path::new(source)
                .file_name()
                .ok_or_else(|| format!("ln: invalid source {}", display_os(source)))?,
        );
    }
    if force {
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_dir() => sys::remove_directory(&destination),
            Ok(_) => sys::remove_file(&destination),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
        .map_err(|error| format!("ln: cannot remove {}: {error}", destination.display()))?;
    }
    sys::link(source, &destination, symbolic).map_err(|error| {
        format!(
            "ln: {} -> {}: {error}",
            display_os(source),
            destination.display()
        )
    })?;
    Ok(0)
}

fn run_readlink(args: &[OsString]) -> Result<u8, String> {
    let mut canonical = false;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "-f" || argument == "--canonicalize" {
            canonical = true;
        } else if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "readlink: unsupported option {}",
                display_os(argument)
            ));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.len() != 1 {
        return Err("readlink: this profile accepts one path".to_owned());
    }
    let path = PathBuf::from(&paths[0]);
    let target = if canonical {
        fs::canonicalize(&path)
    } else {
        fs::read_link(&path)
    }
    .map_err(|error| format!("readlink: {}: {error}", path.display()))?;
    write_line(&target.to_string_lossy())
}

fn run_sleep(args: &[OsString]) -> Result<u8, String> {
    if args.is_empty() {
        return Err("sleep: missing operand".to_owned());
    }
    let mut milliseconds = 0_u64;
    for argument in args {
        let value = parse_duration_milliseconds(argument)?;
        milliseconds = milliseconds
            .checked_add(value)
            .ok_or_else(|| "sleep: duration is too large".to_owned())?;
    }
    sys::sleep_milliseconds(milliseconds).map_err(|error| format!("sleep: {error}"))?;
    Ok(0)
}

fn run_sort(args: &[OsString]) -> Result<u8, String> {
    let mut reverse = false;
    let mut key_field = None;
    let mut files = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            files.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-r" || argument == "--reverse" {
            reverse = true;
        } else if argument == "-k" || argument == "--key" {
            index += 1;
            key_field = Some(parse_sort_key(
                args.get(index).ok_or_else(|| "sort: key is missing")?,
            )?);
        } else if let Some(value) = strip_os_prefix(argument, b"-k") {
            key_field = Some(parse_sort_key(&value)?);
        } else if let Some(value) = strip_os_prefix(argument, b"--key=") {
            key_field = Some(parse_sort_key(&value)?);
        } else if argument == "-u" || argument == "--unique" {
            return Err("sort: --unique is not supported by this profile".to_owned());
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!("sort: unsupported option {}", display_os(argument)));
        } else {
            files.push(argument.clone());
        }
        index += 1;
    }

    let mut input = String::new();
    if files.is_empty() {
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| format!("sort: cannot read stdin: {error}"))?;
    } else {
        for file in files {
            if file == "-" {
                io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|error| format!("sort: cannot read stdin: {error}"))?;
            } else {
                input.push_str(
                    &fs::read_to_string(&file)
                        .map_err(|error| format!("sort: {}: {error}", display_os(&file)))?,
                );
            }
        }
    }
    let mut lines = input.lines().map(str::to_owned).collect::<Vec<_>>();
    lines.sort_by(|left, right| {
        let left_key = sort_key(left, key_field);
        let right_key = sort_key(right, key_field);
        left_key.cmp(&right_key).then_with(|| left.cmp(right))
    });
    if reverse {
        lines.reverse();
    }
    for line in lines {
        write_line(&line)?;
    }
    Ok(0)
}

fn parse_sort_key(value: &OsStr) -> Result<usize, String> {
    let text = value.to_string_lossy();
    let field = text
        .split_once(',')
        .map_or(text.as_ref(), |(first, _)| first)
        .split('.')
        .next()
        .unwrap_or_default()
        .parse::<usize>()
        .map_err(|_| format!("sort: invalid key {text}"))?;
    if field == 0 {
        return Err(format!("sort: invalid key {text}"));
    }
    Ok(field)
}

fn sort_key(line: &str, field: Option<usize>) -> String {
    field
        .and_then(|field| line.split_whitespace().nth(field.saturating_sub(1)))
        .unwrap_or(line)
        .to_owned()
}

fn run_sync(args: &[OsString]) -> Result<u8, String> {
    if !args.is_empty() {
        return Err("sync: this profile accepts no arguments".to_owned());
    }
    sys::sync().map_err(|error| format!("sync: {error}"))?;
    Ok(0)
}

fn run_tr(args: &[OsString]) -> Result<u8, String> {
    let mut delete = false;
    let mut operands = Vec::new();
    for argument in args {
        if argument == "-d" || argument == "--delete" {
            delete = true;
        } else if argument == "--" {
            continue;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!("tr: unsupported option {}", display_os(argument)));
        } else {
            operands.push(argument);
        }
    }
    if operands.len() != if delete { 1 } else { 2 } {
        return Err(if delete {
            "tr: delete mode requires one character set".to_owned()
        } else {
            "tr: this profile requires SET1 and SET2".to_owned()
        });
    }
    let first = parse_tr_set(operands[0])?;
    let second = if delete {
        Vec::new()
    } else {
        parse_tr_set(operands[1])?
    };
    if !delete && second.is_empty() {
        return Err("tr: SET2 is empty".to_owned());
    }
    let mut input = Vec::new();
    io::stdin()
        .read_to_end(&mut input)
        .map_err(|error| format!("tr: cannot read stdin: {error}"))?;
    let mut output = Vec::with_capacity(input.len());
    for byte in input {
        if let Some(position) = first.iter().position(|candidate| *candidate == byte) {
            if delete {
                continue;
            }
            output.push(second[position.min(second.len() - 1)]);
        } else {
            output.push(byte);
        }
    }
    sys::write_all(1, &output).map_err(|error| format!("tr: cannot write stdout: {error}"))?;
    Ok(0)
}

fn parse_tr_set(value: &OsStr) -> Result<Vec<u8>, String> {
    let text = value.to_string_lossy();
    match text.as_ref() {
        "[:upper:]" => return Ok((b'A'..=b'Z').collect()),
        "[:lower:]" => return Ok((b'a'..=b'z').collect()),
        "[:digit:]" => return Ok((b'0'..=b'9').collect()),
        _ => {}
    }
    let bytes = text.as_bytes();
    let mut output = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            let (byte, consumed) = parse_tr_escape(&bytes[index + 1..])?;
            output.push(byte);
            index += consumed + 1;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    Ok(output)
}

fn parse_tr_escape(value: &[u8]) -> Result<(u8, usize), String> {
    let Some(first) = value.first().copied() else {
        return Err("tr: trailing escape".to_owned());
    };
    let named = match first {
        b'n' => Some(b'\n'),
        b'r' => Some(b'\r'),
        b't' => Some(b'\t'),
        b'\\' => Some(b'\\'),
        _ => None,
    };
    if let Some(byte) = named {
        return Ok((byte, 1));
    }
    if !(b'0'..=b'7').contains(&first) {
        return Ok((first, 1));
    }
    let mut number = 0_u16;
    let mut consumed = 0;
    while consumed < value.len() && consumed < 3 && (b'0'..=b'7').contains(&value[consumed]) {
        number = number * 8 + u16::from(value[consumed] - b'0');
        consumed += 1;
    }
    Ok((number.min(255) as u8, consumed))
}

fn run_ls(args: &[OsString]) -> Result<u8, String> {
    let mut all = false;
    let mut one_per_line = false;
    let mut paths = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            paths.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-a" || argument == "--all" {
            all = true;
        } else if argument == "-1" {
            one_per_line = true;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!("ls: unsupported option {}", display_os(argument)));
        } else {
            paths.push(argument.clone());
        }
        index += 1;
    }
    if paths.is_empty() {
        paths.push(OsString::from("."));
    }
    let path_count = paths.len();
    let mut first = true;
    for path in paths {
        let path = PathBuf::from(&path);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("ls: {}: {error}", path.display()))?;
        if !metadata.is_dir() {
            write_line(&path.to_string_lossy())?;
            continue;
        }
        if path_count > 1 {
            if !first {
                println!();
            }
            println!("{}:", path.display());
        }
        first = false;
        let mut entries = fs::read_dir(&path)
            .map_err(|error| format!("ls: {}: {error}", path.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("ls: {}: {error}", path.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        let names = entries
            .into_iter()
            .filter_map(|entry| {
                let name = entry.file_name();
                (all || !name.as_bytes().starts_with(b".")).then_some(name)
            })
            .collect::<Vec<_>>();
        if one_per_line || names.is_empty() {
            for name in names {
                write_line(&name.to_string_lossy())?;
            }
        } else {
            write_line(
                &names
                    .iter()
                    .map(|name| name.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("  "),
            )?;
        }
    }
    Ok(0)
}

fn run_kill(args: &[OsString]) -> Result<u8, String> {
    if args.is_empty() {
        return Err("kill: usage: kill [-SIGNAL|-s SIGNAL] PID...".to_owned());
    }
    let mut signal = 15;
    let mut index = 0;
    if args[0] == "-l" || args[0] == "--list" {
        println!("HUP INT QUIT KILL TERM STOP CONT USR1 USR2");
        return Ok(0);
    }
    if args[0] == "-s" || args[0] == "--signal" {
        signal = parse_signal(
            args.get(1)
                .ok_or_else(|| "kill: signal is missing".to_owned())?,
        )?;
        index = 2;
    } else if let Some(value) = strip_os_prefix(&args[0], b"--signal=") {
        signal = parse_signal(&value)?;
        index = 1;
    } else if args[0].as_bytes().starts_with(b"-") {
        let value = OsString::from_vec(args[0].as_bytes()[1..].to_vec());
        signal = parse_signal(&value)?;
        index = 1;
    }
    if index == args.len() {
        return Err("kill: no process ID specified".to_owned());
    }
    for argument in &args[index..] {
        let pid = argument
            .to_string_lossy()
            .parse::<i32>()
            .map_err(|_| format!("kill: invalid process ID {}", display_os(argument)))?;
        sys::send_signal(pid, signal).map_err(|error| format!("kill: {pid}: {error}"))?;
    }
    Ok(0)
}

fn run_uname(args: &[OsString]) -> Result<u8, String> {
    let mut fields = Vec::new();
    if args.is_empty() {
        fields.push(0);
    }
    for argument in args {
        if argument == "-a" || argument == "--all" {
            fields = vec![0, 1, 2, 3, 4];
            break;
        }
        let text = argument.to_string_lossy();
        if !text.starts_with('-') {
            return Err(format!("uname: unsupported operand {text}"));
        }
        for option in text[1..].chars() {
            let field = match option {
                's' => 0,
                'n' => 1,
                'r' => 2,
                'v' => 3,
                'm' => 4,
                _ => return Err(format!("uname: unsupported option -{option}")),
            };
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    let values = fields
        .into_iter()
        .map(sys::uname_field)
        .collect::<io::Result<Vec<_>>>()
        .map_err(|error| format!("uname: {error}"))?;
    write_line(&values.join(" "))
}

fn run_which(args: &[OsString]) -> Result<u8, String> {
    let mut all = false;
    let mut names = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "-a" || argument == "--all" {
            all = true;
        } else if argument == "--" {
            names.extend_from_slice(&args[index + 1..]);
            break;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "which: unsupported option {}",
                display_os(argument)
            ));
        } else {
            names.push(argument.clone());
        }
        index += 1;
    }
    if names.is_empty() {
        return Err("which: missing command name".to_owned());
    }
    let mut missing = false;
    for name in names {
        let matches = command_paths(&name);
        if matches.is_empty() {
            missing = true;
            continue;
        }
        let selected = if all {
            matches
        } else {
            matches.into_iter().take(1).collect()
        };
        for path in selected {
            write_line(&path.to_string_lossy())?;
        }
    }
    Ok(u8::from(missing))
}

fn run_mount(args: &[OsString]) -> Result<u8, String> {
    let mut filesystem = None;
    let mut option_values = Vec::new();
    let mut flags = 0;
    let mut read_only_argument = false;
    let mut bind_argument = false;
    let mut recursive_bind_argument = false;
    let mut sloppy_argument = false;
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            operands.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-r" || argument == "--read-only" {
            flags |= MS_RDONLY;
            read_only_argument = true;
        } else if argument == "-t" || argument == "--types" {
            index += 1;
            filesystem = Some(
                args.get(index)
                    .ok_or_else(|| "mount: -t requires a filesystem type".to_owned())?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if argument == "-o" || argument == "--options" {
            index += 1;
            option_values.push(
                args.get(index)
                    .ok_or_else(|| "mount: -o requires options".to_owned())?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if let Some(value) = strip_os_prefix(argument, b"-o") {
            if value.is_empty() {
                return Err("mount: -o requires options".to_owned());
            }
            option_values.push(value.to_string_lossy().into_owned());
        } else if argument == "--bind" {
            flags |= MS_BIND;
            bind_argument = true;
        } else if argument == "--rbind" {
            flags |= MS_BIND | MS_REC;
            recursive_bind_argument = true;
        } else if argument == "-s" || argument == "--sloppy" {
            sloppy_argument = true;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "mount: unsupported option {}",
                display_os(argument)
            ));
        } else {
            operands.push(argument.clone());
        }
        index += 1;
    }
    if operands.is_empty() {
        return run_cat(&[OsString::from("/proc/self/mounts")]);
    }
    if operands.len() != 2 {
        return Err("mount: expected SOURCE TARGET".to_owned());
    }
    let source = resolve_tagged_source(&operands[0])?;
    let mut filesystem = filesystem.map(|value| canonical_filesystem_type(&value));
    if filesystem
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("auto"))
    {
        filesystem = None;
    }
    if filesystem.is_none() {
        filesystem = detect_filesystem_type(&source);
    }
    let (options, option_flags) = parse_mount_options(&option_values.join(","))?;
    flags |= option_flags;
    let raw_options = option_values.join(",");
    match sys::mount(
        Some(&source),
        Path::new(&operands[1]),
        filesystem.as_deref(),
        (!options.is_empty()).then_some(options.as_str()),
        flags,
    ) {
        Ok(()) => Ok(0),
        Err(direct_error) => {
            match run_mount_helper(
                &source,
                Path::new(&operands[1]),
                filesystem.as_deref(),
                &raw_options,
                read_only_argument,
                bind_argument,
                recursive_bind_argument,
                sloppy_argument,
            ) {
                Ok(()) => Ok(0),
                Err(helper_error) => Err(format!(
                    "mount: {}: direct syscall failed ({direct_error}); helper fallback failed ({helper_error})",
                    display_os(&operands[1])
                )),
            }
        }
    }
}

fn run_mount_helper(
    source: &OsStr,
    target: &Path,
    filesystem: Option<&str>,
    raw_options: &str,
    read_only: bool,
    bind: bool,
    recursive_bind: bool,
    sloppy: bool,
) -> Result<(), String> {
    let helper =
        mount_helper_path().ok_or_else(|| "no distribution mount helper found".to_owned())?;
    let arguments = mount_helper_arguments(
        source,
        target,
        filesystem,
        raw_options,
        read_only,
        bind,
        recursive_bind,
        sloppy,
    );
    let status = Command::new(&helper)
        .args(&arguments)
        .stdin(Stdio::null())
        .status()
        .map_err(|error| format!("cannot execute {}: {error}", helper.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} exited with {status}", helper.display()))
    }
}

fn mount_helper_arguments(
    source: &OsStr,
    target: &Path,
    filesystem: Option<&str>,
    raw_options: &str,
    read_only: bool,
    bind: bool,
    recursive_bind: bool,
    sloppy: bool,
) -> Vec<OsString> {
    let mut arguments = Vec::new();
    if read_only {
        arguments.push(OsString::from("--read-only"));
    }
    if recursive_bind {
        arguments.push(OsString::from("--rbind"));
    } else if bind {
        arguments.push(OsString::from("--bind"));
    }
    if sloppy {
        arguments.push(OsString::from("--sloppy"));
    }
    if let Some(filesystem) = filesystem.filter(|value| !value.eq_ignore_ascii_case("auto")) {
        arguments.extend([OsString::from("-t"), OsString::from(filesystem)]);
    }
    if !raw_options.is_empty() {
        arguments.extend([OsString::from("-o"), OsString::from(raw_options)]);
    }
    arguments.push(OsString::from("--"));
    arguments.push(source.to_os_string());
    arguments.push(target.as_os_str().to_owned());
    arguments
}

fn mount_helper_path() -> Option<PathBuf> {
    let executable = env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    [
        "/usr/bin/mount",
        "/bin/mount",
        "/usr/sbin/mount",
        "/sbin/mount",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|candidate| {
        let metadata = std::fs::metadata(candidate).ok();
        metadata.as_ref().is_some_and(|metadata| {
            metadata.is_file()
                && metadata.permissions().mode() & 0o111 != 0
                && std::fs::canonicalize(candidate).ok() != executable
        })
    })
}

fn resolve_tagged_source(source: &OsStr) -> Result<OsString, String> {
    let value = source.to_str().unwrap_or_default();
    let Some((kind, identifier)) = value.split_once('=') else {
        return Ok(source.to_os_string());
    };
    let normalized_kind = kind.to_ascii_uppercase();
    let (directory, arguments) = match normalized_kind.as_str() {
        "UUID" => ("by-uuid", vec!["-U".to_owned(), identifier.to_owned()]),
        "LABEL" => ("by-label", vec!["-L".to_owned(), identifier.to_owned()]),
        "PARTUUID" | "PARTLABEL" => (
            "by-partuuid",
            vec![
                "-t".to_owned(),
                format!("{normalized_kind}={identifier}"),
                "-o".to_owned(),
                "device".to_owned(),
            ],
        ),
        _ => return Ok(source.to_os_string()),
    };
    let directory = if normalized_kind == "PARTLABEL" {
        "by-partlabel"
    } else {
        directory
    };
    let link = PathBuf::from("/dev/disk").join(directory).join(identifier);
    if link.exists() {
        return Ok(link.into_os_string());
    }

    for program in ["/usr/bin/blkid", "/usr/sbin/blkid", "blkid"] {
        match Command::new(program)
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => {
                if let Some(device) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                {
                    return Ok(OsString::from(device));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "mount: cannot execute {program} while resolving {value}: {error}"
                ));
            }
        }
    }
    Err(format!(
        "cannot resolve tagged source {value}; /dev/disk and blkid did not find it"
    ))
}

fn detect_filesystem_type(source: &OsStr) -> Option<String> {
    for program in ["/usr/bin/blkid", "/usr/sbin/blkid", "blkid"] {
        match Command::new(program)
            .args(["-s", "TYPE", "-o", "value"])
            .arg(source)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => {
                if let Some(filesystem) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                {
                    return Some(canonical_filesystem_type(filesystem));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        }
    }
    None
}

fn canonical_filesystem_type(value: &str) -> String {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "fat" | "msdos" => "vfat".to_owned(),
        "ext" => "ext4".to_owned(),
        _ => value,
    }
}

fn parse_mount_options(value: &str) -> Result<(String, u64), String> {
    let mut flags = 0;
    let mut data = Vec::new();
    for option in value.split(',').filter(|option| !option.is_empty()) {
        match option {
            "defaults" | "auto" | "noauto" | "nofail" | "_netdev" | "suid" | "dev" | "exec"
            | "async" | "atime" => {}
            "ro" => flags |= MS_RDONLY,
            "rw" => flags &= !MS_RDONLY,
            "nosuid" => flags |= MS_NOSUID,
            "nodev" => flags |= MS_NODEV,
            "noexec" => flags |= MS_NOEXEC,
            "sync" => flags |= MS_SYNCHRONOUS,
            "dirsync" => flags |= MS_DIRSYNC,
            "remount" => flags |= MS_REMOUNT,
            "mand" => flags |= MS_MANDLOCK,
            "noatime" => flags |= MS_NOATIME,
            "nodiratime" => flags |= MS_NODIRATIME,
            "relatime" => flags |= MS_RELATIME,
            "strictatime" => flags |= MS_STRICTATIME,
            "lazytime" => flags |= MS_LAZYTIME,
            "bind" => flags |= MS_BIND,
            "rbind" => flags |= MS_BIND | MS_REC,
            "silent" => flags |= MS_SILENT,
            "private" => flags |= MS_PRIVATE,
            "rprivate" => flags |= MS_PRIVATE | MS_REC,
            "slave" => flags |= MS_SLAVE,
            "rslave" => flags |= MS_SLAVE | MS_REC,
            "shared" => flags |= MS_SHARED,
            "rshared" => flags |= MS_SHARED | MS_REC,
            "unbindable" => flags |= MS_UNBINDABLE,
            "runbindable" => flags |= MS_UNBINDABLE | MS_REC,
            other if other.starts_with("x-") || other.starts_with("comment=") => {}
            other => data.push(other.to_owned()),
        }
    }
    Ok((data.join(","), flags))
}

fn run_umount(args: &[OsString]) -> Result<u8, String> {
    let mut flags = 0;
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            operands.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-l" || argument == "--lazy" {
            flags |= MNT_DETACH;
        } else if argument == "-f" || argument == "--force" {
            flags |= MNT_FORCE;
        } else if argument == "-e" || argument == "--expire" {
            flags |= MNT_EXPIRE;
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "umount: unsupported option {}",
                display_os(argument)
            ));
        } else {
            operands.push(argument.clone());
        }
        index += 1;
    }
    if operands.len() != 1 {
        return Err("umount: this profile accepts one target".to_owned());
    }
    sys::unmount(Path::new(&operands[0]), flags)
        .map_err(|error| format!("umount: {}: {error}", display_os(&operands[0])))?;
    Ok(0)
}

fn run_swapon(args: &[OsString]) -> Result<u8, String> {
    let mut priority = None;
    let mut operands = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            operands.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-p" || argument == "--priority" {
            index += 1;
            priority = Some(parse_i32(
                args.get(index)
                    .ok_or_else(|| "swapon: priority is missing".to_owned())?,
                "priority",
            )?);
        } else if argument == "-o" || argument == "--options" {
            return Err("swapon: -o options are not supported by this profile".to_owned());
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "swapon: unsupported option {}",
                display_os(argument)
            ));
        } else {
            operands.push(argument.clone());
        }
        index += 1;
    }
    if operands.len() != 1 {
        return Err("swapon: this profile accepts one swap path".to_owned());
    }
    let flags = priority.map_or(0, |value| SWAP_FLAG_PREFER | (value & SWAP_FLAG_PRIO_MASK));
    let source = resolve_tagged_source(&operands[0])?;
    sys::enable_swap(Path::new(&source), flags)
        .map_err(|error| format!("swapon: {}: {error}", display_os(&operands[0])))?;
    Ok(0)
}

fn run_swapoff(args: &[OsString]) -> Result<u8, String> {
    let operands = positional_args(args, "swapoff")?;
    if operands.len() != 1 {
        return Err("swapoff: this profile accepts one swap path".to_owned());
    }
    let source = resolve_tagged_source(&operands[0])?;
    sys::disable_swap(Path::new(&source))
        .map_err(|error| format!("swapoff: {}: {error}", display_os(&operands[0])))?;
    Ok(0)
}

fn run_init(args: &[OsString]) -> Result<u8, String> {
    let (program, command_args) = if let Some(program) = args.first() {
        (program.clone(), args[1..].to_vec())
    } else if let Some(program) = env::var_os("RUSTYBOX_INIT") {
        (program, vec![OsString::from("daemon")])
    } else {
        (OsString::from("fractald"), vec![OsString::from("daemon")])
    };
    let mut command = Command::new(&program);
    command.args(command_args);
    let error = command.exec();
    Err(format!(
        "cannot become init {}: {error}",
        display_os(&program)
    ))
}

fn run_insmod(args: &[OsString]) -> Result<u8, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("Usage: insmod MODULE [PARAMETERS...]");
        return Ok(0);
    }
    let module = args
        .first()
        .ok_or_else(|| "insmod: missing module path".to_owned())?;
    if module.as_bytes().starts_with(b"-") {
        return Err("insmod: module path must be the first operand".to_owned());
    }
    let parameters = args[1..]
        .iter()
        .map(|argument| display_os(argument))
        .collect::<Vec<_>>()
        .join(" ");
    match sys::insert_module(Path::new(module), &parameters) {
        Ok(()) => Ok(0),
        Err(direct_error) => {
            if let Some(result) = run_external_kmod("insmod", args) {
                return result.map_err(|fallback_error| {
                    format!(
                        "insmod: direct finit_module failed ({direct_error}); helper fallback failed ({fallback_error})"
                    )
                });
            }
            Err(format!("insmod: {}: {direct_error}", display_os(module)))
        }
    }
}

fn run_modprobe(args: &[OsString]) -> Result<u8, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("Usage: modprobe [OPTIONS] MODULE [MODULE...]");
        return Ok(0);
    }
    if let Some(result) = run_external_kmod("modprobe", args) {
        return result;
    }

    let mut remove = false;
    let mut quiet = false;
    let mut root = PathBuf::from("/");
    let mut release = None;
    let mut modules = Vec::new();
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "--" {
            modules.extend_from_slice(&args[index + 1..]);
            break;
        }
        if argument == "-r" || argument == "--remove" {
            remove = true;
        } else if argument == "-q" || argument == "--quiet" {
            quiet = true;
        } else if argument == "-d" || argument == "--dirname" {
            index += 1;
            root = PathBuf::from(
                args.get(index)
                    .ok_or_else(|| "modprobe: module directory is missing")?,
            );
        } else if let Some(value) = strip_os_prefix(argument, b"--dirname=") {
            root = PathBuf::from(value);
        } else if argument == "-S" || argument == "--set-version" {
            index += 1;
            release = Some(
                args.get(index)
                    .ok_or_else(|| "modprobe: kernel release is missing")?
                    .to_string_lossy()
                    .into_owned(),
            );
        } else if let Some(value) = strip_os_prefix(argument, b"--set-version=") {
            release = Some(value.to_string_lossy().into_owned());
        } else if argument == "--first-time" || argument == "-b" || argument == "--use-blacklist" {
        } else if argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "modprobe: unsupported option {}",
                display_os(argument)
            ));
        } else {
            modules.push(argument.clone());
        }
        index += 1;
    }
    if modules.is_empty() {
        return Err("modprobe: missing module name".to_owned());
    }
    let release =
        release.unwrap_or_else(|| sys::uname_field(2).unwrap_or_else(|_| "unknown".to_owned()));
    let mut loaded = HashSet::new();
    for module in modules {
        let result = if remove {
            sys::remove_module(OsStr::new(&normalize_module_name(&module)))
                .map_err(|error| format!("modprobe: {}: {error}", display_os(&module)))
        } else {
            load_module_name(&root, &release, &module, &mut loaded)
        };
        if let Err(error) = result {
            if quiet {
                continue;
            }
            return Err(error);
        }
    }
    Ok(0)
}

fn run_external_kmod(applet: &str, args: &[OsString]) -> Option<Result<u8, String>> {
    let helper = kmod_helper_path(applet)?;
    let status = Command::new(&helper)
        .args(args)
        .status()
        .map_err(|error| format!("cannot execute {}: {error}", helper.display()));
    Some(status.map(exit_status_code))
}

fn kmod_helper_path(applet: &str) -> Option<PathBuf> {
    let override_name = match applet {
        "insmod" => "FRACTALD_KMOD_INSMOD",
        "modprobe" => "FRACTALD_KMOD_MODPROBE",
        _ => return None,
    };
    let executable = env::current_exe()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    let candidates = env::var_os(override_name)
        .map(PathBuf::from)
        .into_iter()
        .chain([
            PathBuf::from(format!("/usr/libexec/fractald/kmod/{applet}")),
            PathBuf::from(format!("/usr/sbin/{applet}")),
            PathBuf::from(format!("/sbin/{applet}")),
            PathBuf::from(format!("/usr/bin/{applet}")),
            PathBuf::from(format!("/bin/{applet}")),
        ])
        .find(|candidate| {
            let metadata = fs::metadata(candidate).ok();
            metadata.as_ref().is_some_and(|metadata| {
                metadata.is_file()
                    && metadata.permissions().mode() & 0o111 != 0
                    && fs::canonicalize(candidate).ok() != executable
            })
        });
    candidates
}

fn load_module_name(
    root: &Path,
    release: &str,
    name: &OsStr,
    loaded: &mut HashSet<String>,
) -> Result<(), String> {
    let normalized = normalize_module_name(name);
    if loaded.contains(&normalized) || kernel_module_loaded(&normalized) {
        loaded.insert(normalized);
        return Ok(());
    }
    let module_root = root.join("lib/modules").join(release);
    let path = find_module_file(&module_root, &normalized).ok_or_else(|| {
        format!(
            "modprobe: cannot find module {normalized} below {}",
            module_root.display()
        )
    })?;
    let dependencies = module_dependencies(&module_root, &path);
    for dependency in dependencies {
        let dependency_name = module_name_from_path(&dependency);
        load_module_name(root, release, OsStr::new(&dependency_name), loaded)?;
    }
    if path.extension().is_some_and(|extension| extension != "ko") {
        return Err(format!(
            "modprobe: compressed module {} requires the distribution kmod helper",
            path.display()
        ));
    }
    sys::insert_module(&path, "")
        .map_err(|error| format!("modprobe: cannot insert {}: {error}", path.display()))?;
    loaded.insert(normalized);
    Ok(())
}

fn find_module_file(root: &Path, name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_module_file(&path, name) {
                return Some(found);
            }
        } else if path.is_file() && module_name_from_path(&path) == name {
            return Some(path);
        }
    }
    None
}

fn module_dependencies(root: &Path, module: &Path) -> Vec<PathBuf> {
    let dependencies = root.join("modules.dep");
    let Ok(contents) = fs::read_to_string(dependencies) else {
        return Vec::new();
    };
    let Ok(relative) = module.strip_prefix(root) else {
        return Vec::new();
    };
    let relative = relative.to_string_lossy();
    contents
        .lines()
        .find_map(|line| {
            let (path, values) = line.split_once(':')?;
            (path == relative).then(|| {
                values
                    .split_whitespace()
                    .map(|dependency| root.join(dependency))
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default()
}

fn module_name_from_path(path: &Path) -> String {
    let mut name = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_owned();
    for suffix in [".xz", ".zst", ".gz", ".bz2", ".lz4", ".lzma"] {
        if let Some(stripped) = name.strip_suffix(suffix) {
            name = stripped.to_owned();
            break;
        }
    }
    name.strip_suffix(".ko").unwrap_or(&name).replace('-', "_")
}

fn normalize_module_name(value: &OsStr) -> String {
    value
        .to_string_lossy()
        .trim_end_matches(".ko")
        .replace('-', "_")
}

fn kernel_module_loaded(name: &str) -> bool {
    fs::read_to_string("/proc/modules")
        .map(|contents| {
            contents.lines().any(|line| {
                line.split_whitespace()
                    .next()
                    .is_some_and(|loaded| loaded == name || loaded.replace('-', "_") == name)
            })
        })
        .unwrap_or(false)
}

fn run_chroot(args: &[OsString]) -> Result<u8, String> {
    let root = args
        .first()
        .ok_or_else(|| "chroot: missing new root".to_owned())?;
    sys::change_root(Path::new(root))
        .map_err(|error| format!("chroot: {}: {error}", display_os(root)))?;
    env::set_current_dir("/").map_err(|error| format!("chroot: cannot enter new root: {error}"))?;
    let program = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| OsString::from("/bin/sh"));
    let mut command = Command::new(&program);
    command.args(&args[2..]);
    let error = command.exec();
    Err(format!(
        "chroot: cannot execute {}: {error}",
        display_os(&program)
    ))
}

fn run_switch_root(args: &[OsString]) -> Result<u8, String> {
    if args
        .iter()
        .any(|argument| argument == "--help" || argument == "-h")
    {
        println!("Usage: switch_root NEW_ROOT INIT [ARGUMENTS...]");
        return Ok(0);
    }
    let new_root = args
        .first()
        .ok_or_else(|| "switch_root: missing new root".to_owned())?;
    let init = args
        .get(1)
        .ok_or_else(|| "switch_root: missing init program".to_owned())?;
    if !Path::new(init).is_absolute() {
        return Err("switch_root: init program must be absolute".to_owned());
    }
    sys::switch_root(Path::new(new_root)).map_err(|error| {
        format!(
            "switch_root: cannot switch to {}: {error}",
            display_os(new_root)
        )
    })?;
    env::set_current_dir("/")
        .map_err(|error| format!("switch_root: cannot enter new root: {error}"))?;
    let mut command = Command::new(init);
    command.args(&args[2..]);
    let error = command.exec();
    Err(format!(
        "switch_root: cannot execute {}: {error}",
        display_os(init)
    ))
}

fn run_chaos(args: &[OsString]) -> Result<u8, String> {
    match args.first().map(|value| value.to_string_lossy()).as_deref() {
        None | Some("list") => {
            for kind in SystemKind::ALL {
                println!("{}", kind.name());
            }
            Ok(0)
        }
        Some("sample") => {
            let name = args
                .get(1)
                .ok_or_else(|| "chaos sample requires a system name".to_owned())?;
            let name = name.to_string_lossy();
            let kind =
                SystemKind::parse(&name).ok_or_else(|| format!("unknown chaos system: {name}"))?;
            sample_chaos(kind);
            Ok(0)
        }
        Some("check") | Some("self-check") => {
            chaos_self_check()?;
            println!("chaos: ok");
            Ok(0)
        }
        Some(_) => Err("usage: rustybox chaos [list|sample <system>|check]".to_owned()),
    }
}

fn chaos_self_check() -> Result<(), String> {
    let lorenz = Lorenz::default().step(Vec3::new(1.0, 1.0, 1.0), 0.01);
    let rossler = Rossler::default().step(Vec3::new(1.0, 1.0, 1.0), 0.01);
    let duffing = Duffing::default().step(DuffingState::new(0.1, 0.0), 0.0, 0.01);
    if ![
        lorenz.x,
        lorenz.y,
        lorenz.z,
        rossler.x,
        rossler.y,
        rossler.z,
        duffing.position,
        duffing.velocity,
    ]
    .into_iter()
    .all(f64::is_finite)
    {
        return Err("continuous chaos model produced a non-finite state".to_owned());
    }
    let logistic = LogisticMap::new(4.0);
    if logistic.next(0.5) != 1.0 {
        return Err("logistic map invariant failed".to_owned());
    }
    let mandelbrot = Mandelbrot::default();
    if !mandelbrot.is_inside(0.0, 0.0) || mandelbrot.is_inside(2.0, 0.0) {
        return Err("Mandelbrot classification invariant failed".to_owned());
    }
    if !Lyapunov::logistic(logistic, 0.2, 100, 1_000).is_finite() {
        return Err("Lyapunov estimator produced a non-finite value".to_owned());
    }
    Ok(())
}

fn sample_chaos(kind: SystemKind) {
    match kind {
        SystemKind::Lorenz => {
            let mut state = Vec3::new(1.0, 1.0, 1.0);
            for _ in 0..100 {
                state = Lorenz::default().step(state, 0.01);
            }
            println!("lorenz: {:.8} {:.8} {:.8}", state.x, state.y, state.z);
        }
        SystemKind::Rossler => {
            let mut state = Vec3::new(1.0, 1.0, 1.0);
            for _ in 0..100 {
                state = Rossler::default().step(state, 0.01);
            }
            println!("rossler: {:.8} {:.8} {:.8}", state.x, state.y, state.z);
        }
        SystemKind::Duffing => {
            let oscillator = Duffing::default();
            let mut state = DuffingState::new(0.1, 0.0);
            for step in 0..100 {
                state = oscillator.step(state, step as f64 * 0.01, 0.01);
            }
            println!("duffing: {:.8} {:.8}", state.position, state.velocity);
        }
        SystemKind::LogisticMap => {
            let values = LogisticMap::new(4.0).sequence(0.2, 8);
            println!(
                "logistic-map: {}",
                values
                    .iter()
                    .map(|value| format!("{value:.8}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        SystemKind::Mandelbrot => {
            let set = Mandelbrot::default();
            println!(
                "mandelbrot: c=0 -> inside, c=2 -> escaped at {}",
                set.escape_iterations(2.0, 0.0)
            );
        }
        SystemKind::Lyapunov => {
            let exponent = Lyapunov::logistic(LogisticMap::new(4.0), 0.2, 1_000, 20_000);
            println!("lyapunov(logistic r=4): {exponent:.8}");
        }
    }
}

fn positional_args(args: &[OsString], applet: &str) -> Result<Vec<OsString>, String> {
    let mut values = Vec::new();
    let mut options = true;
    for argument in args {
        if options && argument == "--" {
            options = false;
        } else if options && argument.as_bytes().starts_with(b"-") {
            return Err(format!(
                "{applet}: unsupported option {}",
                display_os(argument)
            ));
        } else {
            values.push(argument.clone());
        }
    }
    Ok(values)
}

fn command_paths(name: &OsStr) -> Vec<PathBuf> {
    if name.as_bytes().contains(&b'/') {
        return is_executable(Path::new(name))
            .then(|| PathBuf::from(name))
            .into_iter()
            .collect();
    }
    let path = env::var_os("PATH").unwrap_or_default();
    let mut matches = Vec::new();
    for directory in env::split_paths(&path) {
        let candidate = if directory.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            directory.join(name)
        };
        if is_executable(&candidate) {
            matches.push(candidate);
        }
    }
    matches
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn parse_mode(value: &OsStr) -> Result<u32, String> {
    let text = value.to_string_lossy();
    u32::from_str_radix(text.trim_start_matches('0'), 8)
        .or_else(|_| u32::from_str_radix(&text, 8))
        .map_err(|_| format!("invalid mode {text}"))
}

fn parse_duration_milliseconds(value: &OsStr) -> Result<u64, String> {
    let text = value.to_string_lossy();
    let (number, multiplier) = if let Some(value) = text.strip_suffix('d') {
        (value, 86_400.0)
    } else if let Some(value) = text.strip_suffix('h') {
        (value, 3_600.0)
    } else if let Some(value) = text.strip_suffix('m') {
        (value, 60.0)
    } else {
        (text.as_ref(), 1.0)
    };
    let seconds = number
        .parse::<f64>()
        .map_err(|_| format!("invalid duration {text}"))?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(format!("invalid duration {text}"));
    }
    let milliseconds = (seconds * multiplier * 1_000.0).ceil();
    if milliseconds > u64::MAX as f64 {
        return Err(format!("duration is too large: {text}"));
    }
    Ok(milliseconds as u64)
}

fn parse_i32(value: &OsStr, label: &str) -> Result<i32, String> {
    value
        .to_string_lossy()
        .parse::<i32>()
        .map_err(|_| format!("invalid {label} {}", display_os(value)))
}

fn parse_signal(value: &OsStr) -> Result<i32, String> {
    let text = value.to_string_lossy();
    let text = text.strip_prefix("SIG").unwrap_or(&text);
    if let Ok(number) = text.parse::<i32>() {
        return (0..=64)
            .contains(&number)
            .then_some(number)
            .ok_or_else(|| format!("invalid signal {text}"));
    }
    match text.to_ascii_uppercase().as_str() {
        "HUP" => Ok(1),
        "INT" => Ok(2),
        "QUIT" => Ok(3),
        "KILL" => Ok(9),
        "TERM" => Ok(15),
        "STOP" => Ok(19),
        "CONT" => Ok(18),
        "USR1" => Ok(10),
        "USR2" => Ok(12),
        other => Err(format!("invalid signal {other}")),
    }
}

fn split_assignment(value: &OsStr) -> Option<(OsString, OsString)> {
    let bytes = value.as_bytes();
    let position = bytes.iter().position(|byte| *byte == b'=')?;
    let name = OsString::from_vec(bytes[..position].to_vec());
    let value = OsString::from_vec(bytes[position + 1..].to_vec());
    Some((name, value))
}

fn strip_os_prefix(value: &OsStr, prefix: &[u8]) -> Option<OsString> {
    value
        .as_bytes()
        .strip_prefix(prefix)
        .map(|rest| OsString::from_vec(rest.to_vec()))
}

fn write_line(value: &str) -> Result<u8, String> {
    let mut output = value.as_bytes().to_vec();
    output.push(b'\n');
    sys::write_all(1, &output).map_err(|error| format!("cannot write stdout: {error}"))?;
    Ok(0)
}

fn display_os(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

fn exit_status_code(status: std::process::ExitStatus) -> u8 {
    if let Some(code) = status.code() {
        return code.clamp(0, 255) as u8;
    }
    status.signal().map_or(128, |signal| {
        128_u16.saturating_add(signal as u16).min(255) as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applet_manifest_is_sorted_and_unique() {
        let mut sorted = APPLETS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, APPLETS);
    }

    #[test]
    fn duration_parser_handles_units() {
        assert_eq!(parse_duration_milliseconds(OsStr::new("2")), Ok(2_000));
        assert_eq!(parse_duration_milliseconds(OsStr::new("1.5m")), Ok(90_000));
        assert_eq!(parse_duration_milliseconds(OsStr::new("0")), Ok(0));
    }

    #[test]
    fn mount_options_separate_flags_from_filesystem_data() {
        let (data, flags) = parse_mount_options("ro,nosuid,size=64M,relatime").expect("options");
        assert_eq!(data, "size=64M");
        assert_eq!(flags, MS_RDONLY | MS_NOSUID | MS_RELATIME);

        let (data, flags) = parse_mount_options(
            "nofail,_netdev,x-fractald.device-timeout=5s,comment=optional,ro,umask=0077",
        )
        .expect("fstab metadata");
        assert_eq!(data, "umask=0077");
        assert_eq!(flags, MS_RDONLY);
    }

    #[test]
    fn rebuilds_mount_arguments_for_userspace_filesystem_helpers() {
        let arguments = mount_helper_arguments(
            OsStr::new("server:/export"),
            Path::new("/mnt/share"),
            Some("nfs4"),
            "_netdev,vers=4.2",
            true,
            false,
            false,
            false,
        );
        assert_eq!(
            arguments,
            vec![
                OsString::from("--read-only"),
                OsString::from("-t"),
                OsString::from("nfs4"),
                OsString::from("-o"),
                OsString::from("_netdev,vers=4.2"),
                OsString::from("--"),
                OsString::from("server:/export"),
                OsString::from("/mnt/share"),
            ]
        );

        let arguments = mount_helper_arguments(
            OsStr::new("/dev/vda1"),
            Path::new("/mnt/data"),
            Some("xfs"),
            "defaults",
            false,
            false,
            true,
            false,
        );
        assert_eq!(arguments[0], OsString::from("--rbind"));
        assert_eq!(arguments.last(), Some(&OsString::from("/mnt/data")));
    }

    #[test]
    fn canonicalizes_mount_filesystem_aliases() {
        assert_eq!(canonical_filesystem_type("FAT"), "vfat");
        assert_eq!(canonical_filesystem_type("msdos"), "vfat");
        assert_eq!(canonical_filesystem_type("Ext"), "ext4");
        assert_eq!(canonical_filesystem_type("XFS"), "xfs");
    }

    #[test]
    fn decodes_kernel_mountinfo_escapes() {
        assert_eq!(
            decode_mountinfo_field(r"/media/user\040data"),
            "/media/user data"
        );
        assert_eq!(
            decode_mountinfo_field(r"/run\011fractald\134state"),
            "/run\tfractald\\state"
        );
    }

    #[test]
    fn parses_initramfs_sort_keys_and_character_sets() {
        assert_eq!(parse_sort_key(OsStr::new("2,2")), Ok(2));
        assert_eq!(parse_sort_key(OsStr::new("1.2")), Ok(1));
        assert_eq!(
            parse_tr_set(OsStr::new("[:upper:]")).expect("upper"),
            (b'A'..=b'Z').collect::<Vec<_>>()
        );
        assert_eq!(
            parse_tr_set(OsStr::new(r"\040\011")).expect("escapes"),
            vec![b' ', b'\t']
        );
    }

    #[test]
    fn findmnt_output_fields_are_limited_to_kernel_mountinfo_values() {
        assert_eq!(
            parse_findmnt_fields(OsStr::new("TARGET,FSTYPE,SOURCE")).expect("fields"),
            vec!["TARGET", "FSTYPE", "SOURCE"]
        );
        assert!(parse_findmnt_fields(OsStr::new("TARGET,UNKNOWN")).is_err());
    }

    #[test]
    fn chaos_applet_covers_every_model() {
        for kind in SystemKind::ALL {
            assert!(SystemKind::parse(kind.name()).is_some());
        }
    }
}
