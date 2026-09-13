//! Native FractalD service configuration.
//!
//! A service description is a small INI-like file with a '.svc' suffix. The
//! format is deliberately owned by FractalD: it has no compatibility parser,
//! generator protocol, or script adapter. Package metadata is installed into
//! FractalD's service roots and is validated before it enters the supervisor.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fractald_core::{
    CommandSpec, Condition, CpuQuota, CredentialSource, CredentialSpec, DelegateMode,
    DeviceAccessRule, DevicePolicy, DirectoryKind, DirectorySpec, EnvironmentFileSpec, InputMode,
    LimitRange, LimitValue, ListenerKind, ListenerSpec, ManagerAction, NotifyAccess, OomPolicy,
    OutputMode, PathWatch, PrivateTmpMode, PrivateUsersMode, ProcSubsetMode, ProtectHomeMode,
    ProtectProcMode, ProtectSystemMode, RestartPolicy, ServiceSpec, ServiceType,
    SystemCallArchitectures, SystemCallFilter, SystemCallRule, SystemCallRuleAction, TriggerSpec,
};

mod syscall_groups;

const ALL_LINUX_CAPABILITIES: u64 = (1_u64 << 41) - 1;
const ALL_LINUX_ADDRESS_FAMILIES: u64 = (1_u64 << 45) - 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub section: String,
    pub key: String,
    pub value: String,
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NativeFile {
    sections: BTreeMap<String, Vec<Directive>>,
}

impl NativeFile {
    pub fn parse(source: &str) -> Result<Self, ConfigError> {
        let mut file = Self::default();
        let mut section: Option<String> = None;
        let mut logical = String::new();
        let mut logical_line = 0;

        for (index, physical) in source.lines().enumerate() {
            let line_number = index + 1;
            let physical = physical.strip_suffix('\r').unwrap_or(physical);
            if logical.is_empty() {
                logical_line = line_number;
            }
            if has_continuation(physical) {
                logical.push_str(physical[..physical.len() - 1].trim_end());
                logical.push(' ');
                continue;
            }
            logical.push_str(physical);

            let line = logical.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                logical.clear();
                continue;
            }
            if line.starts_with('[') {
                if !line.ends_with(']') {
                    return Err(ConfigError::at(
                        logical_line,
                        "section header is missing its closing bracket",
                    ));
                }
                let name = line[1..line.len() - 1].trim();
                if name.is_empty()
                    || name.contains(['[', ']'])
                    || !name
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    return Err(ConfigError::at(logical_line, "invalid section name"));
                }
                let name = name.to_ascii_lowercase();
                section = Some(name.clone());
                file.sections.entry(name).or_default();
            } else {
                let section_name = section.as_deref().ok_or_else(|| {
                    ConfigError::at(logical_line, "directive appears before a section header")
                })?;
                let (key, value) = line
                    .split_once('=')
                    .ok_or_else(|| ConfigError::at(logical_line, "directive is missing '='"))?;
                let key = key.trim();
                if key.is_empty()
                    || !key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                {
                    return Err(ConfigError::at(logical_line, "invalid directive name"));
                }
                file.sections
                    .entry(section_name.to_owned())
                    .or_default()
                    .push(Directive {
                        section: section_name.to_owned(),
                        key: key.to_ascii_lowercase(),
                        value: value.trim().to_owned(),
                        line: logical_line,
                    });
            }
            logical.clear();
        }
        if !logical.is_empty() {
            return Err(ConfigError::at(
                logical_line,
                "unterminated line continuation",
            ));
        }
        validate_schema(&file)?;
        Ok(file)
    }

    pub fn directives(&self, section: &str, key: &str) -> Vec<&Directive> {
        let section = section.to_ascii_lowercase();
        let key = key.to_ascii_lowercase();
        self.sections
            .get(&section)
            .into_iter()
            .flat_map(|directives| directives.iter())
            .filter(|directive| directive.key == key)
            .collect()
    }

    pub fn effective_directives(&self, section: &str, key: &str) -> Vec<&Directive> {
        let mut effective = Vec::new();
        for directive in self.directives(section, key) {
            if directive.value.is_empty() && is_reset_key(section, key) {
                effective.clear();
            } else {
                effective.push(directive);
            }
        }
        effective
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.contains_key(&section.to_ascii_lowercase())
    }

    pub fn merge(&mut self, overlay: Self) {
        for (section, directives) in overlay.sections {
            self.sections.entry(section).or_default().extend(directives);
        }
    }

    pub fn value(&self, section: &str, key: &str) -> Option<&str> {
        self.effective_directives(section, key)
            .last()
            .filter(|directive| !directive.value.is_empty())
            .map(|directive| directive.value.as_str())
    }

    pub fn to_service_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, ConfigError> {
        parse_spec(self, name.into())
    }
}

fn validate_schema(file: &NativeFile) -> Result<(), ConfigError> {
    let allowed = |section: &str| -> Option<&'static [&'static str]> {
        Some(match section {
            "service" => &[
                "description",
                "kind",
                "exec",
                "exec_pre",
                "exec_post",
                "exec_stop_post",
                "exec_condition",
                "stop",
                "reload",
                "restart",
                "restart_delay",
                "start_timeout",
                "stop_timeout",
                "runtime_limit",
                "kill_signal",
                "restart_kill_signal",
                "send_sighup",
                "ignore_sigpipe",
                "remain_after_exit",
                "refuse_manual_start",
                "refuse_manual_stop",
                "stop_when_unneeded",
                "default_dependencies",
                "allow_isolate",
                "ignore_on_isolate",
                "working_directory",
                "pid_file",
                "user",
                "group",
                "dynamic_user",
                "supplementary_groups",
                "cgroup_slice",
                "environment",
                "environment_file",
                "unset_environment",
                "stdout",
                "stderr",
                "stdin",
                "tty",
                "watchdog",
                "notify_access",
                "success_exit_status",
                "restart_prevent_exit_status",
                "bus_name",
            ],
            "dependencies" => &[
                "requires",
                "wants",
                "after",
                "before",
                "conflicts",
                "part_of",
                "binds_to",
                "requisite",
                "on_success",
                "on_failure",
                "requires_mount",
                "wants_mount",
                "default_dependencies",
                "allow_isolate",
                "ignore_on_isolate",
                "stop_when_unneeded",
            ],
            "install" => &[
                "alias",
                "profile",
                "profiles",
                "default_dependencies",
                "allow_isolate",
                "ignore_on_isolate",
                "stop_when_unneeded",
            ],
            "paths" => &[
                "configuration",
                "configuration_mode",
                "configuration_preserve",
                "runtime",
                "runtime_mode",
                "runtime_preserve",
                "state",
                "state_mode",
                "state_preserve",
                "cache",
                "cache_mode",
                "cache_preserve",
                "logs",
                "logs_mode",
                "logs_preserve",
            ],
            "credentials" => &["file", "value", "store", "import"],
            "conditions" | "assertions" => &[
                "path_exists",
                "path_exists_glob",
                "directory_not_empty",
                "executable",
                "read_write",
                "directory",
                "file_not_empty",
                "mount_point",
                "symlink",
                "kernel_command_line",
                "virtualization",
                "security",
                "ac_power",
                "capability",
                "kernel_module",
                "firmware",
                "first_boot",
                "credential",
                "cgroup_controller",
                "environment",
                "needs_update",
            ],
            "security" => &[
                "no_new_privileges",
                "memory_deny_write_execute",
                "restrict_realtime",
                "restrict_suid_sgid",
                "protect_control_groups",
                "protect_kernel_modules",
                "protect_kernel_tunables",
                "protect_kernel_logs",
                "protect_clock",
                "protect_hostname",
                "lock_personality",
                "umask",
                "nice",
                "oom_score_adjust",
                "oom_policy",
                "private_tmp",
                "private_devices",
                "private_mounts",
                "private_ipc",
                "private_network",
                "device_policy",
                "device_allow",
                "delegate",
                "private_users",
                "restrict_namespaces",
                "protect_system",
                "protect_home",
                "protect_proc",
                "proc_subset",
                "read_write",
                "read_only",
                "inaccessible",
                "capability_bounding_set",
                "ambient_capabilities",
                "address_families",
                "syscall_architectures",
                "syscall_error_number",
                "syscall_filter",
            ],
            "resources" => &[
                "memory_max",
                "memory_high",
                "memory_min",
                "memory_low",
                "memory_swap_max",
                "tasks_max",
                "cpu_weight",
                "io_weight",
                "cpu_quota",
                "cpu_quota_period",
                "nofile",
                "memlock",
                "nproc",
                "start_limit_interval",
                "start_limit_burst",
            ],
            "manager" => &[
                "failure_action",
                "success_action",
                "job_timeout",
                "job_timeout_action",
            ],
            "listen" => &[
                "accept",
                "service",
                "fd_name",
                "remove_on_stop",
                "mode",
                "user",
                "group",
                "stream",
                "datagram",
                "sequential_packet",
                "fifo",
                "netlink",
                "special",
            ],
            "timer" => &[
                "service",
                "on_boot",
                "on_active",
                "on_inactive",
                "calendar",
                "persistent",
                "random_delay",
                "accuracy",
            ],
            "watch" => &[
                "service",
                "changed",
                "modified",
                "exists",
                "exists_glob",
                "directory_not_empty",
            ],
            "mount" => &[
                "what",
                "where",
                "type",
                "options",
                "directory_mode",
                "lazy",
                "force",
                "timeout",
            ],
            "swap" => &["what", "options", "priority", "timeout"],
            "device" => &["path"],
            _ => return None,
        })
    };
    for (section, directives) in &file.sections {
        let allowed = allowed(section).ok_or_else(|| {
            ConfigError::message(format!("unsupported native section [{section}]"))
        })?;
        for directive in directives {
            if !allowed.contains(&directive.key.as_str()) {
                return Err(ConfigError::at(
                    directive.line,
                    format!("unsupported native directive {}.{}", section, directive.key),
                ));
            }
        }
    }
    Ok(())
}

pub fn parse_service(source: &str, name: impl Into<String>) -> Result<ServiceSpec, ConfigError> {
    NativeFile::parse(source)?.to_service_spec(name)
}

pub fn parse_service_file(path: &Path) -> Result<ServiceSpec, ConfigError> {
    let source = fs::read_to_string(path).map_err(|error| {
        ConfigError::message(format!("cannot read {}: {error}", path.display()))
    })?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ConfigError::message(format!("invalid service file name: {}", path.display()))
        })?;
    if !file_name.ends_with(".svc") {
        return Err(ConfigError::message(format!(
            "service file does not use the .svc suffix: {}",
            path.display()
        )));
    }
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| {
            ConfigError::message(format!("invalid service file name: {}", path.display()))
        })?;
    parse_service(&source, name)
}

fn parse_spec(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    validate_name(&name)?;
    if !file.has_section("service") {
        return Err(ConfigError::message(
            "service file has no [service] section",
        ));
    }
    let kind = file
        .value("service", "kind")
        .unwrap_or("service")
        .to_ascii_lowercase();
    let service_type = match kind.as_str() {
        "service" | "simple" | "exec" => ServiceType::Simple,
        "forking" => ServiceType::Forking,
        "oneshot" => ServiceType::Oneshot,
        "notify" => ServiceType::Notify,
        "dbus" => ServiceType::Dbus,
        "idle" => ServiceType::Idle,
        "group" => ServiceType::Oneshot,
        "listener" => ServiceType::Socket,
        "timer" => ServiceType::Timer,
        "watch" => ServiceType::Path,
        "mount" => ServiceType::Mount,
        "swap" => ServiceType::Swap,
        "device" => ServiceType::Oneshot,
        other => {
            return Err(ConfigError::message(format!(
                "unsupported service kind {other}"
            )));
        }
    };

    let mut spec = match kind.as_str() {
        "group" => {
            let mut spec = ServiceSpec::new(name.clone(), "/bin/true");
            spec.service_type = ServiceType::Oneshot;
            spec.remain_after_exit = true;
            spec
        }
        "listener" => parse_listener(file, name.clone())?,
        "timer" => parse_timer(file, name.clone())?,
        "watch" => parse_watch(file, name.clone())?,
        "mount" => parse_mount(file, name.clone())?,
        "swap" => parse_swap(file, name.clone())?,
        "device" => parse_device(file, name.clone())?,
        _ => parse_process(file, name.clone(), service_type)?,
    };
    parse_common(file, &mut spec)?;
    if kind == "group" {
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
    }
    spec.validate().map_err(|error| {
        ConfigError::message(format!("invalid service specification: {error:?}"))
    })?;
    Ok(spec)
}

fn parse_process(
    file: &NativeFile,
    name: String,
    service_type: ServiceType,
) -> Result<ServiceSpec, ConfigError> {
    let commands = file.effective_directives("service", "exec");
    if commands.len() > 1 && service_type != ServiceType::Oneshot {
        return Err(ConfigError::at(
            commands[1].line,
            "multiple exec directives require kind=oneshot",
        ));
    }
    let start = if let Some(directive) = commands.last() {
        parse_command(&directive.value, directive.line)?
    } else if service_type == ServiceType::Oneshot {
        CommandSpec::new("/bin/true")
    } else {
        return Err(ConfigError::message("service has no exec directive"));
    };
    let mut spec = ServiceSpec::new(name, start.program);
    spec.args = start.args;
    spec.main_ignore_failure = start.ignore_failure;
    spec.main_argv0 = start.argv0;
    spec.main_expand_environment = start.expand_environment;
    spec.service_type = service_type;
    if service_type == ServiceType::Dbus {
        let names = list_values(file, "service", "bus_name")?;
        if names.is_empty() {
            return Err(ConfigError::message("kind=dbus requires bus_name"));
        }
        spec.bus_names = names;
    }
    if service_type == ServiceType::Oneshot && commands.len() > 1 {
        for directive in commands.iter().take(commands.len() - 1) {
            spec.start_pre
                .push(parse_command(&directive.value, directive.line)?);
        }
    }
    Ok(spec)
}

fn parse_common(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for (key, destination) in [
        ("requires", &mut spec.dependencies.requires),
        ("wants", &mut spec.dependencies.wants),
        ("after", &mut spec.dependencies.after),
        ("before", &mut spec.dependencies.before),
        ("conflicts", &mut spec.dependencies.conflicts),
        ("part_of", &mut spec.dependencies.part_of),
        ("binds_to", &mut spec.dependencies.binds_to),
        ("requisite", &mut spec.dependencies.requisite),
        ("on_success", &mut spec.dependencies.on_success),
        ("on_failure", &mut spec.dependencies.on_failure),
    ] {
        for value in list_values(file, "dependencies", key)? {
            validate_name(&value)?;
            destination.insert(value);
        }
    }
    for (key, destination) in [
        ("requires_mount", &mut spec.requires_mounts_for),
        ("wants_mount", &mut spec.wants_mounts_for),
    ] {
        for value in list_values(file, "dependencies", key)? {
            if !value.starts_with('/') && !value.starts_with('%') {
                return Err(ConfigError::message(format!(
                    "{key} path is not absolute: {value}"
                )));
            }
            destination.push(PathBuf::from(value));
        }
    }
    for (key, destination) in [
        ("exec_pre", &mut spec.start_pre),
        ("exec_post", &mut spec.start_post),
        ("exec_stop_post", &mut spec.stop_post),
        ("exec_condition", &mut spec.exec_conditions),
    ] {
        for directive in file.effective_directives("service", key) {
            if !directive.value.is_empty() {
                destination.push(parse_command(&directive.value, directive.line)?);
            }
        }
    }
    if let Some(value) = file.value("service", "stop") {
        spec.stop = Some(parse_command(value, line_of(file, "service", "stop"))?);
    }
    if let Some(value) = file.value("service", "reload") {
        spec.reload = Some(parse_command(value, line_of(file, "service", "reload"))?);
    }
    if let Some(value) = file.value("service", "restart") {
        spec.restart = parse_restart(value)?;
    }
    if let Some(value) = file.value("service", "restart_delay") {
        spec.restart_backoff = parse_duration(value, "restart_delay")?;
    }
    if let Some(value) = file.value("service", "start_timeout") {
        spec.start_timeout = parse_duration(value, "start_timeout")?;
    }
    if let Some(value) = file.value("service", "stop_timeout") {
        spec.stop_timeout = parse_duration(value, "stop_timeout")?;
    }
    if let Some(value) = file.value("service", "runtime_limit") {
        let duration = parse_duration(value, "runtime_limit")?;
        spec.runtime_max = (!duration.is_zero()).then_some(duration);
    }
    if let Some(value) = file.value("service", "kill_signal") {
        spec.kill_signal = parse_signal(value)?;
    }
    if let Some(value) = file.value("service", "restart_kill_signal") {
        spec.restart_kill_signal = Some(parse_signal(value)?);
    }
    for (key, destination) in [
        ("send_sighup", &mut spec.send_sighup),
        ("ignore_sigpipe", &mut spec.ignore_sigpipe),
        ("remain_after_exit", &mut spec.remain_after_exit),
        ("refuse_manual_start", &mut spec.refuse_manual_start),
        ("refuse_manual_stop", &mut spec.refuse_manual_stop),
        ("stop_when_unneeded", &mut spec.stop_when_unneeded),
        ("default_dependencies", &mut spec.default_dependencies),
        ("allow_isolate", &mut spec.allow_isolate),
        ("ignore_on_isolate", &mut spec.ignore_on_isolate),
    ] {
        if let Some(value) = bool_value(file, "service", key)?
            .or(bool_value(file, "dependencies", key)?)
            .or(bool_value(file, "install", key)?)
        {
            *destination = value;
        }
    }
    if let Some(value) = file.value("service", "working_directory") {
        spec.working_directory = Some(PathBuf::from(value));
    }
    if let Some(value) = file.value("service", "pid_file") {
        spec.pid_file = Some(PathBuf::from(value));
    }
    if let Some(value) = file.value("service", "user") {
        spec.user = Some(value.to_owned());
    }
    if let Some(value) = file.value("service", "group") {
        spec.group = Some(value.to_owned());
    }
    if let Some(value) = bool_value(file, "service", "dynamic_user")? {
        spec.dynamic_user = value;
    }
    if let Some(groups) = list_values_optional(file, "service", "supplementary_groups")? {
        spec.supplementary_groups = Some(groups);
    }
    if let Some(value) = file.value("service", "cgroup_slice") {
        spec.cgroup_slice = Some(value.to_owned());
    }
    parse_environment(file, spec)?;
    parse_directories(file, spec)?;
    parse_credentials(file, spec)?;
    parse_conditions(file, spec)?;
    parse_security(file, spec)?;
    parse_resources(file, spec)?;
    parse_manager(file, spec)?;
    parse_limits(file, spec)?;
    if let Some(value) = file.value("service", "stdout") {
        spec.stdout = parse_output_mode(value, "stdout")?;
    }
    if let Some(value) = file.value("service", "stderr") {
        spec.stderr = parse_output_mode(value, "stderr")?;
    }
    if let Some(value) = file.value("service", "stdin") {
        spec.standard_input = parse_input_mode(value, "stdin")?;
    }
    if let Some(value) = file.value("service", "tty") {
        spec.tty_path = Some(PathBuf::from(value));
    }
    if let Some(value) = file.value("service", "watchdog") {
        spec.watchdog = Some(parse_duration(value, "watchdog")?);
    }
    if let Some(value) = file.value("service", "notify_access") {
        spec.notify_access = parse_notify_access(value)?;
    }
    for (key, destination) in [
        ("success_exit_status", &mut spec.success_exit_status),
        (
            "restart_prevent_exit_status",
            &mut spec.restart_prevent_exit_status,
        ),
    ] {
        for value in list_values(file, "service", key)? {
            destination.insert(parse_exit_status(&value, key)?);
        }
    }
    if let Some(value) = file.value("manager", "job_timeout") {
        let timeout = parse_duration(value, "job_timeout")?;
        spec.job_timeout = (!timeout.is_zero()).then_some(timeout);
    }
    if let Some(value) = file.value("manager", "job_timeout_action") {
        spec.job_timeout_action = parse_manager_action(value, "job_timeout_action")?;
    }
    parse_start_limit(file, spec)?;
    for directive in file.effective_directives("install", "alias") {
        for alias in split_words(&directive.value, directive.line)? {
            validate_name(&alias)?;
            spec.aliases.insert(alias);
        }
    }
    for key in ["profile", "profiles"] {
        for profile in list_values(file, "install", key)? {
            validate_name(&profile)?;
            spec.profiles.insert(profile);
        }
    }
    Ok(())
}

fn parse_listener(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let accept = bool_value(file, "listen", "accept")?.unwrap_or(false);
    let mut spec = ServiceSpec::new(name.clone(), "/bin/true");
    spec.service_type = ServiceType::Socket;
    spec.remain_after_exit = true;
    spec.socket_accept = accept;
    spec.socket_service = file
        .value("listen", "service")
        .map(str::to_owned)
        .or_else(|| {
            Some(if accept {
                format!("{}@", name)
            } else {
                name.clone()
            })
        });
    spec.file_descriptor_name = file.value("listen", "fd_name").map(str::to_owned);
    spec.remove_on_stop = bool_value(file, "listen", "remove_on_stop")?.unwrap_or(false);
    if let Some(value) = file.value("listen", "mode") {
        spec.socket_mode = parse_mode(value, "mode")?;
    }
    spec.socket_user = file.value("listen", "user").map(str::to_owned);
    spec.socket_group = file.value("listen", "group").map(str::to_owned);
    for (key, kind) in [
        ("stream", ListenerKind::Stream),
        ("datagram", ListenerKind::Datagram),
        ("sequential_packet", ListenerKind::SequentialPacket),
        ("fifo", ListenerKind::Fifo),
        ("netlink", ListenerKind::Netlink),
        ("special", ListenerKind::Special),
    ] {
        for directive in file.effective_directives("listen", key) {
            let values = if kind == ListenerKind::Netlink {
                vec![directive.value.clone()]
            } else {
                split_words(&directive.value, directive.line)?
            };
            for value in values {
                if value.is_empty() {
                    return Err(ConfigError::at(directive.line, "listener address is empty"));
                }
                spec.listeners.push(ListenerSpec {
                    kind,
                    address: if kind == ListenerKind::Netlink {
                        value
                    } else {
                        normalize_listener_address(&value)
                    },
                });
            }
        }
    }
    if spec.listeners.is_empty() {
        return Err(ConfigError::message("listener has no address"));
    }
    if accept
        && (spec.listeners.len() != 1
            || !matches!(
                spec.listeners[0].kind,
                ListenerKind::Stream | ListenerKind::SequentialPacket
            ))
    {
        return Err(ConfigError::message(
            "accept=true requires exactly one stream listener",
        ));
    }
    Ok(spec)
}

fn parse_timer(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let mut spec = ServiceSpec::new(name, "/bin/true");
    spec.service_type = ServiceType::Timer;
    spec.remain_after_exit = true;
    let service = file
        .value("timer", "service")
        .map(str::to_owned)
        .ok_or_else(|| ConfigError::message("timer has no service target"))?;
    let on_boot = duration_value(file, "timer", "on_boot")?;
    let on_unit_active = duration_value(file, "timer", "on_active")?;
    let on_unit_inactive = duration_value(file, "timer", "on_inactive")?;
    let on_calendar = values_as_strings(file, "timer", "calendar");
    if on_boot.is_none()
        && on_unit_active.is_none()
        && on_unit_inactive.is_none()
        && on_calendar.is_empty()
    {
        return Err(ConfigError::message("timer has no schedule"));
    }
    spec.trigger = Some(TriggerSpec::Timer {
        service,
        on_boot,
        on_unit_active,
        on_unit_inactive,
        on_calendar,
        persistent: bool_value(file, "timer", "persistent")?.unwrap_or(false),
        randomized_delay: duration_value(file, "timer", "random_delay")?,
        accuracy: duration_value(file, "timer", "accuracy")?,
    });
    Ok(spec)
}

fn parse_watch(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let mut spec = ServiceSpec::new(name, "/bin/true");
    spec.service_type = ServiceType::Path;
    spec.remain_after_exit = true;
    let service = file
        .value("watch", "service")
        .map(str::to_owned)
        .ok_or_else(|| ConfigError::message("watch has no service target"))?;
    let mut watches = Vec::new();
    for (key, kind) in [
        ("changed", 0_u8),
        ("modified", 1),
        ("exists", 2),
        ("exists_glob", 3),
        ("directory_not_empty", 4),
    ] {
        for directive in file.effective_directives("watch", key) {
            for value in split_words(&directive.value, directive.line)? {
                let path = PathBuf::from(value);
                watches.push(match kind {
                    0 => PathWatch::Changed(path),
                    1 => PathWatch::Modified(path),
                    2 => PathWatch::Exists(path),
                    3 => PathWatch::ExistsGlob(path),
                    _ => PathWatch::DirectoryNotEmpty(path),
                });
            }
        }
    }
    if watches.is_empty() {
        return Err(ConfigError::message("watch has no path"));
    }
    spec.trigger = Some(TriggerSpec::Path { service, watches });
    Ok(spec)
}

fn parse_mount(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let what = required(file, "mount", "what")?;
    let where_path = PathBuf::from(required(file, "mount", "where")?);
    if !where_path.is_absolute() {
        return Err(ConfigError::message("mount.where must be absolute"));
    }
    let filesystem = file.value("mount", "type").map(canonical_filesystem_type);
    let mut command = CommandSpec::new("mount");
    if let Some(value) = filesystem.as_deref() {
        command
            .args
            .extend([OsString::from("-t"), OsString::from(value)]);
    }
    if let Some(value) = file.value("mount", "options") {
        command
            .args
            .extend([OsString::from("-o"), OsString::from(value)]);
    }
    command.args.extend([
        OsString::from("--"),
        OsString::from(what.clone()),
        where_path.as_os_str().to_owned(),
    ]);
    command.ignore_failure = file
        .value("mount", "options")
        .is_some_and(|value| value.split(',').any(|option| option == "nofail"));
    let mut spec = ServiceSpec::new(name, command.program);
    spec.args = command.args;
    spec.main_ignore_failure = command.ignore_failure;
    spec.service_type = ServiceType::Mount;
    spec.mount_where = Some(where_path.clone());
    spec.mount_filesystem = filesystem;
    spec.remain_after_exit = true;
    let mode = file
        .value("mount", "directory_mode")
        .map(|value| parse_mode(value, "directory_mode"))
        .transpose()?
        .unwrap_or(0o755);
    let mut mkdir = CommandSpec::new("mkdir");
    mkdir.args = vec![
        OsString::from("-p"),
        OsString::from("-m"),
        OsString::from(format!("{mode:o}")),
        OsString::from("--"),
        where_path.as_os_str().to_owned(),
    ];
    spec.start_pre.push(mkdir);
    let mut unmount = CommandSpec::new("umount");
    if bool_value(file, "mount", "lazy")?.unwrap_or(false) {
        unmount.args.push(OsString::from("--lazy"));
    }
    if bool_value(file, "mount", "force")?.unwrap_or(false) {
        unmount.args.push(OsString::from("--force"));
    }
    unmount
        .args
        .extend([OsString::from("--"), where_path.as_os_str().to_owned()]);
    spec.stop = Some(unmount);
    if let Some(value) = file.value("mount", "timeout") {
        let timeout = parse_duration(value, "timeout")?;
        spec.start_timeout = timeout;
        spec.stop_timeout = timeout;
    }
    Ok(spec)
}

fn parse_swap(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let what = required(file, "swap", "what")?;
    let mut command = CommandSpec::new("swapon");
    if let Some(value) = file.value("swap", "priority") {
        value
            .parse::<i32>()
            .map_err(|_| ConfigError::message(format!("invalid swap.priority={value}")))?;
        command
            .args
            .extend([OsString::from("--priority"), OsString::from(value)]);
    }
    if let Some(value) = file.value("swap", "options") {
        command
            .args
            .extend([OsString::from("-o"), OsString::from(value)]);
    }
    command
        .args
        .extend([OsString::from("--"), OsString::from(what.clone())]);
    command.ignore_failure = file
        .value("swap", "options")
        .is_some_and(|value| value.split(',').any(|option| option == "nofail"));
    let mut spec = ServiceSpec::new(name, command.program);
    spec.args = command.args;
    spec.main_ignore_failure = command.ignore_failure;
    spec.service_type = ServiceType::Swap;
    spec.remain_after_exit = true;
    let mut stop = CommandSpec::new("swapoff");
    stop.args = vec![OsString::from("--"), OsString::from(what)];
    spec.stop = Some(stop);
    if let Some(value) = file.value("swap", "timeout") {
        let timeout = parse_duration(value, "timeout")?;
        spec.start_timeout = timeout;
        spec.stop_timeout = timeout;
    }
    Ok(spec)
}

fn parse_device(file: &NativeFile, name: String) -> Result<ServiceSpec, ConfigError> {
    let path = PathBuf::from(required(file, "device", "path")?);
    if !path.is_absolute() {
        return Err(ConfigError::message("device.path must be absolute"));
    }
    let mut spec = ServiceSpec::new(name, "/bin/sh");
    spec.args = vec![
        OsString::from("-c"),
        OsString::from("while [ ! -e \"$1\" ]; do sleep 1; done"),
        OsString::from("fractald-device-wait"),
        path.as_os_str().to_owned(),
    ];
    spec.main_expand_environment = false;
    spec.service_type = ServiceType::Oneshot;
    spec.remain_after_exit = true;
    spec.default_dependencies = false;
    spec.start_timeout = Duration::MAX;
    spec.device_path = Some(path);
    Ok(spec)
}

fn parse_environment(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for directive in file.effective_directives("service", "environment") {
        for entry in split_words(&directive.value, directive.line)? {
            let (name, value) = entry.split_once('=').ok_or_else(|| {
                ConfigError::at(directive.line, "environment entries must use NAME=VALUE")
            })?;
            if !valid_environment_name(name) {
                return Err(ConfigError::at(
                    directive.line,
                    format!("invalid environment name {name}"),
                ));
            }
            spec.environment
                .insert(OsString::from(name), OsString::from(value));
        }
    }
    for value in list_values(file, "service", "unset_environment")? {
        if !valid_environment_name(&value) {
            return Err(ConfigError::message(format!(
                "invalid environment name {value}"
            )));
        }
        spec.unset_environment.insert(OsString::from(value));
    }
    for directive in file.effective_directives("service", "environment_file") {
        for value in split_words(&directive.value, directive.line)? {
            let optional = value.starts_with('-');
            let path = value.strip_prefix('-').unwrap_or(&value);
            if !path.starts_with('/') {
                return Err(ConfigError::at(
                    directive.line,
                    "environment_file path must be absolute",
                ));
            }
            spec.environment_files.push(EnvironmentFileSpec {
                path: PathBuf::from(path),
                optional,
            });
        }
    }
    Ok(())
}

fn parse_directories(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for (key, kind) in [
        ("configuration", DirectoryKind::Configuration),
        ("runtime", DirectoryKind::Runtime),
        ("state", DirectoryKind::State),
        ("cache", DirectoryKind::Cache),
        ("logs", DirectoryKind::Logs),
    ] {
        let mode_key = format!("{key}_mode");
        let preserve_key = format!("{key}_preserve");
        let mode = file
            .value("paths", &mode_key)
            .map(|value| parse_mode(value, &mode_key))
            .transpose()?
            .unwrap_or(0o755);
        let preserve = file
            .value("paths", &preserve_key)
            .map(|value| parse_bool(value, &preserve_key))
            .transpose()?
            .unwrap_or(false);
        for directive in file.effective_directives("paths", key) {
            for value in split_words(&directive.value, directive.line)? {
                let path = PathBuf::from(value);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(ConfigError::at(
                        directive.line,
                        format!("paths.{key} must be relative and cannot contain '..'"),
                    ));
                }
                spec.directories.push(DirectorySpec {
                    kind,
                    path,
                    mode,
                    preserve,
                });
            }
        }
    }
    Ok(())
}

fn parse_credentials(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for directive in file.effective_directives("credentials", "file") {
        let (name, path) = directive.value.split_once(':').ok_or_else(|| {
            ConfigError::at(
                directive.line,
                "credentials.file entries must use NAME:PATH",
            )
        })?;
        validate_credential_name(name, directive.line)?;
        if path.is_empty() {
            return Err(ConfigError::at(directive.line, "credential path is empty"));
        }
        spec.credentials.push(CredentialSpec {
            name: name.to_owned(),
            source: CredentialSource::File(PathBuf::from(path)),
        });
    }
    for directive in file.effective_directives("credentials", "value") {
        let (name, value) = directive.value.split_once(':').ok_or_else(|| {
            ConfigError::at(
                directive.line,
                "credentials.value entries must use NAME:VALUE",
            )
        })?;
        validate_credential_name(name, directive.line)?;
        spec.credentials.push(CredentialSpec {
            name: name.to_owned(),
            source: CredentialSource::Value(value.as_bytes().to_vec()),
        });
    }
    for directive in file.effective_directives("credentials", "store") {
        let (name, value) = directive.value.split_once(':').map_or_else(
            || (directive.value.as_str(), directive.value.as_str()),
            |(name, value)| (name, value),
        );
        validate_credential_name(name, directive.line)?;
        spec.credentials.push(CredentialSpec {
            name: name.to_owned(),
            source: CredentialSource::Store(value.to_owned()),
        });
    }
    for directive in file.effective_directives("credentials", "import") {
        let (pattern, rename) = directive
            .value
            .split_once(':')
            .map_or((directive.value.as_str(), None), |(pattern, rename)| {
                (pattern, Some(rename.to_owned()))
            });
        if pattern.is_empty() || !validate_credential_name_template(pattern) {
            return Err(ConfigError::at(
                directive.line,
                "credential import pattern is invalid",
            ));
        }
        spec.credential_imports
            .push(fractald_core::CredentialImportSpec {
                pattern: pattern.to_owned(),
                rename,
            });
    }
    Ok(())
}

fn parse_conditions(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    parse_condition_section(file, "conditions", &mut spec.conditions)?;
    parse_condition_section(file, "assertions", &mut spec.assertions)
}

fn parse_condition_section(
    file: &NativeFile,
    section: &str,
    destination: &mut Vec<Condition>,
) -> Result<(), ConfigError> {
    for (key, kind) in [
        ("path_exists", 0_u8),
        ("path_exists_glob", 1),
        ("directory_not_empty", 2),
        ("executable", 3),
        ("read_write", 4),
        ("directory", 5),
        ("file_not_empty", 6),
        ("mount_point", 7),
        ("symlink", 8),
        ("kernel_command_line", 9),
        ("virtualization", 10),
        ("security", 11),
        ("ac_power", 12),
        ("capability", 13),
        ("kernel_module", 14),
        ("firmware", 15),
        ("first_boot", 16),
        ("credential", 17),
        ("cgroup_controller", 18),
        ("environment", 19),
        ("needs_update", 20),
    ] {
        for directive in file.effective_directives(section, key) {
            for raw in split_words(&directive.value, directive.line)? {
                let (alternative, negate, value) = condition_prefix(&raw);
                if value.is_empty() {
                    return Err(ConfigError::at(
                        directive.line,
                        format!("{section}.{key} is empty"),
                    ));
                }
                let condition = match kind {
                    0 => Condition::PathExists {
                        path: PathBuf::from(value),
                        negate,
                    },
                    1 => Condition::PathExistsGlob {
                        pattern: PathBuf::from(value),
                        negate,
                    },
                    2 => Condition::DirectoryNotEmpty {
                        path: PathBuf::from(value),
                        negate,
                    },
                    3 => Condition::FileIsExecutable {
                        path: PathBuf::from(value),
                        negate,
                    },
                    4 => Condition::PathIsReadWrite {
                        path: PathBuf::from(value),
                        negate,
                    },
                    5 => Condition::PathIsDirectory {
                        path: PathBuf::from(value),
                        negate,
                    },
                    6 => Condition::FileNotEmpty {
                        path: PathBuf::from(value),
                        negate,
                    },
                    7 => Condition::PathIsMountPoint {
                        path: PathBuf::from(value),
                        negate,
                    },
                    8 => Condition::PathIsSymbolicLink {
                        path: PathBuf::from(value),
                        negate,
                    },
                    9 => Condition::KernelCommandLine {
                        argument: value.to_owned(),
                        negate,
                    },
                    10 => Condition::Virtualization {
                        value: value.to_owned(),
                        negate,
                    },
                    11 => Condition::Security {
                        value: value.to_owned(),
                        negate,
                    },
                    12 => Condition::ACPower {
                        on_ac_power: parse_bool(value, key)?,
                        negate,
                    },
                    13 => Condition::Capability {
                        capability: value.to_owned(),
                        negate,
                    },
                    14 => Condition::KernelModuleLoaded {
                        module: value.to_owned(),
                        negate,
                    },
                    15 => Condition::Firmware {
                        value: value.to_owned(),
                        negate,
                    },
                    16 => Condition::FirstBoot {
                        first_boot: parse_bool(value, key)?,
                        negate,
                    },
                    17 => Condition::Credential {
                        credential: value.to_owned(),
                        negate,
                    },
                    18 => Condition::ControlGroupController {
                        controller: value.to_owned(),
                        negate,
                    },
                    19 => {
                        let (name, expected) = parse_environment_condition(value, key)?;
                        Condition::Environment {
                            name,
                            value: expected,
                            negate,
                        }
                    }
                    _ => Condition::NeedsUpdate {
                        path: parse_needs_update_path(value, key)?,
                        negate,
                    },
                };
                push_condition(destination, condition, alternative);
            }
        }
    }
    Ok(())
}

fn parse_security(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for (key, destination) in [
        ("no_new_privileges", &mut spec.no_new_privileges),
        (
            "memory_deny_write_execute",
            &mut spec.memory_deny_write_execute,
        ),
        ("restrict_realtime", &mut spec.restrict_realtime),
        ("restrict_suid_sgid", &mut spec.restrict_suid_sgid),
        ("protect_control_groups", &mut spec.protect_control_groups),
        ("protect_kernel_modules", &mut spec.protect_kernel_modules),
        ("protect_kernel_tunables", &mut spec.protect_kernel_tunables),
        ("protect_kernel_logs", &mut spec.protect_kernel_logs),
        ("protect_clock", &mut spec.protect_clock),
        ("protect_hostname", &mut spec.protect_hostname),
        ("lock_personality", &mut spec.lock_personality),
    ] {
        if let Some(value) = bool_value(file, "security", key)? {
            *destination = value;
        }
    }
    if let Some(value) = file.value("security", "umask") {
        spec.umask = Some(parse_mode(value, "umask")?);
    }
    if let Some(value) = file.value("security", "nice") {
        let value = value
            .parse::<i32>()
            .map_err(|_| ConfigError::message(format!("invalid nice={value}")))?;
        if !(-20..=19).contains(&value) {
            return Err(ConfigError::message(format!("invalid nice={value}")));
        }
        spec.nice = Some(value);
    }
    if let Some(value) = file.value("security", "oom_score_adjust") {
        let value = value
            .parse::<i32>()
            .map_err(|_| ConfigError::message(format!("invalid oom_score_adjust={value}")))?;
        if !(-1000..=1000).contains(&value) {
            return Err(ConfigError::message(format!(
                "invalid oom_score_adjust={value}"
            )));
        }
        spec.oom_score_adjust = Some(value);
    }
    if let Some(value) = file.value("security", "oom_policy") {
        spec.oom_policy = match value.to_ascii_lowercase().as_str() {
            "continue" => OomPolicy::Continue,
            "stop" => OomPolicy::Stop,
            "kill" => OomPolicy::Kill,
            _ => return Err(ConfigError::message(format!("invalid oom_policy={value}"))),
        };
    }
    if let Some(value) = file.value("security", "private_tmp") {
        spec.private_tmp = parse_private_tmp(value)?;
    }
    for (key, destination) in [
        ("private_devices", &mut spec.private_devices),
        ("private_mounts", &mut spec.private_mounts),
        ("private_ipc", &mut spec.private_ipc),
        ("private_network", &mut spec.private_network),
    ] {
        if let Some(value) = bool_value(file, "security", key)? {
            *destination = value;
        }
    }
    if let Some(value) = file.value("security", "device_policy") {
        spec.device_policy = parse_device_policy(value)?;
    }
    for directive in file.effective_directives("security", "device_allow") {
        let mut values = split_words(&directive.value, directive.line)?;
        let access = values
            .pop()
            .ok_or_else(|| ConfigError::at(directive.line, "device_allow has no access mode"))?;
        let device = values.join(" ");
        if device.is_empty() {
            return Err(ConfigError::at(
                directive.line,
                "device_allow has no device",
            ));
        }
        spec.device_allow.push(DeviceAccessRule {
            device,
            access: parse_device_access(&access, directive.line)?,
        });
    }
    if let Some(value) = file.value("security", "delegate") {
        spec.delegate = parse_delegate(value)?;
    }
    if let Some(value) = file.value("security", "private_users") {
        spec.private_users = parse_private_users(value)?;
    }
    if let Some(value) = file.value("security", "restrict_namespaces") {
        spec.restrict_namespaces = Some(parse_namespace_list(value)?);
    }
    if let Some(value) = file.value("security", "protect_system") {
        spec.protect_system = parse_protect_system(value)?;
    }
    if let Some(value) = file.value("security", "protect_home") {
        spec.protect_home = parse_protect_home(value)?;
    }
    if let Some(value) = file.value("security", "protect_proc") {
        spec.protect_proc = parse_protect_proc(value)?;
    }
    if let Some(value) = file.value("security", "proc_subset") {
        spec.proc_subset = parse_proc_subset(value)?;
    }
    spec.read_write_paths = paths(file, "security", "read_write")?;
    spec.read_only_paths = paths(file, "security", "read_only")?;
    spec.inaccessible_paths = paths(file, "security", "inaccessible")?;
    if let Some(value) = file.value("security", "capability_bounding_set") {
        spec.capability_bounding_set = Some(parse_capabilities(value)?);
    }
    if let Some(value) = file.value("security", "ambient_capabilities") {
        spec.ambient_capabilities = Some(parse_capabilities(value)?);
    }
    if let Some(value) = file.value("security", "address_families") {
        spec.restrict_address_families = Some(parse_address_families(value)?);
    }
    if let Some(value) = file.value("security", "syscall_architectures") {
        spec.system_call_architectures = Some(parse_architectures(value)?);
    }
    if let Some(value) = file.value("security", "syscall_error_number") {
        spec.system_call_error_number = Some(parse_errno(value, "syscall_error_number")?);
    }
    spec.system_call_filter = parse_syscall_filter(file)?;
    Ok(())
}

fn parse_resources(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    let resources = &mut spec.resources;
    resources.memory_max = limit_value(file, "memory_max", true)?;
    resources.memory_high = limit_value(file, "memory_high", true)?;
    resources.memory_min = limit_value(file, "memory_min", true)?;
    resources.memory_low = limit_value(file, "memory_low", true)?;
    resources.memory_swap_max = limit_value(file, "memory_swap_max", true)?;
    resources.tasks_max = limit_value(file, "tasks_max", false)?;
    resources.cpu_weight = number_value(file, "cpu_weight")?;
    resources.io_weight = number_value(file, "io_weight")?;
    let period = duration_value(file, "resources", "cpu_quota_period")?
        .unwrap_or(Duration::from_millis(100));
    if let Some(value) = file.value("resources", "cpu_quota") {
        resources.cpu_quota = parse_cpu_quota(value, period)?;
    }
    Ok(())
}

fn parse_manager(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    if let Some(value) = file.value("manager", "failure_action") {
        spec.failure_action = parse_manager_action(value, "failure_action")?;
    }
    if let Some(value) = file.value("manager", "success_action") {
        spec.success_action = parse_manager_action(value, "success_action")?;
    }
    if let Some(value) = file.value("manager", "job_timeout") {
        let timeout = parse_duration(value, "job_timeout")?;
        spec.job_timeout = (!timeout.is_zero()).then_some(timeout);
    }
    if let Some(value) = file.value("manager", "job_timeout_action") {
        spec.job_timeout_action = parse_manager_action(value, "job_timeout_action")?;
    }
    Ok(())
}

fn parse_limits(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    for (key, destination) in [
        ("nofile", &mut spec.nofile),
        ("memlock", &mut spec.memlock),
        ("nproc", &mut spec.nproc),
    ] {
        if let Some(value) = file.value("resources", key) {
            *destination = Some(parse_limit_range(value, key, key == "memlock")?);
        }
    }
    Ok(())
}

fn parse_start_limit(file: &NativeFile, spec: &mut ServiceSpec) -> Result<(), ConfigError> {
    spec.start_limit_interval = duration_value(file, "resources", "start_limit_interval")?;
    spec.start_limit_burst = number_value(file, "start_limit_burst")?
        .map(|value| {
            u32::try_from(value).map_err(|_| ConfigError::message("start_limit_burst is too large"))
        })
        .transpose()?;
    Ok(())
}

fn parse_syscall_filter(file: &NativeFile) -> Result<Option<SystemCallFilter>, ConfigError> {
    let directives = file.effective_directives("security", "syscall_filter");
    if directives.is_empty() {
        return Ok(None);
    }
    let mut default_allow = true;
    let mut rules = BTreeMap::new();
    for directive in directives {
        let mut words = split_words(&directive.value, directive.line)?;
        if words.is_empty() {
            return Err(ConfigError::at(directive.line, "syscall_filter is empty"));
        }
        let deny = words[0].strip_prefix('!').is_some();
        if deny {
            words[0].remove(0);
            default_allow = true;
        } else {
            default_allow = false;
        }
        for word in words {
            let (name, errno) = if let Some((name, errno)) = word.split_once(':') {
                (name, Some(parse_errno(errno, "syscall_filter")?))
            } else {
                (word.as_str(), None)
            };
            if name.is_empty() {
                return Err(ConfigError::at(
                    directive.line,
                    "syscall_filter has an empty name",
                ));
            }
            let names: Vec<String> = syscall_groups::expand_group(name).map_or_else(
                || vec![name.to_owned()],
                |values| values.iter().map(|value| (*value).to_owned()).collect(),
            );
            let action = if deny {
                errno.map_or(
                    SystemCallRuleAction::Deny,
                    SystemCallRuleAction::DenyWithErrno,
                )
            } else {
                SystemCallRuleAction::Allow
            };
            for name in names {
                rules.insert(name, action);
            }
        }
    }
    Ok(Some(SystemCallFilter {
        default_allow,
        rules: rules
            .into_iter()
            .map(|(name, action)| SystemCallRule { name, action })
            .collect(),
    }))
}

fn required(file: &NativeFile, section: &str, key: &str) -> Result<String, ConfigError> {
    file.value(section, key)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| ConfigError::message(format!("[{section}] has no {key}")))
}

fn line_of(file: &NativeFile, section: &str, key: &str) -> usize {
    file.directives(section, key)
        .last()
        .map_or(0, |directive| directive.line)
}

fn list_values(file: &NativeFile, section: &str, key: &str) -> Result<Vec<String>, ConfigError> {
    let mut values = Vec::new();
    for directive in file.effective_directives(section, key) {
        values.extend(split_words(&directive.value, directive.line)?);
    }
    Ok(values)
}

fn list_values_optional(
    file: &NativeFile,
    section: &str,
    key: &str,
) -> Result<Option<Vec<String>>, ConfigError> {
    let directives = file.effective_directives(section, key);
    if directives.is_empty() {
        return Ok(None);
    }
    Ok(Some(list_values(file, section, key)?))
}

fn values_as_strings(file: &NativeFile, section: &str, key: &str) -> Vec<String> {
    file.effective_directives(section, key)
        .iter()
        .map(|directive| directive.value.clone())
        .filter(|value| !value.is_empty())
        .collect()
}

fn bool_value(file: &NativeFile, section: &str, key: &str) -> Result<Option<bool>, ConfigError> {
    file.value(section, key)
        .map(|value| parse_bool(value, key))
        .transpose()
}

fn duration_value(
    file: &NativeFile,
    section: &str,
    key: &str,
) -> Result<Option<Duration>, ConfigError> {
    file.value(section, key)
        .map(|value| parse_duration(value, key))
        .transpose()
}

fn number_value(file: &NativeFile, key: &str) -> Result<Option<u64>, ConfigError> {
    file.value("resources", key)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| ConfigError::message(format!("invalid {key}={value}")))
        })
        .transpose()
}

fn limit_value(
    file: &NativeFile,
    key: &str,
    bytes: bool,
) -> Result<Option<LimitValue>, ConfigError> {
    file.value("resources", key)
        .map(|value| parse_limit(value, key, bytes))
        .transpose()
}

fn paths(file: &NativeFile, section: &str, key: &str) -> Result<Vec<PathBuf>, ConfigError> {
    let values = list_values(file, section, key)?;
    for value in &values {
        let path = Path::new(value);
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(ConfigError::message(format!(
                "{section}.{key} path is unsafe: {value}"
            )));
        }
    }
    Ok(values.into_iter().map(PathBuf::from).collect())
}

fn validate_name(value: &str) -> Result<(), ConfigError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ConfigError::message(format!(
            "invalid service name {value:?}"
        )));
    }
    Ok(())
}

fn validate_credential_name(value: &str, line: usize) -> Result<(), ConfigError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value == ".fractald-owner"
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(ConfigError::at(
            line,
            format!("invalid credential name {value}"),
        ));
    }
    Ok(())
}

fn parse_command(value: &str, line: usize) -> Result<CommandSpec, ConfigError> {
    let words = split_words(value, line)?;
    let program = words
        .first()
        .ok_or_else(|| ConfigError::at(line, "command is empty"))?;
    let mut command = CommandSpec::new(program);
    command.args = words.into_iter().skip(1).map(OsString::from).collect();
    Ok(command)
}

fn split_words(value: &str, line: usize) -> Result<Vec<String>, ConfigError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        if escaped {
            if character == 'x' {
                let high = characters.next().and_then(hex_value);
                let low = characters.next().and_then(hex_value);
                match (high, low) {
                    (Some(high), Some(low)) => current.push(char::from((high << 4) | low)),
                    _ => return Err(ConfigError::at(line, "invalid hexadecimal escape")),
                }
            } else {
                current.push(match character {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    other => other,
                });
            }
            escaped = false;
            started = true;
            continue;
        }
        if character == '\\' {
            escaped = true;
            started = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            started = true;
            continue;
        }
        match character {
            '\'' | '"' => {
                quote = Some(character);
                started = true;
            }
            character if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if escaped {
        return Err(ConfigError::at(line, "value ends with an escape"));
    }
    if quote.is_some() {
        return Err(ConfigError::at(line, "value has an unterminated quote"));
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

fn condition_prefix(value: &str) -> (bool, bool, &str) {
    let mut value = value;
    let mut alternative = false;
    let mut negate = false;
    loop {
        if let Some(rest) = value.strip_prefix('|') {
            alternative = true;
            value = rest;
        } else if let Some(rest) = value.strip_prefix('!') {
            negate = !negate;
            value = rest;
        } else {
            return (alternative, negate, value);
        }
    }
}

fn push_condition(destination: &mut Vec<Condition>, condition: Condition, alternative: bool) {
    if !alternative {
        destination.push(condition);
        return;
    }
    match destination.pop() {
        Some(Condition::Any(mut values)) => {
            values.push(condition);
            destination.push(Condition::Any(values));
        }
        Some(previous) => destination.push(Condition::Any(vec![previous, condition])),
        None => destination.push(condition),
    }
}

fn parse_bool(value: &str, key: &str) -> Result<bool, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" | "true" | "on" | "1" => Ok(true),
        "n" | "no" | "false" | "off" | "0" => Ok(false),
        other => Err(ConfigError::message(format!("invalid {key}={other}"))),
    }
}

fn parse_restart(value: &str) -> Result<RestartPolicy, ConfigError> {
    let value = value.trim().to_ascii_lowercase().replace('-', "_");
    match value.as_str() {
        "never" | "no" => Ok(RestartPolicy::Never),
        "on_success" => Ok(RestartPolicy::OnSuccess),
        "on_failure" => Ok(RestartPolicy::OnFailure),
        "on_abnormal" => Ok(RestartPolicy::OnAbnormal),
        "on_abort" => Ok(RestartPolicy::OnAbort),
        "always" => Ok(RestartPolicy::Always),
        other => Err(ConfigError::message(format!("invalid restart={other}"))),
    }
}

fn parse_mode(value: &str, key: &str) -> Result<u32, ConfigError> {
    let value = value.trim().strip_prefix("0o").unwrap_or(value.trim());
    let mode = u32::from_str_radix(value, 8)
        .map_err(|_| ConfigError::message(format!("invalid {key}={value}")))?;
    if mode > 0o7777 {
        return Err(ConfigError::message(format!("invalid {key}={value}")));
    }
    Ok(mode)
}

fn parse_duration(value: &str, key: &str) -> Result<Duration, ConfigError> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("infinity") {
        return Ok(Duration::MAX);
    }
    let suffixes = [
        ("us", 1e-6),
        ("ms", 1e-3),
        ("s", 1.0),
        ("m", 60.0),
        ("h", 3_600.0),
        ("d", 86_400.0),
    ];
    let (number, multiplier) = suffixes
        .iter()
        .find_map(|(suffix, multiplier)| {
            value
                .strip_suffix(suffix)
                .map(|number| (number, *multiplier))
        })
        .unwrap_or((value, 1.0));
    let number = number
        .parse::<f64>()
        .map_err(|_| ConfigError::message(format!("invalid {key}={value}")))?;
    let nanos = number * multiplier * 1_000_000_000.0;
    if !nanos.is_finite() || nanos < 0.0 || nanos > u64::MAX as f64 {
        return Err(ConfigError::message(format!("invalid {key}={value}")));
    }
    Ok(Duration::from_nanos(nanos.round() as u64))
}

fn parse_limit(value: &str, key: &str, bytes: bool) -> Result<LimitValue, ConfigError> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("max") || value.eq_ignore_ascii_case("infinity") {
        return Ok(LimitValue::Max);
    }
    let suffixes = [
        ("KiB", 1_u64 << 10),
        ("MiB", 1_u64 << 20),
        ("GiB", 1_u64 << 30),
        ("TiB", 1_u64 << 40),
        ("KB", 1_000),
        ("MB", 1_000_000),
        ("GB", 1_000_000_000),
        ("TB", 1_000_000_000_000),
    ];
    let (number, multiplier) = if bytes {
        suffixes
            .iter()
            .find_map(|(suffix, multiplier)| {
                value
                    .strip_suffix(suffix)
                    .map(|number| (number, *multiplier))
            })
            .unwrap_or((value, 1))
    } else {
        (value, 1)
    };
    let number = number
        .parse::<u64>()
        .map_err(|_| ConfigError::message(format!("invalid {key}={value}")))?;
    number
        .checked_mul(multiplier)
        .map(LimitValue::Value)
        .ok_or_else(|| ConfigError::message(format!("invalid {key}={value}")))
}

fn parse_limit_range(value: &str, key: &str, bytes: bool) -> Result<LimitRange, ConfigError> {
    let (soft, hard) = value.split_once(':').map_or((value, value), |pair| pair);
    let soft = parse_limit(soft, key, bytes)?;
    let hard = parse_limit(hard, key, bytes)?;
    let invalid = match (soft, hard) {
        (LimitValue::Max, LimitValue::Value(_)) => true,
        (LimitValue::Value(left), LimitValue::Value(right)) => left > right,
        _ => false,
    };
    if invalid {
        return Err(ConfigError::message(format!("invalid {key}={value}")));
    }
    Ok(LimitRange { soft, hard })
}

fn parse_cpu_quota(value: &str, period: Duration) -> Result<Option<CpuQuota>, ConfigError> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("max") {
        return Ok(None);
    }
    let percent = value
        .strip_suffix('%')
        .ok_or_else(|| ConfigError::message(format!("cpu_quota must be a percentage: {value}")))?
        .parse::<f64>()
        .map_err(|_| ConfigError::message(format!("invalid cpu_quota={value}")))?;
    if !percent.is_finite() || percent <= 0.0 {
        return Ok(None);
    }
    let period_usec = u64::try_from(period.as_micros())
        .map_err(|_| ConfigError::message("cpu_quota period is too large"))?;
    let quota_usec = (period.as_secs_f64() * 1_000_000.0 * percent / 100.0).ceil() as u64;
    Ok(Some(CpuQuota {
        quota_usec,
        period_usec,
    }))
}

fn parse_signal(value: &str) -> Result<i32, ConfigError> {
    let value = value.trim();
    let normalized = value.strip_prefix("SIG").unwrap_or(value);
    let signal = match normalized.to_ascii_uppercase().as_str() {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "ILL" => 4,
        "TRAP" => 5,
        "ABRT" => 6,
        "BUS" => 7,
        "FPE" => 8,
        "KILL" => 9,
        "USR1" => 10,
        "SEGV" => 11,
        "USR2" => 12,
        "PIPE" => 13,
        "ALRM" => 14,
        "TERM" => 15,
        "CHLD" => 17,
        "CONT" => 18,
        "STOP" => 19,
        "TSTP" => 20,
        "TTIN" => 21,
        "TTOU" => 22,
        "URG" => 23,
        "XCPU" => 24,
        "XFSZ" => 25,
        "VTALRM" => 26,
        "PROF" => 27,
        "WINCH" => 28,
        "IO" => 29,
        "PWR" => 30,
        "SYS" => 31,
        _ => value
            .parse::<i32>()
            .map_err(|_| ConfigError::message(format!("invalid signal={value}")))?,
    };
    if signal <= 0 {
        return Err(ConfigError::message(format!("invalid signal={value}")));
    }
    Ok(signal)
}

fn parse_exit_status(value: &str, key: &str) -> Result<i32, ConfigError> {
    if value
        .chars()
        .all(|character| character.is_ascii_alphabetic() || character == '_')
    {
        return parse_signal(value).or_else(|_| {
            let value = value.strip_prefix("EX_").unwrap_or(value);
            let status = match value {
                "OK" => Some(0),
                "USAGE" => Some(64),
                "DATAERR" => Some(65),
                "NOINPUT" => Some(66),
                "NOUSER" => Some(67),
                "NOHOST" => Some(68),
                "UNAVAILABLE" => Some(69),
                "SOFTWARE" => Some(70),
                "OSERR" => Some(71),
                "OSFILE" => Some(72),
                "CANTCREAT" => Some(73),
                "IOERR" => Some(74),
                "TEMPFAIL" => Some(75),
                "PROTOCOL" => Some(76),
                "NOPERM" => Some(77),
                "CONFIG" => Some(78),
                _ => None,
            };
            status.ok_or_else(|| ConfigError::message(format!("invalid {key}={value}")))
        });
    }
    let status = value
        .parse::<i32>()
        .map_err(|_| ConfigError::message(format!("invalid {key}={value}")))?;
    if !(0..=255).contains(&status) {
        return Err(ConfigError::message(format!("invalid {key}={value}")));
    }
    Ok(status)
}

fn parse_output_mode(value: &str, key: &str) -> Result<OutputMode, ConfigError> {
    match value {
        "journal" => Ok(OutputMode::Journal),
        "null" => Ok(OutputMode::Null),
        "inherit" => Ok(OutputMode::Inherit),
        "tty" => Ok(OutputMode::Tty),
        "socket" => Ok(OutputMode::Socket),
        value if value.starts_with("file:") || value.starts_with("append:") => {
            let append = value.starts_with("append:");
            let path = value.split_once(':').map_or("", |(_, path)| path);
            if !path.starts_with('/') {
                return Err(ConfigError::message(format!("invalid {key}={value}")));
            }
            Ok(OutputMode::File {
                path: PathBuf::from(path),
                append,
            })
        }
        other => Err(ConfigError::message(format!("invalid {key}={other}"))),
    }
}

fn parse_input_mode(value: &str, key: &str) -> Result<InputMode, ConfigError> {
    match value {
        "null" => Ok(InputMode::Null),
        "inherit" => Ok(InputMode::Inherit),
        "tty" => Ok(InputMode::Tty),
        "socket" => Ok(InputMode::Socket),
        value if value.starts_with("file:") => {
            let path = value.strip_prefix("file:").unwrap_or_default();
            if !path.starts_with('/') {
                return Err(ConfigError::message(format!("invalid {key}={value}")));
            }
            Ok(InputMode::File(PathBuf::from(path)))
        }
        other => Err(ConfigError::message(format!("invalid {key}={other}"))),
    }
}

fn parse_manager_action(value: &str, key: &str) -> Result<ManagerAction, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(ManagerAction::None),
        "exit" => Ok(ManagerAction::Exit),
        "halt" => Ok(ManagerAction::Halt),
        "halt_force" => Ok(ManagerAction::HaltForce),
        "poweroff" => Ok(ManagerAction::Poweroff),
        "poweroff_force" => Ok(ManagerAction::PoweroffForce),
        "reboot" => Ok(ManagerAction::Reboot),
        "reboot_force" => Ok(ManagerAction::RebootForce),
        "kexec" => Ok(ManagerAction::Kexec),
        "kexec_force" => Ok(ManagerAction::KexecForce),
        "soft_reboot" => Ok(ManagerAction::SoftReboot),
        "soft_reboot_force" => Ok(ManagerAction::SoftRebootForce),
        other => Err(ConfigError::message(format!("invalid {key}={other}"))),
    }
}

fn parse_notify_access(value: &str) -> Result<NotifyAccess, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" => Ok(NotifyAccess::None),
        "main" => Ok(NotifyAccess::Main),
        "exec" => Ok(NotifyAccess::Exec),
        "all" => Ok(NotifyAccess::All),
        other => Err(ConfigError::message(format!(
            "invalid notify_access={other}"
        ))),
    }
}

fn parse_private_tmp(value: &str) -> Result<PrivateTmpMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "on" | "1" => Ok(PrivateTmpMode::Yes),
        "no" | "false" | "off" | "0" => Ok(PrivateTmpMode::No),
        "disconnected" => Ok(PrivateTmpMode::Disconnected),
        other => Err(ConfigError::message(format!("invalid private_tmp={other}"))),
    }
}

fn parse_protect_system(value: &str) -> Result<ProtectSystemMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "no" | "false" | "off" | "0" => Ok(ProtectSystemMode::No),
        "yes" | "true" | "on" | "1" => Ok(ProtectSystemMode::Yes),
        "full" => Ok(ProtectSystemMode::Full),
        "strict" => Ok(ProtectSystemMode::Strict),
        other => Err(ConfigError::message(format!(
            "invalid protect_system={other}"
        ))),
    }
}

fn parse_protect_home(value: &str) -> Result<ProtectHomeMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "no" | "false" | "off" | "0" => Ok(ProtectHomeMode::No),
        "yes" | "true" | "on" | "1" => Ok(ProtectHomeMode::Yes),
        "read_only" => Ok(ProtectHomeMode::ReadOnly),
        "tmpfs" => Ok(ProtectHomeMode::Tmpfs),
        other => Err(ConfigError::message(format!(
            "invalid protect_home={other}"
        ))),
    }
}

fn parse_protect_proc(value: &str) -> Result<ProtectProcMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "default" => Ok(ProtectProcMode::Default),
        "no_access" => Ok(ProtectProcMode::NoAccess),
        "invisible" => Ok(ProtectProcMode::Invisible),
        "ptraceable" => Ok(ProtectProcMode::Ptraceable),
        other => Err(ConfigError::message(format!(
            "invalid protect_proc={other}"
        ))),
    }
}

fn parse_proc_subset(value: &str) -> Result<ProcSubsetMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "all" => Ok(ProcSubsetMode::All),
        "pid" => Ok(ProcSubsetMode::Pid),
        other => Err(ConfigError::message(format!("invalid proc_subset={other}"))),
    }
}

fn parse_device_policy(value: &str) -> Result<DevicePolicy, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(DevicePolicy::Auto),
        "closed" => Ok(DevicePolicy::Closed),
        "strict" => Ok(DevicePolicy::Strict),
        other => Err(ConfigError::message(format!(
            "invalid device_policy={other}"
        ))),
    }
}

fn parse_device_access(value: &str, line: usize) -> Result<u8, ConfigError> {
    let mut access = 0;
    for character in value.chars() {
        let bit = match character {
            'r' => 2,
            'w' => 4,
            'm' => 1,
            _ => {
                return Err(ConfigError::at(
                    line,
                    format!("invalid device access {value}"),
                ));
            }
        };
        if access & bit != 0 {
            return Err(ConfigError::at(
                line,
                format!("duplicate device access {value}"),
            ));
        }
        access |= bit;
    }
    if access == 0 {
        return Err(ConfigError::at(line, "device access is empty"));
    }
    Ok(access)
}

fn parse_delegate(value: &str) -> Result<DelegateMode, ConfigError> {
    let value = value.trim();
    if matches!(value, "yes" | "true" | "all") {
        return Ok(DelegateMode::All);
    }
    if matches!(value, "no" | "false" | "none") {
        return Ok(DelegateMode::No);
    }
    let mut controllers = BTreeSet::new();
    for controller in split_words(value, 0)? {
        if !matches!(
            controller.as_str(),
            "cpu" | "cpuset" | "io" | "memory" | "pids" | "hugetlb" | "rdma" | "misc" | "dmem"
        ) {
            return Err(ConfigError::message(format!(
                "invalid delegate controller {controller}"
            )));
        }
        controllers.insert(controller);
    }
    Ok(DelegateMode::Controllers(controllers))
}

fn parse_private_users(value: &str) -> Result<PrivateUsersMode, ConfigError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "self" => Ok(PrivateUsersMode::SelfMapping),
        "identity" => Ok(PrivateUsersMode::Identity),
        "full" => Ok(PrivateUsersMode::Full),
        "no" | "false" => Ok(PrivateUsersMode::No),
        other => Err(ConfigError::message(format!(
            "invalid private_users={other}"
        ))),
    }
}

fn parse_namespace_list(value: &str) -> Result<u32, ConfigError> {
    let mut flags = 0;
    for name in split_words(value, 0)? {
        flags |= match name.as_str() {
            "cgroup" => 0x0200_0000,
            "ipc" => 0x0800_0000,
            "net" => 0x4000_0000,
            "mnt" => 0x0002_0000,
            "pid" => 0x2000_0000,
            "time" => 0x0000_0080,
            "user" => 0x1000_0000,
            "uts" => 0x0400_0000,
            other => return Err(ConfigError::message(format!("invalid namespace {other}"))),
        };
    }
    Ok(flags)
}

fn parse_capabilities(value: &str) -> Result<u64, ConfigError> {
    let mut result = 0;
    for name in split_words(value, 0)? {
        let bit = match name.to_ascii_uppercase().as_str() {
            "CAP_CHOWN" => 0,
            "CAP_DAC_OVERRIDE" => 1,
            "CAP_DAC_READ_SEARCH" => 2,
            "CAP_FOWNER" => 3,
            "CAP_FSETID" => 4,
            "CAP_KILL" => 5,
            "CAP_SETGID" => 6,
            "CAP_SETUID" => 7,
            "CAP_SETPCAP" => 8,
            "CAP_LINUX_IMMUTABLE" => 9,
            "CAP_NET_BIND_SERVICE" => 10,
            "CAP_NET_BROADCAST" => 11,
            "CAP_NET_ADMIN" => 12,
            "CAP_NET_RAW" => 13,
            "CAP_IPC_LOCK" => 14,
            "CAP_IPC_OWNER" => 15,
            "CAP_SYS_MODULE" => 16,
            "CAP_SYS_RAWIO" => 17,
            "CAP_SYS_CHROOT" => 18,
            "CAP_SYS_PTRACE" => 19,
            "CAP_SYS_PACCT" => 20,
            "CAP_SYS_ADMIN" => 21,
            "CAP_SYS_BOOT" => 22,
            "CAP_SYS_NICE" => 23,
            "CAP_SYS_RESOURCE" => 24,
            "CAP_SYS_TIME" => 25,
            "CAP_SYS_TTY_CONFIG" => 26,
            "CAP_MKNOD" => 27,
            "CAP_LEASE" => 28,
            "CAP_AUDIT_WRITE" => 29,
            "CAP_AUDIT_CONTROL" => 30,
            "CAP_SETFCAP" => 31,
            "CAP_MAC_OVERRIDE" => 32,
            "CAP_MAC_ADMIN" => 33,
            "CAP_SYSLOG" => 34,
            "CAP_WAKE_ALARM" => 35,
            "CAP_BLOCK_SUSPEND" => 36,
            "CAP_AUDIT_READ" => 37,
            "CAP_PERFMON" => 38,
            "CAP_BPF" => 39,
            "CAP_CHECKPOINT_RESTORE" => 40,
            "ALL" => return Ok(ALL_LINUX_CAPABILITIES),
            other => return Err(ConfigError::message(format!("invalid capability {other}"))),
        };
        result |= 1_u64 << bit;
    }
    Ok(result)
}

fn parse_address_families(value: &str) -> Result<u64, ConfigError> {
    let mut result = 0;
    for name in split_words(value, 0)? {
        let bit = match name.to_ascii_uppercase().as_str() {
            "AF_UNIX" | "AF_LOCAL" => 1,
            "AF_INET" => 2,
            "AF_INET6" => 10,
            "AF_PACKET" => 17,
            "AF_NETLINK" => 16,
            "AF_ALG" => 38,
            "AF_VSOCK" => 40,
            "AF_QIPCRTR" => 42,
            "ALL" => return Ok(ALL_LINUX_ADDRESS_FAMILIES),
            other => {
                return Err(ConfigError::message(format!(
                    "invalid address family {other}"
                )));
            }
        };
        result |= 1_u64 << bit;
    }
    Ok(result)
}

fn parse_architectures(value: &str) -> Result<SystemCallArchitectures, ConfigError> {
    let values = split_words(value, 0)?;
    if values.iter().any(|value| value == "all") {
        return Ok(SystemCallArchitectures::All);
    }
    match values.first().map(String::as_str) {
        Some("native") => Ok(SystemCallArchitectures::Native),
        Some("x86_64") | Some("x86-64") => Ok(SystemCallArchitectures::X86_64),
        _ => Err(ConfigError::message(format!(
            "invalid syscall_architectures={value}"
        ))),
    }
}

fn parse_errno(value: &str, key: &str) -> Result<i32, ConfigError> {
    let errno = match value.to_ascii_uppercase().as_str() {
        "KILL" => 0,
        "EPERM" => 1,
        "ENOENT" => 2,
        "ESRCH" => 3,
        "EINTR" => 4,
        "EIO" => 5,
        "E2BIG" => 7,
        "EACCES" => 13,
        "EFAULT" => 14,
        "EBUSY" => 16,
        "EEXIST" => 17,
        "ENODEV" => 19,
        "EINVAL" => 22,
        "ENOSYS" => 38,
        "ENOTEMPTY" => 39,
        "ELOOP" => 40,
        "EPIPE" => 32,
        "ECANCELED" => 125,
        _ => value
            .parse::<i32>()
            .ok()
            .filter(|value| (1..=4095).contains(value))
            .ok_or_else(|| ConfigError::message(format!("invalid {key}={value}")))?,
    };
    Ok(errno)
}

fn parse_environment_condition(
    value: &str,
    key: &str,
) -> Result<(String, Option<String>), ConfigError> {
    let (name, expected) = value
        .split_once('=')
        .map_or((value, None), |(name, expected)| (name, Some(expected)));
    if !valid_environment_name(name) {
        return Err(ConfigError::message(format!(
            "invalid {key} environment name {name}"
        )));
    }
    Ok((name.to_owned(), expected.map(str::to_owned)))
}

fn parse_needs_update_path(value: &str, key: &str) -> Result<PathBuf, ConfigError> {
    match value {
        "/etc" | "/var" => Ok(PathBuf::from(value)),
        _ => Err(ConfigError::message(format!("{key} must be /etc or /var"))),
    }
}

fn valid_environment_name(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn normalize_listener_address(value: &str) -> String {
    value
        .parse::<u16>()
        .map_or_else(|_| value.to_owned(), |port| format!("0.0.0.0:{port}"))
}

fn canonical_filesystem_type(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "fat" | "msdos" => "vfat".to_owned(),
        "ext" => "ext4".to_owned(),
        value => value.to_owned(),
    }
}

fn is_reset_key(section: &str, key: &str) -> bool {
    matches!(
        (
            section.to_ascii_lowercase().as_str(),
            key.to_ascii_lowercase().as_str()
        ),
        ("service", "exec")
            | ("service", "exec_pre")
            | ("service", "exec_post")
            | ("service", "exec_stop_post")
            | ("service", "exec_condition")
            | ("service", "environment")
            | ("service", "environment_file")
            | ("service", "unset_environment")
            | ("service", "success_exit_status")
            | ("service", "restart_prevent_exit_status")
            | ("service", "bus_name")
            | ("dependencies", "requires")
            | ("dependencies", "wants")
            | ("dependencies", "after")
            | ("dependencies", "before")
            | ("dependencies", "conflicts")
            | ("dependencies", "part_of")
            | ("dependencies", "binds_to")
            | ("dependencies", "requisite")
            | ("dependencies", "on_success")
            | ("dependencies", "on_failure")
            | ("dependencies", "requires_mount")
            | ("dependencies", "wants_mount")
            | ("credentials", "file")
            | ("credentials", "value")
            | ("credentials", "store")
            | ("credentials", "import")
    )
}

fn has_continuation(value: &str) -> bool {
    let mut backslashes = 0;
    for byte in value.as_bytes().iter().rev() {
        if *byte != b'\\' {
            break;
        }
        backslashes += 1;
    }
    backslashes % 2 == 1
}

fn hex_value(value: char) -> Option<u8> {
    value.to_digit(16).map(|value| value as u8)
}

fn validate_credential_name_template(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'*'))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub line: Option<usize>,
    pub message: String,
}

impl ConfigError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    fn message(message: impl Into<String>) -> Self {
        Self {
            line: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(formatter, "line {line}: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;
    use fractald_core::{Condition, RestartPolicy, ServiceType};

    #[test]
    fn parses_a_native_process_and_dependency_graph() {
        let source = r#"
[service]
kind=service
exec=/usr/bin/example --name "native service"
exec_pre=/usr/bin/example --prepare
environment=MODE=production MESSAGE="hello world"
restart=on-failure
restart_delay=250ms
working_directory=/var/lib/example
remain_after_exit=yes

[dependencies]
requires=network
after=network storage

[install]
alias=example-default
profile=boot
"#;
        let spec = parse_service(source, "example").expect("native service");
        assert_eq!(spec.service_type, ServiceType::Simple);
        assert_eq!(spec.program, PathBuf::from("/usr/bin/example"));
        assert_eq!(
            spec.args,
            vec![OsString::from("--name"), OsString::from("native service")]
        );
        assert!(spec.dependencies.requires.contains("network"));
        assert!(spec.dependencies.after.contains("storage"));
        assert_eq!(spec.restart_backoff, Duration::from_millis(250));
        assert_eq!(spec.restart, RestartPolicy::OnFailure);
        assert!(spec.remain_after_exit);
        assert!(spec.aliases.contains("example-default"));
        assert!(spec.profiles.contains("boot"));
    }

    #[test]
    fn parses_native_mount_and_listener_records() {
        let mount = parse_service(
            "[service]\nkind=mount\n[mount]\nwhat=/dev/vda1\nwhere=/srv/data\ntype=ext4\n",
            "data",
        )
        .expect("mount");
        assert_eq!(mount.service_type, ServiceType::Mount);
        assert_eq!(mount.mount_where, Some(PathBuf::from("/srv/data")));

        let listener = parse_service(
            "[service]\nkind=listener\n[listen]\nservice=web\nstream=127.0.0.1:8080\n",
            "web-listener",
        )
        .expect("listener");
        assert_eq!(listener.service_type, ServiceType::Socket);
        assert_eq!(listener.listeners.len(), 1);
        assert_eq!(listener.socket_service.as_deref(), Some("web"));
    }

    #[test]
    fn rejects_missing_native_service_sections_and_commands() {
        assert!(parse_service("[foreign]\nexec=/bin/true\n", "bad").is_err());
        assert!(parse_service("[service]\nkind=service\n", "bad").is_err());
        assert!(
            parse_service(
                "[service]\nkind=service\nexec=/bin/true\n[conditions]\npath_exists=/tmp\n",
                "ok"
            )
            .is_ok()
        );
    }

    #[test]
    fn parses_condition_alternatives() {
        let spec = parse_service(
            "[service]\nexec=/bin/true\n[conditions]\npath_exists=/one\npath_exists=|/two\n",
            "condition",
        )
        .expect("conditions");
        assert!(matches!(spec.conditions.first(), Some(Condition::Any(_))));
    }
}
