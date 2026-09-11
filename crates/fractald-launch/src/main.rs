use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut arguments = env::args_os();
    let _program = arguments.next();
    if arguments.next().as_deref() != Some(OsString::from("--").as_os_str()) {
        eprintln!("fractald-launch: expected -- before the service command");
        return ExitCode::from(2);
    }
    let Some(program) = arguments.next() else {
        eprintln!("fractald-launch: service command is missing");
        return ExitCode::from(2);
    };

    let mut command = Command::new(&program);
    command.args(arguments);
    if env::var_os("LISTEN_FDS").is_some() {
        command.env("LISTEN_PID", std::process::id().to_string());
    } else {
        command.env_remove("LISTEN_PID");
    }
    if let Some(argv0) = env::var_os("FRACTALD_LAUNCH_ARG0") {
        command.arg0(argv0);
        command.env_remove("FRACTALD_LAUNCH_ARG0");
    }
    let error = command.exec();
    eprintln!(
        "fractald-launch: cannot execute {}: {error}",
        program.to_string_lossy()
    );
    ExitCode::from(127)
}
