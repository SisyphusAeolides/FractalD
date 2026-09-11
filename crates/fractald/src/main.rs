use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fractald_chaos::{
    Duffing, DuffingState, LogisticMap, Lorenz, Lyapunov, Mandelbrot, Rossler, SystemKind, Vec3,
};
use fractald_control::{
    Request, Response, RuntimePaths, ShutdownAction, StatePaths, TransactionStatus,
    remove_stale_socket, unit_file_directories, unit_file_enabled_names,
};
use fractald_core::{
    Event, ExitReason, ManagerAction, SIGKILL, ServiceRecord, ServiceSpec, ServiceState,
};
use fractald_platform::{ExitKind, PidFd};
use fractald_storage::{STORAGE_TARGET_UNIT, generate_units, parse_crypttab, parse_fstab};
use fractald_supervisor::{ServiceSnapshot, Supervisor};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("fractald: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8, String> {
    let mut args = env::args_os();
    let _program = args.next();
    let Some(command) = args.next() else {
        if std::process::id() == 1 {
            return run_daemon();
        }
        return Err(usage());
    };

    match command.to_str() {
        Some("--help") | Some("help") => {
            print_help();
            Ok(0)
        }
        Some("--version") | Some("version") => {
            println!("fractald {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Some("self-check") => self_check(),
        Some("chaos") => run_chaos(args),
        Some("daemon") => run_daemon(),
        Some("storage-prepare") => storage_prepare(),
        Some("inspect-unit") => inspect_unit(args),
        Some("run") => run_command(args),
        _ => Err(usage()),
    }
}

fn inspect_unit(mut args: impl Iterator<Item = OsString>) -> Result<u8, String> {
    let path = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| "inspect-unit requires a unit file path".to_owned())?;
    if args.next().is_some() {
        return Err("inspect-unit accepts one unit file path".to_owned());
    }
    let source = fs::read_to_string(&path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("unit path has no valid file name: {}", path.display()))?;
    let unit = fractald_config::UnitFile::parse(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    let spec = parse_loaded_unit(unit, name, &[])
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    println!("name={}", spec.name);
    println!("program={}", spec.program.display());
    println!("type={:?}", spec.service_type);
    println!("bus-names={}", spec.bus_names.join(","));
    println!("restart={:?}", spec.restart);
    println!("job-timeout={:?}", spec.job_timeout);
    println!("job-timeout-action={:?}", spec.job_timeout_action);
    println!("oom-policy={:?}", spec.oom_policy);
    println!("failure-action={:?}", spec.failure_action);
    println!("success-action={:?}", spec.success_action);
    println!("aliases={}", join_names(&spec.aliases));
    println!("private-tmp={:?}", spec.private_tmp);
    println!("private-mounts={}", spec.private_mounts);
    println!("requires={}", join_names(&spec.dependencies.requires));
    println!("wants={}", join_names(&spec.dependencies.wants));
    println!("after={}", join_names(&spec.dependencies.after));
    println!("before={}", join_names(&spec.dependencies.before));
    println!("conflicts={}", join_names(&spec.dependencies.conflicts));
    println!("part_of={}", join_names(&spec.dependencies.part_of));
    println!("binds_to={}", join_names(&spec.dependencies.binds_to));
    println!("requisite={}", join_names(&spec.dependencies.requisite));
    println!("on_failure={}", join_names(&spec.dependencies.on_failure));
    Ok(0)
}

fn join_names(names: &std::collections::BTreeSet<String>) -> String {
    names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",")
}

fn self_check() -> Result<u8, String> {
    chaos_self_check()?;
    let status = supervise("self-check", OsString::from("/bin/sh"), ["-c", "exit 0"])?;
    if status == 0 {
        println!("self-check: ok");
        Ok(0)
    } else {
        Err(format!("self-check child exited with {status}"))
    }
}

fn chaos_self_check() -> Result<(), String> {
    let lorenz = Lorenz::default().step(Vec3::new(1.0, 1.0, 1.0), 0.01);
    let rossler = Rossler::default().step(Vec3::new(1.0, 1.0, 1.0), 0.01);
    let duffing = Duffing::default().step(DuffingState::new(0.1, 0.0), 0.0, 0.01);
    if ![
        lorenz.x, lorenz.y, lorenz.z, rossler.x, rossler.y, rossler.z,
    ]
    .into_iter()
    .chain([duffing.position, duffing.velocity])
    .all(f64::is_finite)
    {
        return Err("continuous chaos model produced a non-finite state".to_owned());
    }
    let logistic = LogisticMap::new(4.0);
    if logistic.next(0.5) != 1.0 {
        return Err("logistic map invariant failed".to_owned());
    }
    if !Mandelbrot::default().is_inside(0.0, 0.0) || Mandelbrot::default().is_inside(2.0, 0.0) {
        return Err("Mandelbrot classification invariant failed".to_owned());
    }
    let exponent = Lyapunov::logistic(logistic, 0.2, 100, 1_000);
    if !exponent.is_finite() {
        return Err("Lyapunov estimator produced a non-finite value".to_owned());
    }
    Ok(())
}

fn run_chaos(mut args: impl Iterator<Item = OsString>) -> Result<u8, String> {
    match args
        .next()
        .and_then(|value| value.into_string().ok())
        .as_deref()
    {
        None | Some("list") => {
            for kind in SystemKind::ALL {
                println!("{}", kind.name());
            }
            Ok(0)
        }
        Some("sample") => {
            let name = args
                .next()
                .ok_or_else(|| "chaos sample requires a system name".to_owned())?;
            let name = name.to_string_lossy();
            let kind =
                SystemKind::parse(&name).ok_or_else(|| format!("unknown chaos system: {name}"))?;
            sample_chaos(kind);
            Ok(0)
        }
        Some(_) => Err("usage: fractald chaos [list|sample <system>]".to_owned()),
    }
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

fn run_daemon() -> Result<u8, String> {
    let pid1 = std::process::id() == 1;
    if pid1 {
        fractald_platform::set_child_subreaper()
            .map_err(|error| format!("cannot initialize PID1 child reaping: {error}"))?;
    }
    fractald_platform::install_shutdown_handlers()
        .map_err(|error| format!("cannot install shutdown handlers: {error}"))?;
    let paths = RuntimePaths::from_environment();
    let state = StatePaths::from_environment();
    let mut supervisor = load_services()?;
    let reconciled = supervisor
        .reconcile_orphans()
        .map_err(|error| format!("cannot reconcile stale service cgroups: {error}"))?;
    if reconciled > 0 {
        eprintln!("fractald: reconciled {reconciled} stale service cgroup(s)");
    }
    let mut journal = EventJournal::open(&state)?;
    let _ = journal.observe(&supervisor)?;
    start_boot_target(&mut supervisor);
    start_storage_target(&mut supervisor);
    start_enabled(&mut supervisor, &state)?;
    start_openrc_runlevel(&mut supervisor);
    let _ = journal.observe(&supervisor)?;
    paths.ensure_directory().map_err(|error| {
        format!(
            "cannot create runtime directory {}: {error}",
            paths.directory.display()
        )
    })?;
    let listener = bind_listener(&paths)?;
    let pid = std::process::id();
    if let Err(error) = fs::write(&paths.pid, format!("{pid}\n")) {
        let _ = remove_stale_socket(&paths.socket);
        return Err(format!(
            "cannot write pid file {}: {error}",
            paths.pid.display()
        ));
    }
    let started = Instant::now();
    let result = daemon_loop(
        &listener,
        &paths,
        &state,
        pid,
        started,
        &mut supervisor,
        &mut journal,
    );
    let _ = fs::remove_file(&paths.pid);
    let _ = remove_stale_socket(&paths.socket);
    result.map(|_| 0)
}

fn start_boot_target(supervisor: &mut Supervisor) {
    let Some(target) = env::var_os("FRACTALD_BOOT_TARGET").filter(|value| !value.is_empty()) else {
        return;
    };
    let target = target.to_string_lossy();
    let name = match ensure_service_loaded(supervisor, &target) {
        Ok(name) => name,
        Err(error) => {
            eprintln!("fractald: cannot load boot target {target}: {error}");
            return;
        }
    };
    if let Err(error) = supervisor.start(&name) {
        eprintln!("fractald: cannot start boot target {name}: {error}");
    }
}

fn start_storage_target(supervisor: &mut Supervisor) {
    if supervisor.resolve_name(STORAGE_TARGET_UNIT).is_none() {
        return;
    }
    if let Err(error) = supervisor.start(STORAGE_TARGET_UNIT) {
        eprintln!("fractald: cannot start storage target: {error}");
    }
}

fn storage_prepare() -> Result<u8, String> {
    // The first pass exposes devices that can be discovered without an
    // encrypted mapping.  The crypttab pass may then create a device-mapper
    // node containing a PV, an md member, or a Btrfs device, so repeat the
    // topology scan after it completes.
    activate_block_storage()?;
    activate_daemon_crypttab()?;
    activate_block_storage()?;
    println!("storage-prepare: complete");
    Ok(0)
}

fn activate_block_storage() -> Result<(), String> {
    for (label, programs, args) in [
        (
            "Btrfs device scan",
            ["/usr/bin/btrfs", "/usr/sbin/btrfs", "btrfs"].as_slice(),
            ["device", "scan", "--all-devices"].as_slice(),
        ),
        (
            "mdraid assembly",
            ["/usr/sbin/mdadm", "/usr/bin/mdadm", "mdadm"].as_slice(),
            ["--assemble", "--scan"].as_slice(),
        ),
        (
            "LVM PV scan",
            ["/usr/bin/lvm", "/usr/sbin/lvm", "lvm"].as_slice(),
            ["pvscan", "--cache"].as_slice(),
        ),
        (
            "LVM volume-group activation",
            ["/usr/bin/lvm", "/usr/sbin/lvm", "lvm"].as_slice(),
            ["vgchange", "--activate", "y"].as_slice(),
        ),
    ] {
        match run_storage_command(programs, args)? {
            Some(true) | None => {}
            Some(false) => eprintln!("fractald: {label} returned a nonzero status"),
        }
    }
    Ok(())
}

fn run_storage_command(programs: &[&str], args: &[&str]) -> Result<Option<bool>, String> {
    for program in programs {
        let result = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        match result {
            Ok(status) => return Ok(Some(status.success())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("cannot execute {program}: {error}")),
        }
    }
    Ok(None)
}

fn activate_daemon_crypttab() -> Result<(), String> {
    let path = env::var_os("FRACTALD_CRYPTTAB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/crypttab"));
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let entries = parse_crypttab(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    for entry in entries {
        if entry.noauto() || entry.source == "none" {
            continue;
        }
        let Some(key) = entry.key.as_deref() else {
            eprintln!(
                "fractald: skipping interactive crypttab mapping {} during daemon storage preparation",
                entry.name
            );
            continue;
        };
        let status = run_storage_command(
            &["/usr/sbin/cryptsetup", "/usr/bin/cryptsetup", "cryptsetup"],
            &["status", &entry.name],
        )?;
        if status == Some(true) {
            continue;
        }
        let source = match resolve_crypttab_source(&entry.source) {
            Ok(Some(source)) => source,
            Ok(None) if entry.nofail() => {
                eprintln!(
                    "fractald: optional crypttab source {} for {} is not available",
                    entry.source, entry.name
                );
                continue;
            }
            Ok(None) => {
                return Err(format!(
                    "cannot resolve crypttab source {} for {}",
                    entry.source, entry.name
                ));
            }
            Err(error) if entry.nofail() => {
                eprintln!(
                    "fractald: optional crypttab mapping {} skipped: {error}",
                    entry.name
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        let mut args = vec!["open"];
        if key != "-" {
            args.extend(["--key-file", key]);
        }
        if entry.options.iter().any(|option| option == "discard") {
            args.push("--allow-discards");
        }
        args.extend([source.as_str(), entry.name.as_str()]);
        let opened = run_storage_command(
            &["/usr/sbin/cryptsetup", "/usr/bin/cryptsetup", "cryptsetup"],
            &args,
        )?;
        if opened != Some(true) && !entry.nofail() {
            return Err(format!("cannot open crypttab mapping {}", entry.name));
        }
    }
    Ok(())
}

fn resolve_crypttab_source(source: &str) -> Result<Option<String>, String> {
    let (query, value) = match source
        .split_once('=')
        .map(|(kind, value)| (kind.to_ascii_uppercase(), value))
    {
        Some((kind, value))
            if matches!(kind.as_str(), "UUID" | "LABEL" | "PARTUUID" | "PARTLABEL") =>
        {
            (Some(kind), Some(value))
        }
        _ => return Ok(Some(source.to_owned())),
    };
    let query = query.expect("storage query exists");
    let value = value.expect("storage query value exists");
    let directory = match query.as_str() {
        "UUID" => "by-uuid",
        "LABEL" => "by-label",
        "PARTUUID" => "by-partuuid",
        "PARTLABEL" => "by-partlabel",
        _ => unreachable!("validated storage query"),
    };
    let device_link = PathBuf::from("/dev/disk").join(directory).join(value);
    if device_link.exists() {
        return Ok(Some(device_link.to_string_lossy().into_owned()));
    }

    let mut arguments = Vec::new();
    match query.as_str() {
        "UUID" => arguments.extend(["-U".to_owned(), value.to_owned()]),
        "LABEL" => arguments.extend(["-L".to_owned(), value.to_owned()]),
        "PARTUUID" | "PARTLABEL" => arguments.extend([
            "-t".to_owned(),
            format!("{query}={value}"),
            "-o".to_owned(),
            "device".to_owned(),
        ]),
        _ => unreachable!("validated storage query"),
    }
    for program in ["/usr/bin/blkid", "/usr/sbin/blkid", "blkid"] {
        match Command::new(program)
            .args(&arguments)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => {
                if let Some(device) = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                {
                    return Ok(Some(device.to_owned()));
                }
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot execute {program} while resolving {source}: {error}"
                ));
            }
        }
    }
    Ok(None)
}

fn start_enabled(supervisor: &mut Supervisor, state: &StatePaths) -> Result<(), String> {
    let mut names = state
        .enabled_names()
        .map_err(|error| format!("cannot read enabled services: {error}"))?;
    names.extend(
        unit_file_enabled_names(&unit_file_directories())
            .map_err(|error| format!("cannot read standard unit enablement: {error}"))?,
    );
    names.retain(|name| name != "fractald.service");
    names.sort();
    names.dedup();
    for requested_name in names {
        let name = match ensure_service_loaded(supervisor, &requested_name) {
            Ok(name) => name,
            Err(error) => {
                eprintln!("fractald: cannot load enabled service {requested_name}: {error}");
                continue;
            }
        };
        if let Err(error) = supervisor.start(&name) {
            eprintln!("fractald: cannot start enabled service {name}: {error}");
        }
    }
    Ok(())
}

fn start_openrc_runlevel(supervisor: &mut Supervisor) {
    let Some(runlevel) = env::var_os("FRACTALD_OPENRC_RUNLEVEL") else {
        return;
    };
    let directory = env::var_os("FRACTALD_OPENRC_RUNLEVEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/runlevels"))
        .join(runlevel);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "fractald: cannot read OpenRC runlevel {}: {error}",
                directory.display()
            );
            return;
        }
    };
    let mut names = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .collect::<Vec<_>>();
    names.sort();
    for name in names {
        let Some(service_name) = supervisor.resolve_name(&name).map(str::to_owned) else {
            eprintln!("fractald: OpenRC runlevel service {name} has no definition");
            continue;
        };
        if let Err(error) = supervisor.start(&service_name) {
            eprintln!("fractald: cannot start OpenRC runlevel service {service_name}: {error}");
        }
    }
}

fn load_services() -> Result<Supervisor, String> {
    let mut supervisor = Supervisor::new();
    for spec in load_all_service_specs()? {
        supervisor
            .add(spec)
            .map_err(|error| format!("cannot load service: {error}"))?;
    }
    Ok(supervisor)
}

fn load_all_service_specs() -> Result<Vec<ServiceSpec>, String> {
    prepare_generated_units()?;
    let mut specs = load_service_specs()?;
    specs.extend(load_openrc_specs()?);
    append_synthetic_device_specs(&mut specs)?;
    Ok(specs)
}

fn append_synthetic_device_specs(specs: &mut Vec<ServiceSpec>) -> Result<(), String> {
    let mut known = specs
        .iter()
        .map(|spec| spec.name.clone())
        .collect::<BTreeSet<_>>();
    let mut index = 0;
    while index < specs.len() {
        let dependencies = synthetic_device_dependencies(&specs[index], &known);
        for dependency in dependencies {
            if let Some(device) = load_named_service_spec(&dependency)? {
                known.insert(device.name.clone());
                specs.push(device);
            }
        }
        index += 1;
    }
    Ok(())
}

fn synthetic_device_dependencies(spec: &ServiceSpec, known: &BTreeSet<String>) -> BTreeSet<String> {
    [
        &spec.dependencies.requires,
        &spec.dependencies.wants,
        &spec.dependencies.after,
        &spec.dependencies.before,
        &spec.dependencies.conflicts,
        &spec.dependencies.part_of,
        &spec.dependencies.binds_to,
        &spec.dependencies.requisite,
        &spec.dependencies.on_success,
        &spec.dependencies.on_failure,
    ]
    .into_iter()
    .flat_map(|set| set.iter())
    .filter(|name| name.ends_with(".device") && !known.contains(*name))
    .cloned()
    .collect()
}

fn ensure_service_loaded(supervisor: &mut Supervisor, requested: &str) -> Result<String, String> {
    let directories = service_directories();
    let requested =
        canonical_unit_name(&directories, requested)?.unwrap_or_else(|| requested.to_owned());
    let requested = supervisor
        .resolve_name(&requested)
        .map(str::to_owned)
        .unwrap_or_else(|| normalize_service_name(&requested));
    let masked = StatePaths::from_environment()
        .is_masked(&requested)
        .map_err(|error| format!("cannot inspect masked state for {requested}: {error}"))?;
    if masked || unit_path_is_masked(&directories, &requested)? {
        return Err(format!("unit {requested} is masked"));
    }
    let mut loading = BTreeSet::new();
    ensure_service_loaded_inner(supervisor, &requested, &mut loading)
}

fn ensure_service_loaded_inner(
    supervisor: &mut Supervisor,
    requested: &str,
    loading: &mut BTreeSet<String>,
) -> Result<String, String> {
    let (name, spec) = if let Some(name) = supervisor.resolve_name(requested).map(str::to_owned) {
        let spec = supervisor
            .specification(&name)
            .cloned()
            .ok_or_else(|| format!("unknown service {name}"))?;
        (name, spec)
    } else {
        let Some(spec) = load_named_service_spec(requested)? else {
            return Err(format!("unknown service {requested}"));
        };
        let name = spec.name.clone();
        (name, spec)
    };
    if !loading.insert(name.clone()) {
        return Ok(name);
    };
    let trigger_target = spec.trigger.as_ref().map(|trigger| match trigger {
        fractald_core::TriggerSpec::Timer { service, .. }
        | fractald_core::TriggerSpec::Path { service, .. } => service.clone(),
    });
    let mut dependencies = spec
        .dependencies
        .requires
        .iter()
        .chain(spec.dependencies.wants.iter())
        .chain(spec.dependencies.binds_to.iter())
        .chain(spec.dependencies.requisite.iter())
        .cloned()
        .collect::<Vec<_>>();
    dependencies.extend(mount_unit_candidates(&spec)?);
    if supervisor.resolve_name(&name).is_none() {
        supervisor
            .add(spec)
            .map_err(|error| format!("cannot load service {name}: {error}"))?;
    }
    for dependency in dependencies {
        if supervisor.resolve_name(&dependency).is_some() {
            continue;
        }
        if load_named_service_spec(&normalize_service_name(&dependency))?.is_some() {
            ensure_service_loaded_inner(supervisor, &normalize_service_name(&dependency), loading)?;
        }
    }
    if let Some(target) = trigger_target {
        if supervisor.resolve_name(&target).is_none() {
            let target = normalize_service_name(&target);
            if load_named_service_spec(&target)?.is_some() {
                ensure_service_loaded_inner(supervisor, &target, loading)?;
            }
        }
    }
    Ok(name)
}

fn mount_unit_candidates(spec: &ServiceSpec) -> Result<Vec<String>, String> {
    let mut candidates = BTreeSet::new();
    let paths = spec
        .expanded_requires_mounts_for()
        .into_iter()
        .chain(spec.expanded_wants_mounts_for())
        .collect::<Vec<_>>();
    for path in paths {
        for candidate in mount_unit_names_for_path(&path) {
            if load_named_service_spec(&candidate)?.is_some() {
                candidates.insert(candidate);
            }
        }
    }
    Ok(candidates.into_iter().collect())
}

fn mount_unit_names_for_path(path: &Path) -> Vec<String> {
    if !path.is_absolute() {
        return Vec::new();
    }
    let mut prefixes = vec![PathBuf::from("/")];
    let mut prefix = PathBuf::from("/");
    for component in path.components() {
        let std::path::Component::Normal(component) = component else {
            continue;
        };
        prefix.push(component);
        prefixes.push(prefix.clone());
    }
    prefixes
        .into_iter()
        .map(|prefix| format!("{}.mount", escape_mount_path(&prefix)))
        .collect()
}

fn escape_mount_path(path: &Path) -> String {
    if path == Path::new("/") {
        return "-".to_owned();
    }
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => {
                Some(escape_mount_component(value.as_encoded_bytes()))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn escape_mount_component(value: &[u8]) -> String {
    value
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':') {
                (*byte as char).to_string()
            } else {
                format!("\\x{byte:02x}")
            }
        })
        .collect()
}

fn normalize_service_name(name: &str) -> String {
    if name.contains('.') {
        name.to_owned()
    } else {
        format!("{name}.service")
    }
}

fn load_named_service_spec(name: &str) -> Result<Option<ServiceSpec>, String> {
    let directories = service_directories();
    let state = StatePaths::from_environment();
    if state
        .is_masked(name)
        .map_err(|error| format!("cannot inspect masked state for {name}: {error}"))?
        || unit_path_is_masked(&directories, name)?
    {
        return Ok(None);
    }
    if let Some(unit) = load_unit(&directories, name)? {
        return parse_loaded_unit(unit, name, &directories).map(Some);
    }

    if let Some(path) = device_unit_path(name) {
        return fractald_config::UnitFile::default()
            .to_device_spec(name.to_owned(), path)
            .map(Some)
            .map_err(|error| format!("cannot create {name}: {error}"));
    }

    if let Some(instance) = name.strip_suffix(".service") {
        if let Some(at) = instance.rfind('@') {
            let template = format!("{}@.service", &instance[..at]);
            if let Some(unit) = load_unit(&directories, &template)? {
                return parse_loaded_unit(unit, name, &directories).map(Some);
            }
        }
    }

    let Some(directory) = openrc_directory() else {
        return Ok(None);
    };
    let script_name = name.strip_suffix(".service").unwrap_or(name);
    let path = directory.join(script_name);
    match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            let source = fs::read_to_string(&path).map_err(|error| {
                format!("cannot read OpenRC script {}: {error}", path.display())
            })?;
            fractald_config::parse_openrc_script(&source, script_name, &path)
                .map(Some)
                .map_err(|error| format!("cannot parse OpenRC script {}: {error}", path.display()))
        }
        Ok(_) => Err(format!(
            "OpenRC service path {} is not a file",
            path.display()
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "cannot inspect OpenRC script {}: {error}",
            path.display()
        )),
    }
}

fn device_unit_path(name: &str) -> Option<PathBuf> {
    let stem = name.strip_suffix(".device")?;
    let encoded = stem
        .strip_prefix("dev-")
        .or_else(|| stem.strip_prefix("sys-"))?;
    let prefix = if stem.starts_with("dev-") {
        "/dev/"
    } else {
        "/sys/"
    };
    let mut path = String::from(prefix);
    let bytes = encoded.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1] == b'x'
            && hex_digit(bytes[index + 2]).is_some()
            && hex_digit(bytes[index + 3]).is_some()
        {
            let high = hex_digit(bytes[index + 2]).expect("checked hex digit");
            let low = hex_digit(bytes[index + 3]).expect("checked hex digit");
            path.push(char::from((high << 4) | low));
            index += 4;
        } else if bytes[index] == b'-' {
            path.push('/');
            index += 1;
        } else {
            path.push(char::from(bytes[index]));
            index += 1;
        }
    }
    Some(PathBuf::from(path))
}

fn hex_digit(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn parse_loaded_unit(
    unit: fractald_config::UnitFile,
    name: &str,
    directories: &[PathBuf],
) -> Result<ServiceSpec, String> {
    if name.ends_with(".target") {
        let mut spec = unit
            .to_target_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))?;
        augment_target_links(&mut spec, directories, name)?;
        Ok(spec)
    } else if name.ends_with(".socket") {
        unit.to_socket_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".timer") {
        unit.to_timer_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".path") {
        unit.to_path_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".mount") {
        unit.to_mount_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".swap") {
        unit.to_swap_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".device") {
        let path = device_unit_path(name)
            .ok_or_else(|| format!("cannot derive a device path from {name}"))?;
        unit.to_device_spec(name.to_owned(), path)
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".automount") {
        unit.to_automount_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".slice") {
        unit.to_slice_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else if name.ends_with(".service")
        && !unit.has_section("Service")
        && unit.value("Unit", "SuccessAction").is_some()
    {
        unit.to_action_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    } else {
        unit.to_service_spec(name.to_owned())
            .map_err(|error| format!("cannot parse {name}: {error}"))
    }
}

const GENERATOR_MARKER: &str = ".fractald-generator-output";
const DEFAULT_GENERATOR_TIMEOUT_MS: u64 = 5_000;

fn prepare_generated_units() -> Result<(), String> {
    let outputs = generated_unit_directories();
    let directories = generator_directories();
    let storage_enabled = storage_generation_enabled();

    if directories.is_empty() && !storage_enabled {
        for output in &outputs {
            if generated_directory_is_owned(output) {
                clear_generated_units(output)?;
            }
        }
        return Ok(());
    }

    for output in &outputs {
        ensure_generated_directory(output)?;
        clear_generated_units(output)?;
        let marker = output.join(GENERATOR_MARKER);
        fs::write(&marker, b"FractalD generated unit directory\n").map_err(|error| {
            format!(
                "cannot mark generated unit directory {}: {error}",
                marker.display()
            )
        })?;
    }

    if storage_enabled {
        generate_storage_units(&outputs[0])?;
    }

    let timeout = Duration::from_millis(
        env::var("FRACTALD_GENERATOR_TIMEOUT_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_GENERATOR_TIMEOUT_MS)
            .clamp(100, 60_000),
    );
    run_generators(&directories, &outputs, timeout)
}

fn run_generators(
    directories: &[PathBuf],
    outputs: &[PathBuf; 3],
    timeout: Duration,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                eprintln!(
                    "fractald: cannot read generator directory {}: {error}",
                    directory.display()
                );
                continue;
            }
        };
        let mut paths = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if name.starts_with('.') || !seen.insert(name.to_owned()) {
                continue;
            }
            if storage_generation_enabled() && name == "systemd-fstab-generator" {
                continue;
            }
            let metadata = match fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    eprintln!(
                        "fractald: cannot inspect generator {}: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
            if let Err(error) = run_generator(&path, outputs, timeout) {
                eprintln!("fractald: generator {} failed: {error}", path.display());
            }
        }
    }
    Ok(())
}

fn storage_generation_enabled() -> bool {
    env::var_os("FRACTALD_STORAGE_FSTAB").is_some()
        || env::var_os("FRACTALD_STORAGE_ENABLE").is_some_and(|value| {
            matches!(
                value.to_string_lossy().as_ref(),
                "1" | "yes" | "true" | "on"
            )
        })
        || (fractald_platform::is_root() && env::var_os("FRACTALD_SERVICE_DIR").is_none())
}

fn generate_storage_units(output: &Path) -> Result<(), String> {
    let path = env::var_os("FRACTALD_STORAGE_FSTAB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/fstab"));
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot read {}: {error}", path.display())),
    };
    let entries = parse_fstab(&source)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
    for unit in generate_units(&entries)
        .map_err(|error| format!("cannot generate storage units: {error}"))?
    {
        let path = output.join(&unit.name);
        fs::write(&path, unit.source)
            .map_err(|error| format!("cannot write generated unit {}: {error}", path.display()))?;
    }
    Ok(())
}

fn run_generator(path: &Path, outputs: &[PathBuf; 3], timeout: Duration) -> Result<(), String> {
    let parent_pid = std::process::id();
    let mut command = Command::new(path);
    command
        .args(outputs)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
            fractald_platform::set_process_group()?;
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot execute: {error}"))?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(Instant::now);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("exited with {status}"));
            }
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let pid = child.id();
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} ms", timeout.as_millis()));
            }
            Err(error) => {
                let pid = child.id();
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("cannot inspect process: {error}"));
            }
        }
    }
}

fn clear_generated_units(output: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(output) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "cannot read generated unit directory {}: {error}",
                output.display()
            ));
        }
    };
    for entry in entries {
        let path = entry
            .map_err(|error| format!("cannot enumerate generated unit directory: {error}"))?
            .path();
        if path.file_name().and_then(|value| value.to_str()) == Some(GENERATOR_MARKER) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!("cannot inspect generated entry {}: {error}", path.display())
        })?;
        if metadata.file_type().is_dir() {
            fs::remove_dir_all(&path).map_err(|error| {
                format!(
                    "cannot remove generated directory {}: {error}",
                    path.display()
                )
            })?;
        } else {
            fs::remove_file(&path).map_err(|error| {
                format!("cannot remove generated entry {}: {error}", path.display())
            })?;
        }
    }
    Ok(())
}

fn ensure_generated_directory(output: &Path) -> Result<(), String> {
    match fs::symlink_metadata(output) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(format!(
                "generated unit path {} is not a directory",
                output.display()
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(output).map_err(|error| {
                format!(
                    "cannot create generated unit directory {}: {error}",
                    output.display()
                )
            })?;
        }
        Err(error) => {
            return Err(format!(
                "cannot inspect generated unit directory {}: {error}",
                output.display()
            ));
        }
    }
    let mut permissions = fs::metadata(output)
        .map_err(|error| format!("cannot inspect generated unit directory: {error}"))?
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(output, permissions).map_err(|error| {
        format!(
            "cannot set generated unit directory permissions {}: {error}",
            output.display()
        )
    })
}

fn generated_directory_is_owned(output: &Path) -> bool {
    fs::symlink_metadata(output.join(GENERATOR_MARKER))
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn generated_unit_directory() -> PathBuf {
    env::var_os("FRACTALD_GENERATED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| RuntimePaths::from_environment().directory.join("generator"))
}

fn generated_unit_directories() -> [PathBuf; 3] {
    let normal = generated_unit_directory();
    let parent = normal.parent().unwrap_or_else(|| Path::new(".")).to_owned();
    let name = normal
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("generator")
        .to_owned();
    [
        normal,
        parent.join(format!("{name}.early")),
        parent.join(format!("{name}.late")),
    ]
}

fn generator_directories() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_GENERATOR_DIR") {
        return vec![PathBuf::from(path)];
    }
    if env::var_os("FRACTALD_SERVICE_DIR").is_some() {
        return Vec::new();
    }
    if fractald_platform::is_root() {
        vec![
            PathBuf::from("/etc/systemd/system-generators"),
            PathBuf::from("/run/systemd/system-generators"),
            PathBuf::from("/usr/local/lib/systemd/system-generators"),
            PathBuf::from("/usr/lib/systemd/system-generators"),
        ]
    } else {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", fractald_platform::effective_uid()))
            });
        let mut directories = vec![
            runtime.join("systemd/user-generators"),
            PathBuf::from("/usr/local/lib/systemd/user-generators"),
            PathBuf::from("/usr/lib/systemd/user-generators"),
        ];
        if let Some(config_home) = config_home {
            directories.insert(0, config_home.join("systemd/user-generators"));
        }
        directories
    }
}

fn load_service_specs() -> Result<Vec<ServiceSpec>, String> {
    let directories = service_directories();
    let state = StatePaths::from_environment();
    let strict = env::var_os("FRACTALD_SERVICE_DIR").is_some();
    let names = discover_service_names(&directories)?;
    let mut specs = Vec::new();
    for name in names {
        if state
            .is_masked(&name)
            .map_err(|error| format!("cannot inspect masked state for {name}: {error}"))?
            || unit_path_is_masked(&directories, &name)?
        {
            continue;
        }
        let Some(unit) = load_unit(&directories, &name)? else {
            continue;
        };
        match parse_loaded_unit(unit, &name, &directories) {
            Ok(spec) => specs.push(spec),
            Err(error) if strict => return Err(error),
            Err(error) => {
                eprintln!("fractald: skipping incompatible systemd unit {name}: {error}");
            }
        }
    }
    Ok(specs)
}

fn service_directories() -> Vec<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_SERVICE_DIR") {
        if env::var_os("FRACTALD_GENERATOR_DIR").is_some() || storage_generation_enabled() {
            let [normal, early, late] = generated_unit_directories();
            return vec![late, PathBuf::from(path), normal, early];
        }
        return vec![PathBuf::from(path)];
    }
    if fractald_platform::is_root() {
        let [normal, early, late] = generated_unit_directories();
        vec![
            late,
            PathBuf::from("/usr/lib/systemd/system"),
            PathBuf::from("/usr/local/lib/systemd/system"),
            PathBuf::from("/lib/systemd/system"),
            PathBuf::from("/run/systemd/system"),
            normal,
            PathBuf::from("/etc/systemd/system"),
            PathBuf::from("/etc/fractald/services"),
            early,
        ]
    } else {
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", fractald_platform::effective_uid()))
            });
        let [normal, early, late] = generated_unit_directories();
        let mut directories = vec![
            late,
            PathBuf::from("/usr/lib/systemd/user"),
            PathBuf::from("/usr/local/lib/systemd/user"),
            runtime.join("systemd/user"),
            normal,
        ];
        if let Some(config_home) = config_home {
            directories.push(config_home.join("systemd/user"));
            directories.push(config_home.join("fractald/services"));
        }
        directories.push(early);
        directories
    }
}

fn discover_service_names(directories: &[PathBuf]) -> Result<BTreeSet<String>, String> {
    let mut names = BTreeSet::new();
    for directory in directories {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "cannot read service directory {}: {error}",
                    directory.display()
                ));
            }
        };
        for entry in entries {
            let path = entry
                .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?
                .path();
            let Some(file_name) = path.file_name().and_then(|value| value.to_str()) else {
                continue;
            };
            if is_discoverable_unit_name(file_name) && path.is_file() {
                if unit_path_is_masked(directories, file_name)? {
                    continue;
                }
                if canonical_unit_name(directories, file_name)?.is_some() {
                    continue;
                }
                names.insert(file_name.to_owned());
            } else if file_name.ends_with(".d")
                && is_discoverable_unit_name(file_name.trim_end_matches(".d"))
                && path.is_dir()
            {
                let base = file_name.trim_end_matches(".d");
                names.insert(base.to_owned());
            } else if (file_name.ends_with(".wants") || file_name.ends_with(".requires"))
                && path.is_dir()
            {
                let entries = fs::read_dir(&path).map_err(|error| {
                    format!(
                        "cannot read unit relationship directory {}: {error}",
                        path.display()
                    )
                })?;
                for entry in entries {
                    let entry = entry
                        .map_err(|error| format!("cannot enumerate {}: {error}", path.display()))?;
                    let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                        continue;
                    };
                    if is_discoverable_unit_name(&name) {
                        names.insert(canonical_unit_name(directories, &name)?.unwrap_or(name));
                    }
                }
            }
        }
    }
    Ok(names)
}

fn is_discoverable_unit_name(name: &str) -> bool {
    [
        ".automount",
        ".device",
        ".mount",
        ".path",
        ".service",
        ".slice",
        ".socket",
        ".swap",
        ".target",
        ".timer",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
}

fn augment_target_links(
    spec: &mut ServiceSpec,
    directories: &[PathBuf],
    target: &str,
) -> Result<(), String> {
    for (suffix, destination) in [
        ("wants", &mut spec.dependencies.wants),
        ("requires", &mut spec.dependencies.requires),
    ] {
        for directory in directories {
            let relationship_directory = directory.join(format!("{target}.{suffix}"));
            let entries = match fs::read_dir(&relationship_directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot read target relationship directory {}: {error}",
                        relationship_directory.display()
                    ));
                }
            };
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!(
                        "cannot enumerate {}: {error}",
                        relationship_directory.display()
                    )
                })?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if is_discoverable_unit_name(&name) {
                    if fs::read_link(entry.path())
                        .ok()
                        .is_some_and(|path| path == PathBuf::from("/dev/null"))
                    {
                        continue;
                    }
                    destination.insert(canonical_unit_name(directories, &name)?.unwrap_or(name));
                }
            }
        }
    }
    Ok(())
}

fn canonical_unit_name(directories: &[PathBuf], name: &str) -> Result<Option<String>, String> {
    let mut current = name.to_owned();
    let mut seen = BTreeSet::new();
    let mut changed = false;
    while seen.insert(current.clone()) {
        let mut link = None;
        for directory in directories.iter().rev() {
            let path = directory.join(&current);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    link = Some(path);
                    break;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!("cannot inspect {}: {error}", path.display()));
                }
            }
        }
        let Some(link) = link else {
            break;
        };
        let target = fs::read_link(&link)
            .map_err(|error| format!("cannot read unit alias {}: {error}", link.display()))?;
        let Some(target_name) = target.file_name().and_then(|value| value.to_str()) else {
            break;
        };
        if target_name == current || target_name == "." || target_name == ".." {
            break;
        }
        let target_exists = directories
            .iter()
            .any(|directory| fs::symlink_metadata(directory.join(target_name)).is_ok());
        if !target_exists {
            break;
        }
        current = target_name.to_owned();
        changed = true;
    }
    Ok(changed.then_some(current))
}

fn unit_path_is_masked(directories: &[PathBuf], name: &str) -> Result<bool, String> {
    for directory in directories.iter().rev() {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                let target = fs::read_link(&path).map_err(|error| {
                    format!("cannot read unit mask {}: {error}", path.display())
                })?;
                if target == Path::new("/dev/null") {
                    return Ok(true);
                }
                let resolved = if target.is_absolute() {
                    target
                } else {
                    path.parent().unwrap_or_else(|| Path::new(".")).join(target)
                };
                return match fs::canonicalize(&resolved) {
                    Ok(resolved) => Ok(resolved == Path::new("/dev/null")),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
                    Err(error) => Err(format!(
                        "cannot resolve unit mask {}: {error}",
                        path.display()
                    )),
                };
            }
            Ok(_) => return Ok(false),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("cannot inspect {}: {error}", path.display()));
            }
        }
    }
    Ok(false)
}

fn template_unit_name(name: &str) -> Option<String> {
    let (stem, suffix) = name.rsplit_once('.')?;
    let at = stem.rfind('@')?;
    if at + 1 == stem.len() {
        return None;
    }
    Some(format!("{}@.{suffix}", &stem[..at]))
}

fn find_unit_path(directories: &[PathBuf], name: &str) -> Result<Option<PathBuf>, String> {
    for directory in directories.iter().rev() {
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {
                return Err(format!("unit path {} is a directory", path.display()));
            }
            Ok(_) => return Ok(Some(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
        }
    }
    Ok(None)
}

fn load_unit(
    directories: &[PathBuf],
    name: &str,
) -> Result<Option<fractald_config::UnitFile>, String> {
    if unit_path_is_masked(directories, name)? {
        return Ok(None);
    }
    let template = template_unit_name(name);
    let mut base_path = find_unit_path(directories, name)?;
    if base_path.is_none() {
        if let Some(template) = template.as_deref() {
            if unit_path_is_masked(directories, template)? {
                return Ok(None);
            }
            base_path = find_unit_path(directories, template)?;
        }
    }

    let mut unit = match base_path {
        Some(ref path) => {
            let source = fs::read_to_string(path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            fractald_config::UnitFile::parse(&source)
                .map_err(|error| format!("cannot parse {}: {error}", path.display()))?
        }
        None => fractald_config::UnitFile::default(),
    };
    let mut found_drop_in = false;
    let mut drop_in_names = Vec::with_capacity(2);
    if let Some(template) = template.as_deref() {
        drop_in_names.push(template);
    }
    drop_in_names.push(name);
    for directory in directories {
        for drop_in_name in &drop_in_names {
            let drop_in_directory = directory.join(format!("{drop_in_name}.d"));
            let entries = match fs::read_dir(&drop_in_directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot read drop-in directory {}: {error}",
                        drop_in_directory.display()
                    ));
                }
            };
            let mut paths = entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    format!("cannot enumerate {}: {error}", drop_in_directory.display())
                })?;
            paths.sort();
            for path in paths {
                if path.extension().and_then(|value| value.to_str()) != Some("conf") {
                    continue;
                }
                let source = fs::read_to_string(&path)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
                let overlay = fractald_config::UnitFile::parse(&source)
                    .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
                unit.merge(overlay);
                found_drop_in = true;
            }
        }
    }
    if base_path.is_none() && !found_drop_in {
        Ok(None)
    } else {
        Ok(Some(unit))
    }
}

fn load_openrc_specs() -> Result<Vec<ServiceSpec>, String> {
    let Some(directory) = openrc_directory() else {
        return Ok(Vec::new());
    };
    let state = StatePaths::from_environment();
    let strict = env::var_os("FRACTALD_OPENRC_DIR").is_some();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "cannot read OpenRC directory {}: {error}",
                directory.display()
            ));
        }
    };
    let mut paths = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("cannot enumerate {}: {error}", directory.display()))?;
    paths.sort();

    let mut specs = Vec::new();
    for path in paths {
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        if state
            .is_masked(name)
            .map_err(|error| format!("cannot inspect masked state for {name}: {error}"))?
        {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("cannot read OpenRC script {}: {error}", path.display()))?;
        match fractald_config::parse_openrc_script(&source, name, &path) {
            Ok(spec) => specs.push(spec),
            Err(error) if strict => {
                return Err(format!(
                    "cannot parse OpenRC script {}: {error}",
                    path.display()
                ));
            }
            Err(error) => {
                eprintln!(
                    "fractald: skipping incompatible OpenRC script {}: {error}",
                    path.display()
                );
            }
        }
    }
    Ok(specs)
}

fn openrc_directory() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_OPENRC_DIR") {
        return Some(PathBuf::from(path));
    }
    if env::var_os("FRACTALD_SERVICE_DIR").is_some() {
        return None;
    }
    if fractald_platform::is_root() {
        Some(PathBuf::from("/etc/init.d"))
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|path| path.join("fractald/init.d"))
    }
}

fn bind_listener(paths: &RuntimePaths) -> Result<UnixListener, String> {
    match UnixListener::bind(&paths.socket) {
        Ok(listener) => configure_listener(listener),
        Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
            match paths.connect() {
                Ok(_) => Err(format!(
                    "daemon is already running at {}",
                    paths.socket.display()
                )),
                Err(probe_error)
                    if matches!(
                        probe_error.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) =>
                {
                    remove_stale_socket(&paths.socket).map_err(|remove_error| {
                        format!("cannot remove stale control socket: {remove_error}")
                    })?;
                    configure_listener(UnixListener::bind(&paths.socket).map_err(|bind_error| {
                        format!("cannot bind control socket: {bind_error}")
                    })?)
                }
                Err(probe_error) => Err(format!("control socket is unavailable: {probe_error}")),
            }
        }
        Err(error) => Err(format!(
            "cannot bind control socket {}: {error}",
            paths.socket.display()
        )),
    }
}

fn configure_listener(listener: UnixListener) -> Result<UnixListener, String> {
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure control socket: {error}"))?;
    Ok(listener)
}

const DEFAULT_EVENT_LOG_EVENTS: usize = 4_096;
const MAX_EVENT_LOG_EVENTS: usize = 1_000_000;

struct EventJournal {
    path: PathBuf,
    next_sequence: u64,
    event_count: usize,
    max_events: usize,
    last: BTreeMap<String, ServiceSnapshot>,
}

impl EventJournal {
    fn open(state: &StatePaths) -> Result<Self, String> {
        state.ensure_directory().map_err(|error| {
            format!(
                "cannot create state directory {}: {error}",
                state.directory.display()
            )
        })?;
        let max_events = env::var("FRACTALD_EVENT_LOG_EVENTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .map(|value| value.clamp(1, MAX_EVENT_LOG_EVENTS))
            .unwrap_or(DEFAULT_EVENT_LOG_EVENTS);
        let mut next_sequence = 0;
        let event_count = match fs::read_to_string(&state.events) {
            Ok(contents) => {
                let mut count = 0;
                for line in contents.lines() {
                    count += 1;
                    if let Some(sequence) = event_sequence(line) {
                        next_sequence = next_sequence.max(sequence.saturating_add(1));
                    }
                }
                count
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
            Err(error) => {
                return Err(format!(
                    "cannot read event log {}: {error}",
                    state.events.display()
                ));
            }
        };
        let mut journal = Self {
            path: state.events.clone(),
            next_sequence,
            event_count,
            max_events,
            last: BTreeMap::new(),
        };
        journal.compact_if_needed()?;
        Ok(journal)
    }

    fn observe(&mut self, supervisor: &Supervisor) -> Result<Vec<String>, String> {
        let mut events = Vec::new();
        for snapshot in supervisor.snapshots() {
            let changed = self
                .last
                .get(&snapshot.name)
                .map_or(true, |previous| previous != &snapshot);
            if !changed {
                continue;
            }
            let sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            let line = format_event(sequence, &snapshot);
            self.append(&line)?;
            events.push(line);
            self.last.insert(snapshot.name.clone(), snapshot);
        }
        self.compact_if_needed()?;
        Ok(events)
    }

    fn events_since(&self, since: u64) -> Result<Vec<String>, String> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(format!(
                    "cannot read event log {}: {error}",
                    self.path.display()
                ));
            }
        };
        Ok(contents
            .lines()
            .filter(|line| event_sequence(line).is_some_and(|sequence| sequence >= since))
            .map(str::to_owned)
            .collect())
    }

    fn append(&mut self, line: &str) -> Result<(), String> {
        let needs_separator = match fs::read(&self.path) {
            Ok(contents) => !contents.is_empty() && !contents.ends_with(b"\n"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(format!(
                    "cannot inspect event log {}: {error}",
                    self.path.display()
                ));
            }
        };
        let mut file = OpenOptions::new();
        file.create(true).append(true).mode(0o600);
        let mut file = file
            .open(&self.path)
            .map_err(|error| format!("cannot open event log {}: {error}", self.path.display()))?;
        if needs_separator {
            file.write_all(b"\n").map_err(|error| {
                format!(
                    "cannot append event log separator {}: {error}",
                    self.path.display()
                )
            })?;
        }
        file.write_all(line.as_bytes())
            .map_err(|error| format!("cannot append event log {}: {error}", self.path.display()))?;
        file.sync_data()
            .map_err(|error| format!("cannot flush event log {}: {error}", self.path.display()))?;
        self.event_count += 1;
        Ok(())
    }

    fn compact_if_needed(&mut self) -> Result<(), String> {
        let contents = match fs::read_to_string(&self.path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                self.event_count = 0;
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "cannot read event log {}: {error}",
                    self.path.display()
                ));
            }
        };
        let lines = contents.lines().collect::<Vec<_>>();
        self.event_count = lines.len();
        let too_many_events = self.event_count > self.max_events;
        let too_many_bytes = fs::metadata(&self.path)
            .map(|metadata| metadata.len() > (self.max_events.saturating_mul(1_024)) as u64)
            .unwrap_or(false);
        if !too_many_events && !too_many_bytes {
            return Ok(());
        }

        let first = lines.len().saturating_sub(self.max_events);
        let retained = &lines[first..];
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        let file_name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("events.log");
        let mut temporary = None;
        for attempt in 0..100 {
            let temporary_path = parent.join(format!(
                ".{file_name}.tmp.{}.{}",
                std::process::id(),
                attempt
            ));
            let mut options = OpenOptions::new();
            options.create_new(true).write(true).mode(0o600);
            match options.open(&temporary_path) {
                Ok(file) => {
                    temporary = Some((temporary_path, file));
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!(
                        "cannot create event log compaction file {}: {error}",
                        temporary_path.display()
                    ));
                }
            }
        }
        let Some((temporary_path, mut temporary_file)) = temporary else {
            return Err("cannot allocate a unique event log compaction file".to_owned());
        };
        let result = (|| {
            for line in retained {
                temporary_file.write_all(line.as_bytes())?;
                temporary_file.write_all(b"\n")?;
            }
            temporary_file.sync_all()?;
            drop(temporary_file);
            fs::rename(&temporary_path, &self.path)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary_path);
            return Err(format!(
                "cannot compact event log {}: {error}",
                self.path.display()
            ));
        }
        self.event_count = retained.len();
        Ok(())
    }
}

fn format_event(sequence: u64, snapshot: &ServiceSnapshot) -> String {
    let pid = snapshot
        .pid
        .map_or_else(|| "-".to_owned(), |pid| pid.to_string());
    let exit = snapshot.last_exit.map_or_else(
        || "-".to_owned(),
        |reason| match reason {
            ExitReason::Exited(code) => format!("exit:{code}"),
            ExitReason::Signaled(signal) => format!("signal:{signal}"),
            ExitReason::CoreDumped(signal) => format!("core:{signal}"),
        },
    );
    format!(
        "seq={sequence} time_ms={} service={} state={} pid={pid} generation={} restarts={} exit={exit}\n",
        unix_timestamp_millis(),
        snapshot.name,
        service_state_name(snapshot.state),
        snapshot.generation,
        snapshot.restart_count,
    )
}

fn event_sequence(line: &str) -> Option<u64> {
    line.split_whitespace()
        .next()
        .and_then(|field| field.strip_prefix("seq="))
        .and_then(|value| value.parse().ok())
}

fn unix_timestamp_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransactionState {
    Pending,
    Done,
    Failed,
}

impl TransactionState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug)]
struct TransactionRecord {
    operation: String,
    name: String,
    state: TransactionState,
}

const MAX_TRANSACTION_RECORDS: usize = 4096;

struct EventSubscription {
    stream: UnixStream,
}

fn open_event_subscription(
    mut stream: UnixStream,
    journal: &EventJournal,
    since: Option<u64>,
) -> Result<Option<EventSubscription>, String> {
    let backlog = since
        .map(|since| journal.events_since(since))
        .transpose()?
        .unwrap_or_default();
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("cannot configure event subscription: {error}"))?;
    if Response::Subscribed.write_to(&mut stream).is_err() {
        return Ok(None);
    }
    for event in backlog {
        if (Response::Event { line: event })
            .write_to(&mut stream)
            .is_err()
        {
            return Ok(None);
        }
    }
    stream
        .set_nonblocking(true)
        .map_err(|error| format!("cannot configure event subscription: {error}"))?;
    Ok(Some(EventSubscription { stream }))
}

fn broadcast_events(subscriptions: &mut Vec<EventSubscription>, events: &[String]) {
    subscriptions.retain_mut(|subscription| {
        for event in events {
            if (Response::Event {
                line: event.clone(),
            })
            .write_to(&mut subscription.stream)
            .is_err()
            {
                return false;
            }
        }
        true
    });
}

fn daemon_loop(
    listener: &UnixListener,
    paths: &RuntimePaths,
    state: &StatePaths,
    pid: u32,
    started: Instant,
    supervisor: &mut Supervisor,
    journal: &mut EventJournal,
) -> Result<(), String> {
    let mut next_transaction_id = 1_u64;
    let mut transactions = BTreeMap::new();
    let mut subscriptions = Vec::new();
    loop {
        if pid == 1 {
            supervisor
                .reap_untracked_children()
                .map_err(|error| format!("PID1 child reaping failed: {error}"))?;
        }
        if fractald_platform::shutdown_requested() {
            supervisor
                .stop_all()
                .map_err(|error| format!("cannot begin signal-driven shutdown: {error}"))?;
            let events = journal.observe(supervisor)?;
            broadcast_events(&mut subscriptions, &events);
            return wait_for_shutdown(supervisor, journal);
        }
        supervisor
            .poll()
            .map_err(|error| format!("service supervision failed: {error}"))?;
        if let Some(action) = supervisor.take_manager_action() {
            if pid == 1 || action == ManagerAction::Exit {
                supervisor
                    .stop_all()
                    .map_err(|error| format!("cannot begin manager action shutdown: {error}"))?;
                let events = journal.observe(supervisor)?;
                broadcast_events(&mut subscriptions, &events);
                wait_for_shutdown(supervisor, journal)?;
                if pid == 1 {
                    perform_manager_action(action)?;
                }
                return Ok(());
            }
            eprintln!(
                "fractald: ignoring manager action {:?} outside the PID1 manager",
                action
            );
        }
        refresh_transactions(&mut transactions, supervisor);
        let events = journal.observe(supervisor)?;
        broadcast_events(&mut subscriptions, &events);
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let request = read_request(&mut stream);
                if let Ok(Request::Subscribe(since)) = &request {
                    if let Some(subscription) = open_event_subscription(stream, journal, *since)? {
                        subscriptions.push(subscription);
                    }
                    continue;
                }
                let requested_shutdown = match &request {
                    Ok(Request::Stop) => Some(ShutdownAction::Stop),
                    Ok(Request::Shutdown(action)) => Some(*action),
                    _ => None,
                };
                let mut shutdown = false;
                let response = match request {
                    Ok(Request::Status) => Response::Status {
                        pid,
                        uptime_ms: elapsed_millis(started),
                    },
                    Ok(Request::Ping) => Response::Pong,
                    Ok(Request::Stop) => match supervisor.stop_all() {
                        Ok(()) => {
                            shutdown = true;
                            Response::Stopping
                        }
                        Err(error) => Response::Error {
                            message: error.to_string(),
                        },
                    },
                    Ok(Request::Shutdown(action)) => {
                        if !matches!(action, ShutdownAction::Stop) && std::process::id() != 1 {
                            Response::Error {
                                message: format!(
                                    "{} requires FractalD to be running as PID 1",
                                    action.as_str()
                                ),
                            }
                        } else {
                            match supervisor.stop_all() {
                                Ok(()) => {
                                    shutdown = true;
                                    Response::Stopping
                                }
                                Err(error) => Response::Error {
                                    message: error.to_string(),
                                },
                            }
                        }
                    }
                    Ok(Request::List) => Response::Services {
                        names: supervisor.names().map(str::to_owned).collect(),
                    },
                    Ok(Request::EnableService(name)) => {
                        let name = canonical_service_name(supervisor, name);
                        match state.enable(&name) {
                            Ok(()) => Response::Enabled { name },
                            Err(error) => Response::Error {
                                message: format!("cannot enable service: {error}"),
                            },
                        }
                    }
                    Ok(Request::DisableService(name)) => {
                        let name = canonical_service_name(supervisor, name);
                        match state.disable(&name) {
                            Ok(()) => Response::Disabled { name },
                            Err(error) => Response::Error {
                                message: format!("cannot disable service: {error}"),
                            },
                        }
                    }
                    Ok(Request::IsEnabled(name)) => {
                        let name = canonical_service_name(supervisor, name);
                        match state.is_enabled(&name) {
                            Ok(enabled) => Response::EnabledStatus { name, enabled },
                            Err(error) => Response::Error {
                                message: format!("cannot inspect enabled state: {error}"),
                            },
                        }
                    }
                    Ok(Request::ServiceStatus(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name) {
                            Ok(name) => supervisor
                                .snapshot(&name)
                                .map(service_response)
                                .unwrap_or_else(|| Response::Error {
                                    message: format!("unknown service {name}"),
                                }),
                            Err(message) => Response::Error { message },
                        }
                    }
                    Ok(Request::TransactionStatus(id)) => transaction_response(&transactions, id),
                    Ok(Request::StartService(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name).and_then(|name| {
                            supervisor
                                .start(&name)
                                .map(|()| name)
                                .map_err(|error| error.to_string())
                        }) {
                            Ok(name) => accepted_transaction(
                                &mut transactions,
                                &mut next_transaction_id,
                                "start",
                                name,
                            ),
                            Err(error) => Response::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Ok(Request::IsolateService(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name).and_then(|name| {
                            supervisor
                                .isolate(&name)
                                .map(|()| name)
                                .map_err(|error| error.to_string())
                        }) {
                            Ok(name) => accepted_transaction(
                                &mut transactions,
                                &mut next_transaction_id,
                                "isolate",
                                name,
                            ),
                            Err(error) => Response::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Ok(Request::StopService(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name).and_then(|name| {
                            supervisor
                                .stop(&name)
                                .map(|()| name)
                                .map_err(|error| error.to_string())
                        }) {
                            Ok(name) => accepted_transaction(
                                &mut transactions,
                                &mut next_transaction_id,
                                "stop",
                                name,
                            ),
                            Err(error) => Response::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Ok(Request::RestartService(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name).and_then(|name| {
                            supervisor
                                .restart(&name)
                                .map(|()| name)
                                .map_err(|error| error.to_string())
                        }) {
                            Ok(name) => accepted_transaction(
                                &mut transactions,
                                &mut next_transaction_id,
                                "restart",
                                name,
                            ),
                            Err(error) => Response::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Ok(Request::ReloadService(name)) => {
                        match ensure_service_loaded(&mut *supervisor, &name).and_then(|name| {
                            supervisor
                                .reload_service(&name)
                                .map(|()| name)
                                .map_err(|error| error.to_string())
                        }) {
                            Ok(name) => accepted_transaction(
                                &mut transactions,
                                &mut next_transaction_id,
                                "reload",
                                name,
                            ),
                            Err(error) => Response::Error {
                                message: error.to_string(),
                            },
                        }
                    }
                    Ok(Request::ResetFailed(name)) => {
                        let result = match name {
                            Some(name) => {
                                let name = canonical_service_name(&supervisor, name);
                                supervisor.reset_failed(&name)
                            }
                            None => {
                                supervisor.reset_failed_all();
                                Ok(())
                            }
                        };
                        match result {
                            Ok(()) => Response::Reset,
                            Err(error) => Response::Error {
                                message: format!("cannot reset failed state: {error}"),
                            },
                        }
                    }
                    Ok(Request::Reload) => match load_all_service_specs()
                        .map_err(|error| error.to_string())
                        .and_then(|specs| {
                            supervisor.reload(specs).map_err(|error| error.to_string())
                        }) {
                        Ok(()) => Response::Reloaded,
                        Err(message) => Response::Error { message },
                    },
                    Ok(Request::Subscribe(_)) => Response::Error {
                        message: "event subscription could not be registered".to_owned(),
                    },
                    Err(error) => Response::Error { message: error },
                };
                response
                    .write_to(&mut stream)
                    .map_err(|error| format!("cannot write control response: {error}"))?;
                let _ = stream.shutdown(Shutdown::Write);
                if shutdown {
                    let events = journal.observe(supervisor)?;
                    broadcast_events(&mut subscriptions, &events);
                    wait_for_shutdown(supervisor, journal)?;
                    if let Some(action) = requested_shutdown {
                        perform_shutdown_action(action)?;
                    }
                    return Ok(());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(format!("control socket accept failed: {error}")),
        }
        if !paths.pid.exists() {
            return Err("pid file disappeared while daemon was running".to_owned());
        }
    }
}

fn canonical_service_name(supervisor: &Supervisor, name: String) -> String {
    supervisor
        .resolve_name(&name)
        .map(str::to_owned)
        .unwrap_or(name)
}

fn accepted_transaction(
    transactions: &mut BTreeMap<u64, TransactionRecord>,
    next: &mut u64,
    operation: &str,
    name: String,
) -> Response {
    let id = allocate_transaction_id(next, transactions);
    transactions.insert(
        id,
        TransactionRecord {
            operation: operation.to_owned(),
            name: name.clone(),
            state: TransactionState::Pending,
        },
    );
    prune_transactions(transactions);
    Response::Accepted {
        id,
        operation: operation.to_owned(),
        name,
    }
}

fn allocate_transaction_id(next: &mut u64, transactions: &BTreeMap<u64, TransactionRecord>) -> u64 {
    loop {
        let id = (*next).max(1);
        *next = next.wrapping_add(1).max(1);
        if !transactions.contains_key(&id) {
            return id;
        }
    }
}

fn prune_transactions(transactions: &mut BTreeMap<u64, TransactionRecord>) {
    while transactions.len() > MAX_TRANSACTION_RECORDS {
        let Some(id) = transactions
            .iter()
            .find(|(_, transaction)| transaction.state != TransactionState::Pending)
            .map(|(id, _)| *id)
        else {
            break;
        };
        transactions.remove(&id);
    }
}

fn refresh_transactions(
    transactions: &mut BTreeMap<u64, TransactionRecord>,
    supervisor: &Supervisor,
) {
    for transaction in transactions.values_mut() {
        if transaction.state != TransactionState::Pending {
            continue;
        }
        let Some(snapshot) = supervisor.snapshot(&transaction.name) else {
            transaction.state = TransactionState::Failed;
            continue;
        };
        transaction.state = match transaction.operation.as_str() {
            "start" | "restart" | "isolate" => match snapshot.state {
                ServiceState::Running
                | ServiceState::Active
                | ServiceState::Exited
                | ServiceState::Skipped => TransactionState::Done,
                ServiceState::Failed => TransactionState::Failed,
                ServiceState::Defined
                | ServiceState::Starting
                | ServiceState::Stopping
                | ServiceState::Backoff => TransactionState::Pending,
            },
            "stop" => match snapshot.state {
                ServiceState::Defined
                | ServiceState::Exited
                | ServiceState::Skipped
                | ServiceState::Failed => TransactionState::Done,
                ServiceState::Starting
                | ServiceState::Running
                | ServiceState::Active
                | ServiceState::Stopping
                | ServiceState::Backoff => TransactionState::Pending,
            },
            "reload" => match supervisor.reload_state(&transaction.name) {
                Some("pending") => TransactionState::Pending,
                Some("done") => TransactionState::Done,
                Some("failed") | None => TransactionState::Failed,
                Some(_) => TransactionState::Failed,
            },
            _ => TransactionState::Failed,
        };
    }
}

fn transaction_response(transactions: &BTreeMap<u64, TransactionRecord>, id: u64) -> Response {
    let (operation, name, state) = transactions
        .get(&id)
        .map(|transaction| {
            (
                transaction.operation.clone(),
                transaction.name.clone(),
                transaction.state.as_str().to_owned(),
            )
        })
        .unwrap_or_else(|| ("unknown".to_owned(), "-".to_owned(), "unknown".to_owned()));
    Response::Transaction(TransactionStatus {
        id,
        operation,
        name,
        state,
    })
}

fn service_response(snapshot: ServiceSnapshot) -> Response {
    Response::Service(fractald_control::ServiceStatus {
        name: snapshot.name,
        state: service_state_name(snapshot.state).to_owned(),
        pid: snapshot.pid,
        generation: snapshot.generation,
        restart_count: snapshot.restart_count,
    })
}

fn service_state_name(state: fractald_core::ServiceState) -> &'static str {
    match state {
        fractald_core::ServiceState::Defined => "defined",
        fractald_core::ServiceState::Starting => "starting",
        fractald_core::ServiceState::Running => "running",
        fractald_core::ServiceState::Active => "active",
        fractald_core::ServiceState::Stopping => "stopping",
        fractald_core::ServiceState::Backoff => "backoff",
        fractald_core::ServiceState::Exited => "exited",
        fractald_core::ServiceState::Skipped => "skipped",
        fractald_core::ServiceState::Failed => "failed",
    }
}

fn wait_for_shutdown(
    supervisor: &mut Supervisor,
    journal: &mut EventJournal,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !supervisor.is_stopped() {
        supervisor
            .poll()
            .map_err(|error| format!("service shutdown failed: {error}"))?;
        // A deliberate shutdown must not recursively trigger SuccessAction or
        // FailureAction while the manager is draining the service graph.
        let _ = supervisor.take_manager_action();
        if std::process::id() == 1 {
            supervisor
                .reap_untracked_children()
                .map_err(|error| format!("PID1 child reaping failed during shutdown: {error}"))?;
        }
        let _ = journal.observe(supervisor)?;
        if Instant::now() >= deadline {
            return Err("services did not stop within 10 seconds".to_owned());
        }
        thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

fn perform_shutdown_action(action: ShutdownAction) -> Result<(), String> {
    let action = match action {
        ShutdownAction::Stop => return Ok(()),
        ShutdownAction::Poweroff => fractald_platform::PowerAction::Poweroff,
        ShutdownAction::Reboot => fractald_platform::PowerAction::Reboot,
        ShutdownAction::Halt => fractald_platform::PowerAction::Halt,
    };
    fractald_platform::power_action(action)
        .map_err(|error| format!("cannot perform {}: {error}", action_name(action)))
}

fn perform_manager_action(action: ManagerAction) -> Result<(), String> {
    let action = match action {
        ManagerAction::None | ManagerAction::Exit => return Ok(()),
        ManagerAction::Poweroff | ManagerAction::PoweroffForce => {
            fractald_platform::PowerAction::Poweroff
        }
        ManagerAction::Reboot
        | ManagerAction::RebootForce
        | ManagerAction::Kexec
        | ManagerAction::KexecForce
        | ManagerAction::SoftReboot
        | ManagerAction::SoftRebootForce => fractald_platform::PowerAction::Reboot,
        ManagerAction::Halt | ManagerAction::HaltForce => fractald_platform::PowerAction::Halt,
    };
    fractald_platform::power_action(action)
        .map_err(|error| format!("cannot perform {}: {error}", action_name(action)))
}

fn action_name(action: fractald_platform::PowerAction) -> &'static str {
    match action {
        fractald_platform::PowerAction::Poweroff => "poweroff",
        fractald_platform::PowerAction::Reboot => "reboot",
        fractald_platform::PowerAction::Halt => "halt",
    }
}

fn read_request(stream: &mut UnixStream) -> Result<Request, String> {
    let mut input = String::new();
    let bytes_read = stream
        .take(4097)
        .read_to_string(&mut input)
        .map_err(|error| format!("cannot read control request: {error}"))?;
    if bytes_read > 4096 {
        return Err("control request is too large".to_owned());
    }
    Request::parse(&input).map_err(|error| error.to_string())
}

fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn run_command(mut args: impl Iterator<Item = OsString>) -> Result<u8, String> {
    let command = args
        .next()
        .ok_or_else(|| "run requires a command\n\n".to_owned() + &usage())?;
    let command_name = command.to_string_lossy().into_owned();
    supervise(&command_name, command, args)
}

fn supervise(
    name: &str,
    command: OsString,
    args: impl IntoIterator<Item = impl Into<OsString>>,
) -> Result<u8, String> {
    let mut spec = ServiceSpec::new(name, command.clone());
    spec.args = args.into_iter().map(Into::into).collect();
    let mut record =
        ServiceRecord::new(spec).map_err(|error| format!("invalid service: {error:?}"))?;
    record
        .transition(Event::StartRequested)
        .map_err(|error| format!("cannot start service: {error:?}"))?;

    let mut child_command = Command::new(&command);
    child_command
        .args(&record.spec().args)
        .envs(&record.spec().environment);
    let parent_pid = std::process::id();
    unsafe {
        child_command.pre_exec(move || {
            fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
            fractald_platform::set_process_group()
        });
    }
    let mut child = child_command
        .spawn()
        .map_err(|error| format!("cannot execute {}: {error}", command.to_string_lossy()))?;
    let pid = child.id();
    let generation = record.generation();
    record
        .transition(Event::Spawned { pid, generation })
        .map_err(|error| format!("cannot register child: {error:?}"))?;

    let exit = match PidFd::open(pid) {
        Ok(pidfd) => match pidfd.wait() {
            Ok(exit) => exit,
            Err(error) => {
                let _ = pidfd.send_signal(SIGKILL);
                let _ = child.wait();
                return Err(format!("cannot observe child {pid}: {error}"));
            }
        },
        Err(error) if pidfd_unavailable(&error) => wait_without_pidfd(&mut child)?,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("cannot open pidfd for {pid}: {error}"));
        }
    };
    let reason = match exit {
        ExitKind::Exited(code) => ExitReason::Exited(code),
        ExitKind::Signaled(signal) => ExitReason::Signaled(signal),
        ExitKind::CoreDumped(signal) => ExitReason::CoreDumped(signal),
    };
    record
        .transition(Event::Exited {
            pid,
            generation,
            reason,
        })
        .map_err(|error| format!("cannot commit child exit: {error:?}"))?;

    Ok(match reason {
        ExitReason::Exited(code) if (0..=255).contains(&code) => code as u8,
        ExitReason::Exited(_) => 1,
        ExitReason::Signaled(signal) | ExitReason::CoreDumped(signal) => {
            signal.saturating_add(128).min(255) as u8
        }
    })
}

fn pidfd_unavailable(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 38 | 95))
}

fn wait_without_pidfd(child: &mut Child) -> Result<ExitKind, String> {
    let status = child
        .wait()
        .map_err(|error| format!("cannot wait for child: {error}"))?;
    Ok(match status.code() {
        Some(code) => ExitKind::Exited(code),
        None => match status.signal() {
            Some(signal) => ExitKind::Signaled(signal),
            None => ExitKind::Exited(1),
        },
    })
}

fn usage() -> String {
    "usage: fractald <daemon|storage-prepare|self-check|chaos|inspect-unit|run|help|version>\n       fractald run <command> [args...]\n       fractald inspect-unit <path>\n       fractald (as PID 1)".to_owned()
}

fn print_help() {
    println!(
        "FractalD service supervisor\n\nCommands:\n  daemon                     run the control daemon\n  storage-prepare            activate discovered storage topology\n  self-check                 exercise supervisor and chaos invariants\n  chaos [list]               list the supported dynamics\n  chaos sample <system>      print a deterministic sample\n  inspect-unit <path>        validate and inspect a systemd service unit\n  run <command> [args...]    supervise one foreground command\n  help, --help               show this help\n  version, --version         show the version"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn recognizes_dev_null_unit_masks_at_the_highest_priority() {
        let root = std::env::temp_dir().join(format!(
            "fractald-unit-mask-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let low = root.join("low");
        let high = root.join("high");
        fs::create_dir_all(&low).expect("low directory");
        fs::create_dir_all(&high).expect("high directory");
        fs::write(low.join("demo.service"), "[Service]\nExecStart=/bin/true\n").expect("unit");
        symlink("/dev/null", high.join("demo.service")).expect("mask");

        assert!(
            unit_path_is_masked(&[low.clone(), high.clone()], "demo.service")
                .expect("inspect mask")
        );

        fs::remove_file(high.join("demo.service")).expect("remove mask");
        fs::write(
            high.join("demo.service"),
            "[Service]\nExecStart=/bin/true\n",
        )
        .expect("higher priority unit");
        assert!(!unit_path_is_masked(&[low, high], "demo.service").expect("inspect unit"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parses_native_storage_units_without_systemd_generators() {
        let entries = parse_fstab(
            "UUID=pool /srv/pool btrfs compress=zstd:1 0 0\n/dev/mapper/vg-data /srv/pool/data xfs nofail 0 0\n/dev/zram0 none swap pri=100 0 0\n",
        )
        .expect("fstab");
        for unit in generate_units(&entries).expect("storage units") {
            let parsed = fractald_config::UnitFile::parse(&unit.source).expect("unit syntax");
            parse_loaded_unit(parsed, &unit.name, &[]).expect("unit compatibility");
        }
    }

    #[test]
    fn deduplicates_device_dependencies_shared_by_ordering_edges() {
        let mut spec = ServiceSpec::new("data.mount", "/bin/true");
        spec.dependencies
            .requires
            .insert("dev-vda.device".to_owned());
        spec.dependencies.after.insert("dev-vda.device".to_owned());

        let dependencies = synthetic_device_dependencies(&spec, &BTreeSet::new());
        assert_eq!(dependencies, ["dev-vda.device".to_owned()].into());
    }

    #[test]
    fn preserves_explicit_crypttab_device_sources() {
        assert_eq!(
            resolve_crypttab_source("/dev/mapper/data").expect("device source"),
            Some("/dev/mapper/data".to_owned())
        );
        assert_eq!(
            resolve_crypttab_source("none").expect("none source"),
            Some("none".to_owned())
        );
    }

    #[test]
    fn loads_template_units_with_instance_drop_ins() {
        let root = std::env::temp_dir().join(format!(
            "fractald-template-unit-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("worker@0.service.d")).expect("drop-in directory");
        fs::write(
            root.join("worker@.service"),
            "[Service]\nExecStart=/bin/true\nEnvironment=BASE=template\n",
        )
        .expect("template unit");
        fs::write(
            root.join("worker@0.service.d/50-instance.conf"),
            "[Service]\nEnvironment=INSTANCE=zero\n",
        )
        .expect("instance drop-in");

        let unit = load_unit(&[root.clone()], "worker@0.service")
            .expect("load template instance")
            .expect("template unit exists");
        assert_eq!(unit.value("Service", "ExecStart"), Some("/bin/true"));
        assert_eq!(unit.value("Service", "Environment"), Some("INSTANCE=zero"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn discovers_slice_and_automount_units_for_boot_transactions() {
        let root = std::env::temp_dir().join(format!(
            "fractald-unit-kinds-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("slices.target.wants")).expect("relationship directory");
        fs::write(
            root.join("application.slice"),
            "[Unit]\nDescription=Application slice\n",
        )
        .expect("slice unit");
        fs::write(
            root.join("data.automount"),
            "[Automount]\nWhere=/srv/data\n",
        )
        .expect("automount unit");
        symlink(
            "../application.slice",
            root.join("slices.target.wants/application.slice"),
        )
        .expect("slice relationship");
        symlink(
            "../data.automount",
            root.join("slices.target.wants/data.automount"),
        )
        .expect("automount relationship");

        let names = discover_service_names(std::slice::from_ref(&root)).expect("unit names");
        assert!(names.contains("application.slice"));
        assert!(names.contains("data.automount"));
        assert!(is_discoverable_unit_name("application.slice"));
        assert!(is_discoverable_unit_name("data.automount"));
        assert!(is_discoverable_unit_name("dev-vda.device"));

        assert_eq!(
            device_unit_path(r"dev-disk-by\x2duuid-1234.device"),
            Some(PathBuf::from("/dev/disk/by-uuid/1234"))
        );
        assert_eq!(
            device_unit_path("sys-devices-virtual-block-vda.device"),
            Some(PathBuf::from("/sys/devices/virtual/block/vda"))
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn executes_a_generator_with_the_standard_output_arguments() {
        let root = std::env::temp_dir().join(format!(
            "fractald-generator-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let directory = root.join("generators");
        let output = root.join("generator");
        let early = root.join("generator.early");
        let late = root.join("generator.late");
        fs::create_dir_all(&directory).expect("generator directory");
        fs::create_dir_all(&output).expect("generator output");
        fs::create_dir_all(&early).expect("early generator output");
        fs::create_dir_all(&late).expect("late generator output");
        let generator = directory.join("demo-generator");
        fs::write(
            &generator,
            "#!/bin/sh\nprintf '[Service]\\nExecStart=/bin/true\\n' > \"$1/demo.service\"\nprintf early > \"$2/early.marker\"\nprintf late > \"$3/late.marker\"\n",
        )
        .expect("generator");
        let mut permissions = fs::metadata(&generator)
            .expect("generator metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&generator, permissions).expect("generator executable");

        run_generators(
            std::slice::from_ref(&directory),
            &[output.clone(), early.clone(), late.clone()],
            Duration::from_secs(2),
        )
        .expect("run generator");
        assert_eq!(
            fs::read_to_string(output.join("demo.service")).expect("generated unit"),
            "[Service]\nExecStart=/bin/true\n"
        );
        assert_eq!(
            fs::read_to_string(early.join("early.marker")).expect("early marker"),
            "early"
        );
        assert_eq!(
            fs::read_to_string(late.join("late.marker")).expect("late marker"),
            "late"
        );

        let _ = fs::remove_dir_all(root);
    }
}
