use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use fractald_core::{
    CommandSpec, Condition, CpuQuota, CredentialImportSpec, CredentialSource, CredentialSpec,
    DelegateMode, DeviceAccessRule, DevicePolicy, DirectoryKind, DirectorySpec,
    EnvironmentFileSpec, InputMode, KillMode, LimitRange, LimitValue, ListenerKind, ListenerSpec,
    ManagerAction, NotifyAccess, OomPolicy, OutputMode, PathWatch, PrivateTmpMode,
    PrivateUsersMode, ProcSubsetMode, ProtectHomeMode, ProtectProcMode, ProtectSystemMode,
    RestartPolicy, ServiceSpec, ServiceType, SystemCallArchitectures, SystemCallFilter,
    SystemCallRule, SystemCallRuleAction, TriggerSpec,
};

mod syscall_groups;

const ALL_LINUX_CAPABILITIES: u64 = (1_u64 << 41) - 1;
const ALL_LINUX_ADDRESS_FAMILIES: u64 = (1_u64 << 45) - 1;
const ALL_LINUX_NAMESPACE_FLAGS: u32 = 0x7e020080;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directive {
    pub section: String,
    pub key: String,
    pub value: String,
    pub line: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UnitFile {
    sections: BTreeMap<String, Vec<Directive>>,
}

impl UnitFile {
    pub fn parse(source: &str) -> Result<Self, UnitError> {
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
                    return Err(UnitError::at(
                        logical_line,
                        "section header is missing its closing bracket",
                    ));
                }
                let name = line[1..line.len() - 1].trim();
                if name.is_empty() || name.contains('[') || name.contains(']') {
                    return Err(UnitError::at(logical_line, "invalid section name"));
                }
                section = Some(name.to_owned());
                file.sections.entry(name.to_owned()).or_default();
            } else {
                let section_name = section.as_deref().ok_or_else(|| {
                    UnitError::at(logical_line, "directive appears before a section header")
                })?;
                let (key, value) = line
                    .split_once('=')
                    .ok_or_else(|| UnitError::at(logical_line, "directive is missing '='"))?;
                let key = key.trim();
                if key.is_empty() || key.chars().any(char::is_whitespace) {
                    return Err(UnitError::at(logical_line, "invalid directive name"));
                }
                file.sections
                    .entry(section_name.to_owned())
                    .or_default()
                    .push(Directive {
                        section: section_name.to_owned(),
                        key: key.to_owned(),
                        value: value.trim().to_owned(),
                        line: logical_line,
                    });
            }
            logical.clear();
        }

        if !logical.is_empty() {
            return Err(UnitError::at(
                logical_line,
                "unterminated line continuation",
            ));
        }
        Ok(file)
    }

    pub fn directives(&self, section: &str, key: &str) -> Vec<&Directive> {
        self.sections
            .get(section)
            .into_iter()
            .flat_map(|directives| directives.iter())
            .filter(|directive| directive.key == key)
            .collect()
    }

    pub fn effective_directives(&self, section: &str, key: &str) -> Vec<&Directive> {
        let mut effective = Vec::new();
        for directive in self.directives(section, key) {
            if directive.value.is_empty() && is_reset_directive(directive) {
                effective.clear();
            } else {
                effective.push(directive);
            }
        }
        effective
    }

    pub fn has_section(&self, section: &str) -> bool {
        self.sections.contains_key(section)
    }

    pub fn merge(&mut self, overlay: Self) {
        for (section, directives) in overlay.sections {
            let destination = self.sections.entry(section).or_default();
            for directive in directives {
                if directive.value.is_empty() && is_reset_directive(&directive) {
                    destination.retain(|existing| existing.key != directive.key);
                    continue;
                }
                destination.push(directive);
            }
        }
    }

    pub fn value(&self, section: &str, key: &str) -> Option<&str> {
        self.effective_directives(section, key)
            .last()
            .filter(|directive| !directive.value.is_empty())
            .map(|directive| directive.value.as_str())
    }

    pub fn to_service_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Service") {
            return Err(UnitError::message(
                "unit does not contain a [Service] section",
            ));
        }

        let service_type = self
            .value("Service", "Type")
            .map(parse_service_type)
            .transpose()?
            .unwrap_or(ServiceType::Simple);
        let start_values = self.effective_directives("Service", "ExecStart");
        if start_values.len() > 1 && service_type != ServiceType::Oneshot {
            return Err(UnitError::at(
                start_values[1].line,
                "multiple ExecStart directives are not supported by one ServiceSpec",
            ));
        }
        let starts = start_values
            .iter()
            .map(|directive| parse_command(&directive.value, directive.line))
            .collect::<Result<Vec<_>, _>>()?;
        let start = starts
            .last()
            .cloned()
            .or_else(|| {
                (service_type == ServiceType::Oneshot
                    && self
                        .effective_directives("Service", "ExecStop")
                        .iter()
                        .any(|directive| !directive.value.is_empty()))
                .then(|| CommandSpec::new("/bin/true"))
            })
            .ok_or_else(|| UnitError::message("service has no ExecStart directive"))?;
        let mut spec = ServiceSpec::new(name, start.program);
        spec.args = start.args;
        spec.main_ignore_failure = start.ignore_failure;
        spec.main_argv0 = start.argv0;
        spec.main_expand_environment = start.expand_environment;
        spec.service_type = service_type;
        for directive in self.effective_directives("Service", "BusName") {
            for bus_name in split_words(&directive.value, directive.line)? {
                if !valid_bus_name(&bus_name) {
                    return Err(UnitError::at(
                        directive.line,
                        format!("invalid BusName={bus_name}"),
                    ));
                }
                spec.bus_names.push(bus_name);
            }
        }
        if service_type == ServiceType::Dbus && spec.bus_names.is_empty() {
            return Err(UnitError::message("Type=dbus requires BusName"));
        }

        for directive in self.effective_directives("Install", "Alias") {
            for alias in split_words(&directive.value, directive.line)? {
                if alias.is_empty()
                    || alias
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
                    || alias.contains('/')
                {
                    return Err(UnitError::at(
                        directive.line,
                        format!("invalid install alias {alias}"),
                    ));
                }
                spec.aliases.insert(alias);
            }
        }

        if let Some(value) = self.value("Unit", "RefuseManualStart") {
            spec.refuse_manual_start = parse_bool(value, "RefuseManualStart")?;
        }
        if let Some(value) = self.value("Unit", "RefuseManualStop") {
            spec.refuse_manual_stop = parse_bool(value, "RefuseManualStop")?;
        }

        if let Some(value) = self.value("Service", "Restart") {
            spec.restart = parse_restart(value)?;
        }
        if let Some(value) = self.value("Service", "RestartSec") {
            spec.restart_backoff = parse_duration(value, "RestartSec")?;
        }
        if let Some(value) = self.value("Service", "TimeoutSec") {
            let timeout = parse_duration(value, "TimeoutSec")?;
            spec.start_timeout = timeout;
            spec.stop_timeout = timeout;
        }
        if let Some(value) = self.value("Service", "TimeoutStartSec") {
            spec.start_timeout = parse_duration(value, "TimeoutStartSec")?;
        }
        if let Some(value) = self.value("Service", "TimeoutStopSec") {
            spec.stop_timeout = parse_duration(value, "TimeoutStopSec")?;
        }
        if let Some(value) = self.value("Service", "RuntimeMaxSec") {
            let runtime_max = parse_duration(value, "RuntimeMaxSec")?;
            spec.runtime_max = (!runtime_max.is_zero()).then_some(runtime_max);
        }
        if let Some(value) = self.value("Service", "KillSignal") {
            spec.kill_signal = parse_signal(value)?;
        }
        if let Some(value) = self.value("Service", "RestartKillSignal") {
            spec.restart_kill_signal = Some(parse_signal(value)?);
        }
        if let Some(value) = self.value("Service", "SendSIGHUP") {
            spec.send_sighup = parse_bool(value, "SendSIGHUP")?;
        }
        if let Some(value) = self.value("Service", "IgnoreSIGPIPE") {
            spec.ignore_sigpipe = parse_bool(value, "IgnoreSIGPIPE")?;
        }
        if let Some(value) = self.value("Service", "KillMode") {
            spec.kill_mode = parse_kill_mode(value)?;
        }
        if let Some(value) = self.value("Service", "WorkingDirectory") {
            spec.working_directory = Some(PathBuf::from(value));
        }
        if let Some(value) = self.value("Service", "PIDFile") {
            if value.is_empty() {
                return Err(UnitError::message("PIDFile cannot be empty"));
            }
            spec.pid_file = Some(PathBuf::from(value));
        }
        if let Some(value) = self.value("Service", "User") {
            spec.user = Some(value.to_owned());
        }
        if let Some(value) = self.value("Service", "Group") {
            spec.group = Some(value.to_owned());
        }
        if let Some(value) = self.value("Service", "DynamicUser") {
            spec.dynamic_user = parse_bool(value, "DynamicUser")?;
        }
        if let Some(value) = self.value("Service", "Slice") {
            spec.cgroup_slice = Some(parse_slice(value, "Slice")?);
        }
        for directive in self.effective_directives("Service", "Sockets") {
            for socket in split_words(&directive.value, directive.line)? {
                spec.dependencies.wants.insert(socket.clone());
                spec.dependencies.after.insert(socket);
            }
        }
        let supplementary_group_directives = self.directives("Service", "SupplementaryGroups");
        if !supplementary_group_directives.is_empty() {
            let mut groups = Vec::new();
            for directive in self.effective_directives("Service", "SupplementaryGroups") {
                for group in split_words(&directive.value, directive.line)? {
                    if group.is_empty()
                        || group
                            .chars()
                            .any(|character| character.is_control() || character == ':')
                    {
                        return Err(UnitError::at(
                            directive.line,
                            format!("invalid SupplementaryGroups value {group}"),
                        ));
                    }
                    groups.push(group);
                }
            }
            spec.supplementary_groups = Some(groups);
        }
        let capability_directives = self.directives("Service", "CapabilityBoundingSet");
        if !capability_directives.is_empty() {
            let mut mask = None;
            for directive in capability_directives {
                if directive.value.trim().is_empty() {
                    mask = Some(0);
                    continue;
                }
                let (inverted, value) = directive
                    .value
                    .strip_prefix('~')
                    .map_or((false, directive.value.as_str()), |value| (true, value));
                if inverted && value.trim().is_empty() {
                    mask = Some(ALL_LINUX_CAPABILITIES);
                    continue;
                }
                let values = split_words(value, directive.line)?;
                if values.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        "CapabilityBoundingSet contains an empty value",
                    ));
                }
                let mut bits = 0_u64;
                for value in values {
                    bits |= parse_capability(&value, directive.line)?;
                }
                let current = mask.unwrap_or(if inverted { ALL_LINUX_CAPABILITIES } else { 0 });
                mask = Some(if inverted {
                    current & !bits
                } else {
                    current | bits
                });
            }
            spec.capability_bounding_set = mask;
        }
        let ambient_capability_directives = self.directives("Service", "AmbientCapabilities");
        if !ambient_capability_directives.is_empty() {
            let mut mask = 0_u64;
            for directive in ambient_capability_directives {
                if directive.value.trim().is_empty() {
                    mask = 0;
                    continue;
                }
                if directive.value.trim_start().starts_with('~') {
                    return Err(UnitError::at(
                        directive.line,
                        "AmbientCapabilities does not support inverted capability lists",
                    ));
                }
                let values = split_words(&directive.value, directive.line)?;
                if values.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        "AmbientCapabilities contains an empty value",
                    ));
                }
                for value in values {
                    mask |= parse_capability(&value, directive.line)?;
                }
            }
            spec.ambient_capabilities = Some(mask);
        }
        let address_family_directives = self.directives("Service", "RestrictAddressFamilies");
        if !address_family_directives.is_empty() {
            let mut mask = None;
            for directive in address_family_directives {
                if directive.value.trim().is_empty() {
                    mask = Some(0);
                    continue;
                }
                let (inverted, value) = directive
                    .value
                    .strip_prefix('~')
                    .map_or((false, directive.value.as_str()), |value| (true, value));
                if inverted && value.trim().is_empty() {
                    mask = Some(ALL_LINUX_ADDRESS_FAMILIES);
                    continue;
                }
                let values = split_words(value, directive.line)?;
                if values.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        "RestrictAddressFamilies contains an empty value",
                    ));
                }
                let mut bits = 0_u64;
                for value in values {
                    bits |= parse_address_family(&value, directive.line)?;
                }
                let current = mask.unwrap_or(if inverted {
                    ALL_LINUX_ADDRESS_FAMILIES
                } else {
                    0
                });
                mask = Some(if inverted {
                    current & !bits
                } else {
                    current | bits
                });
            }
            spec.restrict_address_families = mask;
        }
        spec.system_call_error_number = self
            .value("Service", "SystemCallErrorNumber")
            .map(|value| parse_errno(value, "SystemCallErrorNumber"))
            .transpose()?;
        if let Some(value) = self.value("Service", "SystemCallArchitectures") {
            spec.system_call_architectures = Some(parse_system_call_architectures(value)?);
        }
        spec.system_call_filter = parse_system_call_filter(self)?;
        if let Some(value) = self.value("Service", "NoNewPrivileges") {
            spec.no_new_privileges = parse_bool(value, "NoNewPrivileges")?;
        }
        if let Some(value) = self.value("Service", "MemoryDenyWriteExecute") {
            spec.memory_deny_write_execute = parse_bool(value, "MemoryDenyWriteExecute")?;
        }
        if let Some(value) = self.value("Service", "RestrictRealtime") {
            spec.restrict_realtime = parse_bool(value, "RestrictRealtime")?;
        }
        for (key, destination) in [
            ("ProtectControlGroups", &mut spec.protect_control_groups),
            ("ProtectKernelModules", &mut spec.protect_kernel_modules),
            ("ProtectKernelTunables", &mut spec.protect_kernel_tunables),
            ("ProtectKernelLogs", &mut spec.protect_kernel_logs),
            ("ProtectClock", &mut spec.protect_clock),
            ("ProtectHostname", &mut spec.protect_hostname),
            ("LockPersonality", &mut spec.lock_personality),
            ("RestrictSUIDSGID", &mut spec.restrict_suid_sgid),
        ] {
            if let Some(value) = self.value("Service", key) {
                *destination = parse_bool(value, key)?;
            }
        }
        if let Some(value) = self.value("Service", "UMask") {
            spec.umask = Some(parse_mode(value, "UMask")?);
        }
        if let Some(value) = self.value("Service", "Nice") {
            let nice = value
                .parse::<i32>()
                .map_err(|_| UnitError::message(format!("invalid Nice={value}")))?;
            if !(-20..=19).contains(&nice) {
                return Err(UnitError::message(format!("invalid Nice={value}")));
            }
            spec.nice = Some(nice);
        }
        if let Some(value) = self.value("Service", "OOMScoreAdjust") {
            let adjust = value
                .parse::<i32>()
                .map_err(|_| UnitError::message(format!("invalid OOMScoreAdjust={value}")))?;
            if !(-1000..=1000).contains(&adjust) {
                return Err(UnitError::message(format!(
                    "invalid OOMScoreAdjust={value}"
                )));
            }
            spec.oom_score_adjust = Some(adjust);
        }
        if let Some(value) = self.value("Service", "OOMPolicy") {
            spec.oom_policy = parse_oom_policy(value)?;
        }
        if let Some(value) = self.value("Service", "LimitNOFILE") {
            spec.nofile = Some(parse_limit_range(value, "LimitNOFILE", false)?);
        }
        if let Some(value) = self.value("Service", "LimitMEMLOCK") {
            spec.memlock = Some(parse_limit_range(value, "LimitMEMLOCK", true)?);
        }
        if let Some(value) = self.value("Service", "LimitNPROC") {
            spec.nproc = Some(parse_limit_range(value, "LimitNPROC", false)?);
        }
        if let Some(value) = self.value("Service", "WatchdogSec") {
            let watchdog = parse_duration(value, "WatchdogSec")?;
            spec.watchdog = (!watchdog.is_zero()).then_some(watchdog);
        }
        let configured_notify_access = self
            .value("Service", "NotifyAccess")
            .map(parse_notify_access)
            .transpose()?;
        spec.notify_access = match configured_notify_access {
            Some(NotifyAccess::None)
                if service_type == ServiceType::Notify || spec.watchdog.is_some() =>
            {
                NotifyAccess::Main
            }
            Some(access) => access,
            None if service_type == ServiceType::Notify || spec.watchdog.is_some() => {
                NotifyAccess::Main
            }
            None => NotifyAccess::None,
        };
        if let Some(value) = self.value("Service", "PrivateTmp") {
            spec.private_tmp = parse_private_tmp(value)?;
        }
        if let Some(value) = self.value("Service", "PrivateDevices") {
            spec.private_devices = parse_bool(value, "PrivateDevices")?;
        }
        if let Some(value) = self.value("Service", "DevicePolicy") {
            spec.device_policy = parse_device_policy(value)?;
        }
        let device_allow_directives = self.directives("Service", "DeviceAllow");
        if !device_allow_directives.is_empty() {
            let mut rules = Vec::new();
            for directive in device_allow_directives {
                if directive.value.trim().is_empty() {
                    rules.clear();
                    continue;
                }
                let values = split_words(&directive.value, directive.line)?;
                if values.len() != 2 {
                    return Err(UnitError::at(
                        directive.line,
                        "DeviceAllow must contain a device specifier and an access mode",
                    ));
                }
                let device = values[0].clone();
                if !(device.starts_with("/dev/")
                    || device.starts_with("char-")
                    || device.starts_with("block-"))
                    || (device.starts_with("/dev/") && device.contains(['*', '?']))
                    || device.chars().any(char::is_control)
                {
                    return Err(UnitError::at(
                        directive.line,
                        format!("invalid DeviceAllow device {device}"),
                    ));
                }
                rules.push(DeviceAccessRule {
                    device,
                    access: parse_device_access(&values[1], directive.line)?,
                });
            }
            spec.device_allow = rules;
        }
        if let Some(directive) = self.directives("Service", "Delegate").last() {
            spec.delegate = parse_delegate(&directive.value, directive.line)?;
        }
        if let Some(directive) = self.directives("Service", "PrivateUsers").last() {
            spec.private_users = parse_private_users(&directive.value, directive.line)?;
        }
        if let Some(value) = self.value("Service", "PrivateMounts") {
            spec.private_mounts = parse_bool(value, "PrivateMounts")?;
        }
        if let Some(value) = self.value("Service", "PrivateIPC") {
            spec.private_ipc = parse_bool(value, "PrivateIPC")?;
        }
        if let Some(value) = self.value("Service", "PrivateNetwork") {
            spec.private_network = parse_bool(value, "PrivateNetwork")?;
        }
        let namespace_directives = self.directives("Service", "RestrictNamespaces");
        if !namespace_directives.is_empty() {
            spec.restrict_namespaces = Some(parse_restrict_namespaces(namespace_directives)?);
        }
        if let Some(value) = self.value("Service", "ProtectSystem") {
            spec.protect_system = parse_protect_system(value)?;
        }
        if let Some(value) = self.value("Service", "ProtectHome") {
            spec.protect_home = parse_protect_home(value)?;
        }
        if let Some(value) = self.value("Service", "ProtectProc") {
            spec.protect_proc = parse_protect_proc(value)?;
        }
        if let Some(value) = self.value("Service", "ProcSubset") {
            spec.proc_subset = parse_proc_subset(value)?;
        }
        for directive in self.effective_directives("Service", "ReadWritePaths") {
            for value in split_words(&directive.value, directive.line)? {
                let path = value.strip_prefix('-').unwrap_or(&value);
                let path = PathBuf::from(path);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(UnitError::at(
                        directive.line,
                        "ReadWritePaths must contain absolute paths without '..'",
                    ));
                }
                spec.read_write_paths.push(path);
            }
        }
        for directive in self.effective_directives("Service", "ReadOnlyPaths") {
            for value in split_words(&directive.value, directive.line)? {
                let path = value.strip_prefix('-').unwrap_or(&value);
                let path = PathBuf::from(path);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(UnitError::at(
                        directive.line,
                        "ReadOnlyPaths must contain absolute paths without '..'",
                    ));
                }
                spec.read_only_paths.push(path);
            }
        }
        for directive in self.effective_directives("Service", "InaccessiblePaths") {
            for value in split_words(&directive.value, directive.line)? {
                let path = value.strip_prefix('-').unwrap_or(&value);
                let path = PathBuf::from(path);
                if !path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(UnitError::at(
                        directive.line,
                        "InaccessiblePaths must contain absolute paths without '..'",
                    ));
                }
                spec.inaccessible_paths.push(path);
            }
        }
        if let Some(value) = self.value("Service", "StandardInput") {
            spec.standard_input = parse_input_mode(value, "StandardInput")?;
        }
        if let Some(value) = self.value("Service", "TTYPath") {
            if value.is_empty() || !value.starts_with('/') {
                return Err(UnitError::message(format!("invalid TTYPath={value}")));
            }
            spec.tty_path = Some(PathBuf::from(value));
        }
        if let Some(value) = self.value("Service", "StandardOutput") {
            spec.stdout = parse_output_mode(value, "StandardOutput")?;
        }
        if let Some(value) = self.value("Service", "StandardError") {
            spec.stderr = parse_output_mode(value, "StandardError")?;
        }
        if let Some(value) = self.value("Service", "RemainAfterExit") {
            spec.remain_after_exit = parse_bool(value, "RemainAfterExit")?;
        }
        if let Some(value) = self.value("Service", "MemoryMax") {
            spec.resources.memory_max = Some(parse_limit(value, "MemoryMax", true)?);
        }
        for (key, destination) in [
            ("MemoryHigh", &mut spec.resources.memory_high),
            ("MemoryMin", &mut spec.resources.memory_min),
            ("MemoryLow", &mut spec.resources.memory_low),
            ("MemorySwapMax", &mut spec.resources.memory_swap_max),
        ] {
            if let Some(value) = self.value("Service", key) {
                *destination = Some(parse_limit(value, key, true)?);
            }
        }
        if let Some(value) = self.value("Service", "CPUWeight") {
            let weight = value
                .parse::<u64>()
                .map_err(|_| UnitError::message(format!("invalid CPUWeight={value}")))?;
            if !(1..=10_000).contains(&weight) {
                return Err(UnitError::message(format!("invalid CPUWeight={value}")));
            }
            spec.resources.cpu_weight = Some(weight);
        }
        if let Some(value) = self.value("Service", "IOWeight") {
            let weight = value
                .parse::<u64>()
                .map_err(|_| UnitError::message(format!("invalid IOWeight={value}")))?;
            if !(1..=10_000).contains(&weight) {
                return Err(UnitError::message(format!("invalid IOWeight={value}")));
            }
            spec.resources.io_weight = Some(weight);
        }
        if let Some(value) = self.value("Service", "TasksMax") {
            spec.resources.tasks_max = Some(parse_limit(value, "TasksMax", false)?);
        }
        spec.resources.cpu_quota = parse_cpu_quota(self, "Service")?;
        for (key, destination) in [
            ("SuccessExitStatus", &mut spec.success_exit_status),
            (
                "RestartPreventExitStatus",
                &mut spec.restart_prevent_exit_status,
            ),
        ] {
            for directive in self.effective_directives("Service", key) {
                for value in split_words(&directive.value, directive.line)? {
                    destination.insert(parse_exit_status(&value, key)?);
                }
            }
        }

        for directive in self.effective_directives("Service", "EnvironmentFile") {
            for entry in split_words(&directive.value, directive.line)? {
                let (optional, path) = entry
                    .strip_prefix('-')
                    .map_or((false, entry.as_str()), |path| (true, path));
                if path.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        "EnvironmentFile contains an empty path",
                    ));
                }
                spec.environment_files.push(EnvironmentFileSpec {
                    path: PathBuf::from(path),
                    optional,
                });
            }
        }

        for directive in self.effective_directives("Service", "Environment") {
            for assignment in split_words(&directive.value, directive.line)? {
                let (key, value) = assignment.split_once('=').ok_or_else(|| {
                    UnitError::at(directive.line, "Environment entries must use KEY=VALUE")
                })?;
                if key.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        "Environment contains an empty variable name",
                    ));
                }
                spec.environment
                    .insert(OsString::from(key), OsString::from(value));
            }
        }
        parse_credentials(self, &mut spec)?;
        let unset_environment_directives = self.directives("Service", "UnsetEnvironment");
        if !unset_environment_directives.is_empty() {
            for directive in self.effective_directives("Service", "UnsetEnvironment") {
                for variable in split_words(&directive.value, directive.line)? {
                    if !valid_environment_name(&variable) {
                        return Err(UnitError::at(
                            directive.line,
                            format!("invalid UnsetEnvironment variable {variable}"),
                        ));
                    }
                    spec.unset_environment.insert(OsString::from(variable));
                }
            }
        }

        if let Some(directive) = self.effective_directives("Service", "ExecStop").last() {
            if !directive.value.is_empty() {
                spec.stop = Some(parse_command(&directive.value, directive.line)?);
            }
        }
        if let Some(directive) = self.effective_directives("Service", "ExecReload").last() {
            if !directive.value.is_empty() {
                spec.reload = Some(parse_command(&directive.value, directive.line)?);
            }
        }
        for (key, destination) in [
            ("ExecStartPre", &mut spec.start_pre),
            ("ExecStartPost", &mut spec.start_post),
            ("ExecStopPost", &mut spec.stop_post),
        ] {
            for directive in self.effective_directives("Service", key) {
                if !directive.value.is_empty() {
                    destination.push(parse_command(&directive.value, directive.line)?);
                }
            }
        }
        for directive in self.effective_directives("Service", "ExecCondition") {
            if !directive.value.is_empty() {
                spec.exec_conditions
                    .push(parse_command(&directive.value, directive.line)?);
            }
        }
        parse_directories(self, &mut spec)?;
        if service_type == ServiceType::Oneshot {
            spec.start_pre.extend(
                starts
                    .into_iter()
                    .take(start_values.len().saturating_sub(1)),
            );
        }

        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_timer_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Timer") {
            return Err(UnitError::message(
                "timer does not contain a [Timer] section",
            ));
        }
        let service = self.timer_target("Timer", &name);
        let on_boot = self
            .value("Timer", "OnBootSec")
            .or_else(|| self.value("Timer", "OnStartupSec"))
            .map(|value| parse_duration(value, "OnBootSec"))
            .transpose()?;
        let on_unit_active = self
            .value("Timer", "OnUnitActiveSec")
            .or_else(|| self.value("Timer", "OnActiveSec"))
            .map(|value| parse_duration(value, "OnUnitActiveSec"))
            .transpose()?;
        let on_unit_inactive = self
            .value("Timer", "OnUnitInactiveSec")
            .map(|value| parse_duration(value, "OnUnitInactiveSec"))
            .transpose()?;
        let on_calendar = self
            .effective_directives("Timer", "OnCalendar")
            .into_iter()
            .filter_map(|directive| {
                let value = directive.value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            })
            .collect::<Vec<_>>();
        let persistent = self
            .value("Timer", "Persistent")
            .map(|value| parse_bool(value, "Persistent"))
            .transpose()?
            .unwrap_or(false);
        let randomized_delay = self
            .value("Timer", "RandomizedDelaySec")
            .map(|value| parse_duration(value, "RandomizedDelaySec"))
            .transpose()?
            .filter(|delay| !delay.is_zero());
        let accuracy = self
            .value("Timer", "AccuracySec")
            .map(|value| parse_duration(value, "AccuracySec"))
            .transpose()?
            .filter(|accuracy| !accuracy.is_zero());
        if on_boot.is_none()
            && on_unit_active.is_none()
            && on_unit_inactive.is_none()
            && on_calendar.is_empty()
        {
            return Err(UnitError::message("timer has no supported schedule"));
        }
        let mut spec = ServiceSpec::new(name, "/bin/true");
        spec.service_type = ServiceType::Timer;
        spec.remain_after_exit = true;
        spec.trigger = Some(TriggerSpec::Timer {
            service,
            on_boot,
            on_unit_active,
            on_unit_inactive,
            on_calendar,
            persistent,
            randomized_delay,
            accuracy,
        });
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_path_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Path") {
            return Err(UnitError::message(
                "path unit does not contain a [Path] section",
            ));
        }
        let service = self.timer_target("Path", &name);
        let mut watches = Vec::new();
        for (key, kind) in [
            ("PathChanged", 0_u8),
            ("PathModified", 1_u8),
            ("PathExists", 2_u8),
            ("PathExistsGlob", 3_u8),
            ("DirectoryNotEmpty", 4_u8),
        ] {
            for directive in self.effective_directives("Path", key) {
                for value in split_words(&directive.value, directive.line)? {
                    if value.is_empty() {
                        return Err(UnitError::at(
                            directive.line,
                            format!("{key} contains an empty path"),
                        ));
                    }
                    watches.push(match kind {
                        0 => PathWatch::Changed(PathBuf::from(value)),
                        1 => PathWatch::Modified(PathBuf::from(value)),
                        2 => PathWatch::Exists(PathBuf::from(value)),
                        3 => PathWatch::ExistsGlob(PathBuf::from(value)),
                        _ => PathWatch::DirectoryNotEmpty(PathBuf::from(value)),
                    });
                }
            }
        }
        if watches.is_empty() {
            return Err(UnitError::message("path unit has no supported watch"));
        }
        let mut spec = ServiceSpec::new(name, "/bin/true");
        spec.service_type = ServiceType::Path;
        spec.remain_after_exit = true;
        spec.trigger = Some(TriggerSpec::Path { service, watches });
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_mount_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Mount") {
            return Err(UnitError::message(
                "mount does not contain a [Mount] section",
            ));
        }
        let what = required_value(self, "Mount", "What")?;
        let where_path = PathBuf::from(required_value(self, "Mount", "Where")?);
        if !where_path.is_absolute() {
            return Err(UnitError::message("Mount Where must be an absolute path"));
        }

        let mount_filesystem = self.value("Mount", "Type").map(canonical_filesystem_type);
        let mut mount = CommandSpec::new("mount");
        if let Some(value) = mount_filesystem.as_deref() {
            mount
                .args
                .extend([OsString::from("-t"), OsString::from(value)]);
        }
        if let Some(value) = self.value("Mount", "Options") {
            mount
                .args
                .extend([OsString::from("-o"), OsString::from(value)]);
        }
        if let Some(value) = self.value("Mount", "SloppyOptions") {
            if parse_bool(value, "SloppyOptions")? {
                mount.args.push(OsString::from("-s"));
            }
        }
        mount.args.push(OsString::from("--"));
        mount.args.push(OsString::from(what));
        mount.args.push(where_path.as_os_str().to_owned());
        if self
            .value("Mount", "Options")
            .is_some_and(|value| value.split(',').any(|option| option == "nofail"))
        {
            mount.ignore_failure = true;
        }

        let mut spec = ServiceSpec::new(name, mount.program);
        spec.args = mount.args;
        spec.main_ignore_failure = mount.ignore_failure;
        spec.service_type = ServiceType::Mount;
        spec.mount_where = Some(where_path.clone());
        spec.mount_filesystem = mount_filesystem;
        spec.remain_after_exit = true;
        let directory_mode = self
            .value("Mount", "DirectoryMode")
            .map(|value| parse_mode(value, "DirectoryMode"))
            .transpose()?
            .unwrap_or(0o755);
        let mut mkdir = CommandSpec::new("mkdir");
        mkdir.args = vec![
            OsString::from("-p"),
            OsString::from("-m"),
            OsString::from(format!("{directory_mode:o}")),
            OsString::from("--"),
            where_path.as_os_str().to_owned(),
        ];
        spec.start_pre.push(mkdir);

        let mut unmount = CommandSpec::new("umount");
        if let Some(value) = self.value("Mount", "LazyUnmount") {
            if parse_bool(value, "LazyUnmount")? {
                unmount.args.push(OsString::from("--lazy"));
            }
        }
        if let Some(value) = self.value("Mount", "ForceUnmount") {
            if parse_bool(value, "ForceUnmount")? {
                unmount.args.push(OsString::from("--force"));
            }
        }
        unmount
            .args
            .extend([OsString::from("--"), where_path.as_os_str().to_owned()]);
        spec.stop = Some(unmount);
        if let Some(value) = self.value("Mount", "TimeoutSec") {
            let timeout = parse_duration(value, "TimeoutSec")?;
            spec.start_timeout = timeout;
            spec.stop_timeout = timeout;
        }
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_swap_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Swap") {
            return Err(UnitError::message("swap does not contain a [Swap] section"));
        }
        let what = required_value(self, "Swap", "What")?;
        let mut swapon = CommandSpec::new("swapon");
        if let Some(value) = self.value("Swap", "Priority") {
            if value.parse::<i32>().is_err() {
                return Err(UnitError::message(format!("invalid Priority={value}")));
            }
            swapon
                .args
                .extend([OsString::from("--priority"), OsString::from(value)]);
        }
        if let Some(value) = self.value("Swap", "Options") {
            swapon
                .args
                .extend([OsString::from("-o"), OsString::from(value)]);
        }
        swapon
            .args
            .extend([OsString::from("--"), OsString::from(what.clone())]);
        if self
            .value("Swap", "Options")
            .is_some_and(|value| value.split(',').any(|option| option == "nofail"))
        {
            swapon.ignore_failure = true;
        }

        let mut spec = ServiceSpec::new(name, swapon.program);
        spec.args = swapon.args;
        spec.main_ignore_failure = swapon.ignore_failure;
        spec.service_type = ServiceType::Swap;
        spec.remain_after_exit = true;
        let mut swapoff = CommandSpec::new("swapoff");
        swapoff.args = vec![OsString::from("--"), OsString::from(what)];
        spec.stop = Some(swapoff);
        if let Some(value) = self.value("Swap", "TimeoutSec") {
            let timeout = parse_duration(value, "TimeoutSec")?;
            spec.start_timeout = timeout;
            spec.stop_timeout = timeout;
        }
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_device_spec(
        &self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
    ) -> Result<ServiceSpec, UnitError> {
        let path = path.into();
        if !path.is_absolute() {
            return Err(UnitError::message("device path must be absolute"));
        }
        let mut spec = ServiceSpec::new(name, "/bin/sh");
        spec.args = vec![
            OsString::from("-c"),
            OsString::from(
                "device_identity_ready() { \
  [ -e \"$1\" ] && return 0; \
  case \"$1\" in \
    /dev/disk/by-uuid/*) kind=UUID ;; \
    /dev/disk/by-label/*) kind=LABEL ;; \
    /dev/disk/by-partuuid/*) kind=PARTUUID ;; \
    /dev/disk/by-partlabel/*) kind=PARTLABEL ;; \
    *) return 1 ;; \
  esac; \
  value=\"${1##*/}\"; \
  for helper in /usr/bin/blkid /usr/sbin/blkid; do \
    [ -x \"$helper\" ] || continue; \
    case \"$kind\" in \
      UUID) \"$helper\" -U \"$value\" >/dev/null 2>&1 ;; \
      LABEL) \"$helper\" -L \"$value\" >/dev/null 2>&1 ;; \
      PARTUUID|PARTLABEL) \"$helper\" -t \"$kind=$value\" -o device >/dev/null 2>&1 ;; \
    esac && return 0; \
  done; \
  return 1; \
}; \
while ! device_identity_ready \"$1\"; do sleep 1; done",
            ),
            OsString::from("fractald-device-wait"),
            path.as_os_str().to_owned(),
        ];
        spec.main_expand_environment = false;
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        spec.default_dependencies = false;
        spec.start_timeout = Duration::MAX;
        spec.device_path = Some(path);
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_automount_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Automount") {
            return Err(UnitError::message(
                "automount does not contain an [Automount] section",
            ));
        }
        let where_path = PathBuf::from(required_value(self, "Automount", "Where")?);
        if !where_path.is_absolute() {
            return Err(UnitError::message(
                "Automount Where must be an absolute path",
            ));
        }

        // The kernel autofs protocol is deliberately kept out of the common
        // service process path.  An automount unit therefore becomes an
        // eager, dependency-aware mount transaction: create the point and
        // request its corresponding mount unit.  This preserves package boot
        // ordering and mount lifecycle behavior while the native autofs
        // provider is implemented separately.
        let directory_mode = self
            .value("Automount", "DirectoryMode")
            .map(|value| parse_mode(value, "DirectoryMode"))
            .transpose()?
            .unwrap_or(0o755);
        let mut mkdir = CommandSpec::new("mkdir");
        mkdir.args = vec![
            OsString::from("-p"),
            OsString::from("-m"),
            OsString::from(format!("{directory_mode:o}")),
            OsString::from("--"),
            where_path.as_os_str().to_owned(),
        ];

        let mut spec = ServiceSpec::new(name.clone(), "/bin/true");
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        spec.start_pre.push(mkdir);
        let mount_name = format!(
            "{}.mount",
            name.strip_suffix(".automount").unwrap_or(name.as_str())
        );
        spec.dependencies.wants.insert(mount_name.clone());
        spec.dependencies.after.insert(mount_name);
        if let Some(value) = self.value("Automount", "TimeoutIdleSec") {
            let _ = parse_duration(value, "TimeoutIdleSec")?;
        }
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_slice_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        // A slice is a cgroup placement unit, so it has no process of its
        // own.  Keep an active, zero-work lifecycle node in the graph; child
        // services still receive their own Service=Slice= placement.
        let mut spec = ServiceSpec::new(name, "/bin/true");
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        spec.resources.cpu_quota = parse_cpu_quota(self, "Slice")?;
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_action_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Unit") {
            return Err(UnitError::message(
                "action unit does not contain a [Unit] section",
            ));
        }
        let action = required_value(self, "Unit", "SuccessAction")?;
        let manager_action = parse_manager_action(&action, "SuccessAction")?;
        let mut spec = ServiceSpec::new(name, "/bin/true");
        spec.service_type = ServiceType::Oneshot;
        spec.success_action = manager_action;
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    fn timer_target(&self, section: &str, name: &str) -> String {
        self.value(section, "Unit")
            .map(str::to_owned)
            .unwrap_or_else(|| {
                format!(
                    "{}.service",
                    name.strip_suffix(if section == "Timer" {
                        ".timer"
                    } else {
                        ".path"
                    })
                    .unwrap_or(name)
                )
            })
    }

    pub fn to_target_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        if !self.sections.contains_key("Unit") {
            return Err(UnitError::message(
                "target does not contain a [Unit] section",
            ));
        }
        let mut spec = ServiceSpec::new(name, "/bin/true");
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }

    pub fn to_socket_spec(&self, name: impl Into<String>) -> Result<ServiceSpec, UnitError> {
        let name = name.into();
        if !self.sections.contains_key("Socket") {
            return Err(UnitError::message(
                "socket does not contain a [Socket] section",
            ));
        }
        let accept = self
            .value("Socket", "Accept")
            .map(|value| parse_bool(value, "Accept"))
            .transpose()?
            .unwrap_or(false);
        let mut spec = ServiceSpec::new(name.clone(), "/bin/true");
        spec.service_type = ServiceType::Socket;
        spec.remain_after_exit = true;
        spec.socket_accept = accept;
        if let Some(directive) = self
            .effective_directives("Socket", "FileDescriptorName")
            .last()
        {
            if !is_valid_file_descriptor_name(&directive.value) {
                return Err(UnitError::at(
                    directive.line,
                    format!("invalid FileDescriptorName={}", directive.value),
                ));
            }
            spec.file_descriptor_name = Some(directive.value.clone());
        }
        if let Some(value) = self.value("Socket", "RemoveOnStop") {
            spec.remove_on_stop = parse_bool(value, "RemoveOnStop")?;
        }
        if let Some(value) = self.value("Socket", "SocketMode") {
            spec.socket_mode = parse_mode(value, "SocketMode")?;
        }
        if let Some(value) = self.value("Socket", "SocketUser") {
            spec.socket_user = Some(value.to_owned());
        }
        if let Some(value) = self.value("Socket", "SocketGroup") {
            spec.socket_group = Some(value.to_owned());
        }
        let stem = name.strip_suffix(".socket").unwrap_or(name.as_str());
        let default_service = if accept {
            let base = stem.rsplit_once('@').map_or(stem, |(prefix, _)| prefix);
            format!("{base}@.service")
        } else {
            format!("{stem}.service")
        };
        spec.socket_service = Some(
            self.value("Socket", "Service")
                .map(str::to_owned)
                .unwrap_or(default_service),
        );
        for (key, kind) in [
            ("ListenStream", ListenerKind::Stream),
            ("ListenDatagram", ListenerKind::Datagram),
            ("ListenSequentialPacket", ListenerKind::SequentialPacket),
            ("ListenFIFO", ListenerKind::Fifo),
            ("ListenNetlink", ListenerKind::Netlink),
            ("ListenSpecial", ListenerKind::Special),
        ] {
            for directive in self.effective_directives("Socket", key) {
                if directive.value.is_empty() {
                    continue;
                }
                let addresses = if kind == ListenerKind::Netlink {
                    vec![directive.value.clone()]
                } else {
                    split_words(&directive.value, directive.line)?
                };
                for address in addresses {
                    if address.is_empty() {
                        return Err(UnitError::at(directive.line, "socket address is empty"));
                    }
                    spec.listeners.push(ListenerSpec {
                        kind,
                        address: if kind == ListenerKind::Netlink {
                            address
                        } else {
                            normalize_listener_address(&address)
                        },
                    });
                }
            }
        }
        if spec.listeners.is_empty() {
            return Err(UnitError::message(
                "socket has no supported ListenStream or ListenDatagram",
            ));
        }
        if accept {
            if spec.listeners.len() != 1
                || spec.listeners.iter().any(|listener| {
                    !matches!(
                        listener.kind,
                        ListenerKind::Stream | ListenerKind::SequentialPacket
                    )
                })
            {
                return Err(UnitError::message(
                    "Accept=yes requires exactly one stream listener",
                ));
            }
        }
        parse_dependencies(self, &mut spec)?;
        parse_conditions(self, &mut spec)?;
        parse_start_limit(self, &mut spec)?;
        Ok(spec)
    }
}

pub fn parse_systemd_service(
    source: &str,
    name: impl Into<String>,
) -> Result<ServiceSpec, UnitError> {
    UnitFile::parse(source)?.to_service_spec(name)
}

pub fn parse_openrc_script(
    source: &str,
    name: impl Into<String>,
    script: impl Into<PathBuf>,
) -> Result<ServiceSpec, UnitError> {
    let name = name.into();
    let script = script.into();
    if name.trim().is_empty() {
        return Err(UnitError::message("OpenRC script has no service name"));
    }

    let mut spec = ServiceSpec::new(name, "/bin/sh");
    spec.args = vec![
        OsString::from("-c"),
        OsString::from(
            "\
script=\"$0\"; action=\"$1\"; \"$script\" \"$action\"; status=$?; \
if [ \"$status\" -ne 0 ]; then exit \"$status\"; fi; \
trap 'exit 0' TERM INT; while :; do sleep 86400; done",
        ),
        script.as_os_str().to_owned(),
        OsString::from("start"),
    ];
    spec.main_expand_environment = false;
    spec.service_type = ServiceType::Simple;
    spec.remain_after_exit = true;
    let mut stop = CommandSpec::new("/bin/sh");
    stop.args = vec![script.as_os_str().to_owned(), OsString::from("stop")];
    spec.stop = Some(stop);
    if has_openrc_function(source, "reload") || has_openrc_case_action(source, "reload") {
        let mut reload = CommandSpec::new("/bin/sh");
        reload.args = vec![script.as_os_str().to_owned(), OsString::from("reload")];
        spec.reload = Some(reload);
    }
    parse_openrc_dependencies(source, &mut spec.dependencies);
    Ok(spec)
}

fn required_value(file: &UnitFile, section: &str, key: &str) -> Result<String, UnitError> {
    file.value(section, key)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| UnitError::message(format!("{section} unit has no {key} directive")))
}

fn parse_dependencies(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    if let Some(value) = file.value("Unit", "DefaultDependencies") {
        spec.default_dependencies = parse_bool(value, "DefaultDependencies")?;
    }
    if let Some(value) = file.value("Unit", "AllowIsolate") {
        spec.allow_isolate = parse_bool(value, "AllowIsolate")?;
    }
    if let Some(value) = file.value("Unit", "IgnoreOnIsolate") {
        spec.ignore_on_isolate = parse_bool(value, "IgnoreOnIsolate")?;
    }
    for (key, destination) in [
        ("Requires", &mut spec.dependencies.requires),
        ("Wants", &mut spec.dependencies.wants),
        ("After", &mut spec.dependencies.after),
        ("Before", &mut spec.dependencies.before),
        ("Conflicts", &mut spec.dependencies.conflicts),
        ("PartOf", &mut spec.dependencies.part_of),
        ("BindsTo", &mut spec.dependencies.binds_to),
        ("Requisite", &mut spec.dependencies.requisite),
        ("OnSuccess", &mut spec.dependencies.on_success),
        ("OnFailure", &mut spec.dependencies.on_failure),
    ] {
        for directive in file.effective_directives("Unit", key) {
            for dependency in split_unit_names(&directive.value, directive.line)? {
                destination.insert(dependency);
            }
        }
    }
    for directive in file.effective_directives("Unit", "RequiresMountsFor") {
        for path in split_words(&directive.value, directive.line)? {
            if !path.starts_with('/') && !path.starts_with('%') {
                return Err(UnitError::at(
                    directive.line,
                    format!("RequiresMountsFor path is not absolute: {path}"),
                ));
            }
            spec.requires_mounts_for.push(PathBuf::from(path));
        }
    }
    for directive in file.effective_directives("Unit", "WantsMountsFor") {
        for path in split_words(&directive.value, directive.line)? {
            if !path.starts_with('/') && !path.starts_with('%') {
                return Err(UnitError::at(
                    directive.line,
                    format!("WantsMountsFor path is not absolute: {path}"),
                ));
            }
            spec.wants_mounts_for.push(PathBuf::from(path));
        }
    }
    if let Some(value) = file.value("Unit", "StopWhenUnneeded") {
        spec.stop_when_unneeded = parse_bool(value, "StopWhenUnneeded")?;
    }
    if let Some(value) = file.value("Unit", "JobTimeoutSec") {
        let timeout = parse_duration(value, "JobTimeoutSec")?;
        spec.job_timeout = (!timeout.is_zero()).then_some(timeout);
    }
    if let Some(value) = file.value("Unit", "JobTimeoutAction") {
        spec.job_timeout_action = parse_manager_action(value, "JobTimeoutAction")?;
    }
    if let Some(value) = file.value("Unit", "FailureAction") {
        spec.failure_action = parse_manager_action(value, "FailureAction")?;
    }
    if let Some(value) = file.value("Unit", "SuccessAction") {
        spec.success_action = parse_manager_action(value, "SuccessAction")?;
    }
    Ok(())
}

fn parse_start_limit(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    let interval = file
        .value("Unit", "StartLimitIntervalSec")
        .or_else(|| file.value("Unit", "StartLimitInterval"))
        .map(|value| parse_duration(value, "StartLimitIntervalSec"))
        .transpose()?;
    let burst = file
        .value("Unit", "StartLimitBurst")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| UnitError::message(format!("invalid StartLimitBurst={value}")))
        })
        .transpose()?;
    spec.start_limit_interval = interval;
    spec.start_limit_burst = burst;
    Ok(())
}

fn parse_directories(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    for (key, mode_key, preserve_key, kind) in [
        (
            "ConfigurationDirectory",
            "ConfigurationDirectoryMode",
            "",
            DirectoryKind::Configuration,
        ),
        (
            "RuntimeDirectory",
            "RuntimeDirectoryMode",
            "RuntimeDirectoryPreserve",
            DirectoryKind::Runtime,
        ),
        (
            "StateDirectory",
            "StateDirectoryMode",
            "StateDirectoryPreserve",
            DirectoryKind::State,
        ),
        (
            "CacheDirectory",
            "CacheDirectoryMode",
            "CacheDirectoryPreserve",
            DirectoryKind::Cache,
        ),
        (
            "LogsDirectory",
            "LogsDirectoryMode",
            "LogsDirectoryPreserve",
            DirectoryKind::Logs,
        ),
    ] {
        let mode = file
            .value("Service", mode_key)
            .map(|value| parse_mode(value, mode_key))
            .transpose()?
            .unwrap_or(0o755);
        let preserve = if preserve_key.is_empty() {
            false
        } else {
            file.value("Service", preserve_key)
                .map(|value| parse_directory_preserve(value, preserve_key))
                .transpose()?
                .unwrap_or(false)
        };
        for directive in file.effective_directives("Service", key) {
            for value in split_words(&directive.value, directive.line)? {
                let path = PathBuf::from(&value);
                if path.is_absolute()
                    || path
                        .components()
                        .any(|component| matches!(component, std::path::Component::ParentDir))
                {
                    return Err(UnitError::at(
                        directive.line,
                        format!("{key} must contain relative paths without '..'"),
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

fn parse_credentials(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    let mut names = BTreeSet::new();
    for directive in file.effective_directives("Service", "LoadCredential") {
        for entry in split_words(&directive.value, directive.line)? {
            let (name, path) = entry
                .split_once(':')
                .map_or((entry.as_str(), None), |(name, path)| (name, Some(path)));
            validate_credential_name(name, directive.line)?;
            if path.is_some_and(str::is_empty) {
                return Err(UnitError::at(
                    directive.line,
                    "LoadCredential contains an empty source path",
                ));
            }
            if !names.insert(name.to_owned()) {
                return Err(UnitError::at(
                    directive.line,
                    format!("credential {name} is declared more than once"),
                ));
            }
            let source = match path {
                None => CredentialSource::Store(name.to_owned()),
                Some(path) if is_valid_credential_name_template(path) => {
                    CredentialSource::Store(path.to_owned())
                }
                Some(path) => CredentialSource::File(PathBuf::from(path)),
            };
            spec.credentials.push(CredentialSpec {
                name: name.to_owned(),
                source,
            });
        }
    }
    for directive in file.effective_directives("Service", "SetCredential") {
        for entry in split_words(&directive.value, directive.line)? {
            let (name, value) = entry.split_once(':').ok_or_else(|| {
                UnitError::at(directive.line, "SetCredential entries must use NAME:VALUE")
            })?;
            validate_credential_name(name, directive.line)?;
            if !names.insert(name.to_owned()) {
                return Err(UnitError::at(
                    directive.line,
                    format!("credential {name} is declared more than once"),
                ));
            }
            spec.credentials.push(CredentialSpec {
                name: name.to_owned(),
                source: CredentialSource::Value(value.as_bytes().to_vec()),
            });
        }
    }
    for directive in file.effective_directives("Service", "ImportCredential") {
        for entry in split_words(&directive.value, directive.line)? {
            let (pattern, rename) = entry
                .split_once(':')
                .map_or((entry.as_str(), None), |(pattern, rename)| {
                    (pattern, Some(rename))
                });
            validate_credential_import_pattern(pattern, directive.line)?;
            if let Some(rename) = rename {
                validate_credential_import_rename(rename, directive.line)?;
            }
            spec.credential_imports.push(CredentialImportSpec {
                pattern: pattern.to_owned(),
                rename: rename.map(str::to_owned),
            });
        }
    }
    Ok(())
}

fn validate_credential_name(value: &str, line: usize) -> Result<(), UnitError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value == ".fractald-owner"
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(UnitError::at(
            line,
            format!("invalid credential name {value}"),
        ));
    }
    Ok(())
}

fn validate_credential_import_pattern(value: &str, line: usize) -> Result<(), UnitError> {
    let wildcard_count = value.bytes().filter(|byte| *byte == b'*').count();
    let valid = wildcard_count <= 1
        && !value.bytes().any(|byte| matches!(byte, b'?' | b'[' | b']'))
        && if let Some(prefix) = value.strip_suffix('*') {
            prefix.is_empty() || is_valid_credential_name_template(prefix)
        } else {
            wildcard_count == 0 && is_valid_credential_name_template(value)
        };
    if !valid {
        return Err(UnitError::at(
            line,
            format!("invalid ImportCredential pattern {value}"),
        ));
    }
    Ok(())
}

fn validate_credential_import_rename(value: &str, line: usize) -> Result<(), UnitError> {
    if !is_valid_credential_name_template(value) || value.contains('*') {
        return Err(UnitError::at(
            line,
            format!("invalid ImportCredential rename {value}"),
        ));
    }
    Ok(())
}

fn is_valid_credential_name_template(value: &str) -> bool {
    if value.is_empty() || value == "." || value == ".." || value == ".fractald-owner" {
        return false;
    }
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '%' {
            let Some(specifier) = characters.next() else {
                return false;
            };
            if !matches!(
                specifier,
                '%' | 'n' | 'N' | 'p' | 'P' | 'i' | 'I' | 'f' | 'u' | 'U' | 'v' | 'b' | 'm' | 'H'
            ) {
                return false;
            }
        } else if !character.is_ascii_alphanumeric() && !matches!(character, '_' | '-' | '.') {
            return false;
        }
    }
    true
}

fn parse_mode(value: &str, key: &str) -> Result<u32, UnitError> {
    let value = value.trim().strip_prefix("0o").unwrap_or(value.trim());
    let mode = u32::from_str_radix(value, 8)
        .map_err(|_| UnitError::message(format!("invalid {key}={value}")))?;
    if mode > 0o7777 {
        return Err(UnitError::message(format!("invalid {key}={value}")));
    }
    Ok(mode)
}

fn parse_directory_preserve(value: &str, key: &str) -> Result<bool, UnitError> {
    match value {
        "yes" | "true" | "on" | "1" | "restart" => Ok(true),
        "no" | "false" | "off" | "0" => Ok(false),
        other => Err(UnitError::message(format!("invalid {key}={other}"))),
    }
}

fn parse_slice(value: &str, key: &str) -> Result<String, UnitError> {
    let value = value.trim();
    if value.is_empty()
        || (!value.ends_with(".slice") && value != "-.slice")
        || value.contains('/')
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(UnitError::message(format!("invalid {key}={value}")));
    }
    Ok(value.to_owned())
}

fn parse_output_mode(value: &str, key: &str) -> Result<OutputMode, UnitError> {
    match value {
        "journal" | "journal-or-kmsg" | "kmsg" | "journal+console" | "kmsg+console" => {
            Ok(OutputMode::Journal)
        }
        "null" => Ok(OutputMode::Null),
        "inherit" => Ok(OutputMode::Inherit),
        "tty" => Ok(OutputMode::Tty),
        "socket" => Ok(OutputMode::Socket),
        value if value.starts_with("file:") || value.starts_with("append:") => {
            let append = value.starts_with("append:");
            let path = value
                .split_once(':')
                .map(|(_, path)| path)
                .unwrap_or_default();
            if path.is_empty() || !path.starts_with('/') {
                return Err(UnitError::message(format!("invalid {key}={value}")));
            }
            Ok(OutputMode::File {
                path: PathBuf::from(path),
                append,
            })
        }
        value if value.starts_with("truncate:") => {
            let path = value.strip_prefix("truncate:").unwrap_or_default();
            if path.is_empty() || !path.starts_with('/') {
                return Err(UnitError::message(format!("invalid {key}={value}")));
            }
            Ok(OutputMode::File {
                path: PathBuf::from(path),
                append: false,
            })
        }
        other => Err(UnitError::message(format!("unsupported {key}={other}"))),
    }
}

fn parse_input_mode(value: &str, key: &str) -> Result<InputMode, UnitError> {
    match value {
        "null" | "data" => Ok(InputMode::Null),
        "inherit" => Ok(InputMode::Inherit),
        "tty" | "tty-force" => Ok(InputMode::Tty),
        "socket" => Ok(InputMode::Socket),
        value if value.starts_with("file:") => {
            let path = value.strip_prefix("file:").unwrap_or_default();
            if path.is_empty() || !path.starts_with('/') {
                return Err(UnitError::message(format!("invalid {key}={value}")));
            }
            Ok(InputMode::File(PathBuf::from(path)))
        }
        other => Err(UnitError::message(format!("unsupported {key}={other}"))),
    }
}

fn parse_conditions(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    for (key, kind) in [
        ("ConditionPathExists", 0_u8),
        ("ConditionPathExistsGlob", 1_u8),
        ("ConditionDirectoryNotEmpty", 2_u8),
        ("ConditionFileIsExecutable", 3_u8),
        ("ConditionPathIsReadWrite", 4_u8),
        ("ConditionPathIsDirectory", 5_u8),
        ("ConditionFileNotEmpty", 6_u8),
        ("ConditionPathIsMountPoint", 7_u8),
        ("ConditionPathIsSymbolicLink", 8_u8),
        ("ConditionKernelCommandLine", 9_u8),
        ("ConditionVirtualization", 10_u8),
        ("ConditionSecurity", 11_u8),
        ("ConditionACPower", 12_u8),
        ("ConditionCapability", 13_u8),
        ("ConditionKernelModuleLoaded", 14_u8),
        ("ConditionFirmware", 15_u8),
        ("ConditionFirstBoot", 16_u8),
        ("ConditionCredential", 17_u8),
        ("ConditionControlGroupController", 18_u8),
        ("ConditionEnvironment", 19_u8),
        ("ConditionNeedsUpdate", 20_u8),
    ] {
        for directive in file.effective_directives("Unit", key) {
            for value in split_words(&directive.value, directive.line)? {
                let (alternative, negate, value) = condition_prefix(&value);
                if value.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        format!("{key} contains an empty value"),
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
                        let (name, expected) =
                            parse_environment_condition(value, key, directive.line)?;
                        Condition::Environment {
                            name,
                            value: expected,
                            negate,
                        }
                    }
                    _ => Condition::NeedsUpdate {
                        path: parse_needs_update_path(value, key, directive.line)?,
                        negate,
                    },
                };
                push_condition(&mut spec.conditions, condition, alternative);
            }
        }
    }
    parse_assertions(file, spec)?;
    Ok(())
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
            break;
        }
    }
    (alternative, negate, value)
}

fn push_condition(destination: &mut Vec<Condition>, condition: Condition, alternative: bool) {
    if !alternative {
        destination.push(condition);
        return;
    }
    match destination.pop() {
        Some(Condition::Any(mut conditions)) => {
            conditions.push(condition);
            destination.push(Condition::Any(conditions));
        }
        Some(previous) => destination.push(Condition::Any(vec![previous, condition])),
        None => destination.push(condition),
    }
}

fn parse_assertions(file: &UnitFile, spec: &mut ServiceSpec) -> Result<(), UnitError> {
    for (key, kind) in [
        ("AssertPathExists", 0_u8),
        ("AssertPathExistsGlob", 1_u8),
        ("AssertDirectoryNotEmpty", 2_u8),
        ("AssertFileIsExecutable", 3_u8),
        ("AssertPathIsReadWrite", 4_u8),
        ("AssertPathIsDirectory", 5_u8),
        ("AssertFileNotEmpty", 6_u8),
        ("AssertPathIsMountPoint", 7_u8),
        ("AssertPathIsSymbolicLink", 8_u8),
        ("AssertKernelCommandLine", 9_u8),
        ("AssertVirtualization", 10_u8),
        ("AssertSecurity", 11_u8),
        ("AssertACPower", 12_u8),
        ("AssertCapability", 13_u8),
        ("AssertKernelModuleLoaded", 14_u8),
        ("AssertFirmware", 15_u8),
        ("AssertFirstBoot", 16_u8),
        ("AssertCredential", 17_u8),
        ("AssertControlGroupController", 18_u8),
        ("AssertEnvironment", 19_u8),
        ("AssertNeedsUpdate", 20_u8),
    ] {
        for directive in file.effective_directives("Unit", key) {
            for value in split_words(&directive.value, directive.line)? {
                let (alternative, negate, value) = condition_prefix(&value);
                if value.is_empty() {
                    return Err(UnitError::at(
                        directive.line,
                        format!("{key} contains an empty value"),
                    ));
                }
                let assertion = match kind {
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
                        let (name, expected) =
                            parse_environment_condition(value, key, directive.line)?;
                        Condition::Environment {
                            name,
                            value: expected,
                            negate,
                        }
                    }
                    _ => Condition::NeedsUpdate {
                        path: parse_needs_update_path(value, key, directive.line)?,
                        negate,
                    },
                };
                push_condition(&mut spec.assertions, assertion, alternative);
            }
        }
    }
    Ok(())
}

fn normalize_listener_address(address: &str) -> String {
    address
        .parse::<u16>()
        .map_or_else(|_| address.to_owned(), |port| format!("0.0.0.0:{port}"))
}

fn has_openrc_function(source: &str, function: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(&format!("{function}()")) || line.starts_with(&format!("{function} ()"))
    })
}

fn has_openrc_case_action(source: &str, action: &str) -> bool {
    source.lines().any(|line| {
        let line = line.trim_start();
        line.starts_with(&format!("{action})")) || line.starts_with(&format!("{action} )"))
    })
}

fn parse_openrc_dependencies(source: &str, dependencies: &mut fractald_core::DependencySet) {
    let mut in_depend = false;
    let mut depth = 0_i32;
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if !in_depend {
            if line.starts_with("depend()") || line.starts_with("depend ()") {
                in_depend = true;
                depth = brace_delta(line);
                if depth == 0 {
                    depth = 1;
                }
            }
            continue;
        }

        let mut fields = line.split_whitespace();
        let Some(kind) = fields.next() else {
            depth += brace_delta(line);
            if depth <= 0 {
                break;
            }
            continue;
        };
        let destination = match kind {
            "need" => Some(&mut dependencies.requires),
            "use" => Some(&mut dependencies.wants),
            "after" => Some(&mut dependencies.after),
            "before" => Some(&mut dependencies.before),
            _ => None,
        };
        if let Some(destination) = destination {
            for dependency in fields {
                let dependency =
                    dependency.trim_matches(|character: char| matches!(character, ';' | '{' | '}'));
                if !dependency.is_empty() {
                    destination.insert(dependency.to_owned());
                }
            }
        }
        depth += brace_delta(line);
        if depth <= 0 {
            break;
        }
    }
}

fn brace_delta(value: &str) -> i32 {
    value.chars().fold(0, |depth, character| match character {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

fn is_reset_directive(directive: &Directive) -> bool {
    matches!(
        (directive.section.as_str(), directive.key.as_str()),
        ("Service", "ExecStart")
            | ("Service", "ExecStartPre")
            | ("Service", "ExecStartPost")
            | ("Service", "ExecStop")
            | ("Service", "ExecStopPost")
            | ("Service", "ExecReload")
            | ("Service", "ExecCondition")
            | ("Service", "BusName")
            | ("Service", "Environment")
            | ("Service", "EnvironmentFile")
            | ("Service", "LoadCredential")
            | ("Service", "AmbientCapabilities")
            | ("Service", "SetCredential")
            | ("Service", "ImportCredential")
            | ("Service", "UnsetEnvironment")
            | ("Service", "SupplementaryGroups")
            | ("Service", "DeviceAllow")
            | ("Service", "ConfigurationDirectory")
            | ("Service", "RuntimeDirectory")
            | ("Service", "StateDirectory")
            | ("Service", "CacheDirectory")
            | ("Service", "LogsDirectory")
            | ("Service", "ReadWritePaths")
            | ("Service", "ReadOnlyPaths")
            | ("Service", "InaccessiblePaths")
            | ("Socket", "FileDescriptorName")
            | ("Service", "RestrictNamespaces")
            | ("Service", "SystemCallFilter")
            | ("Service", "SystemCallArchitectures")
            | ("Service", "SystemCallErrorNumber")
            | ("Service", "SuccessExitStatus")
            | ("Service", "RestartPreventExitStatus")
            | ("Install", "Alias")
            | ("Unit", "Requires")
            | ("Unit", "Wants")
            | ("Unit", "After")
            | ("Unit", "Before")
            | ("Unit", "Conflicts")
            | ("Unit", "PartOf")
            | ("Unit", "BindsTo")
            | ("Unit", "Requisite")
            | ("Unit", "OnSuccess")
            | ("Unit", "OnFailure")
            | ("Unit", "RequiresMountsFor")
            | ("Unit", "WantsMountsFor")
            | ("Unit", "ConditionPathExists")
            | ("Unit", "ConditionPathExistsGlob")
            | ("Unit", "ConditionDirectoryNotEmpty")
            | ("Unit", "ConditionFileIsExecutable")
            | ("Unit", "ConditionPathIsReadWrite")
            | ("Unit", "ConditionPathIsDirectory")
            | ("Unit", "ConditionFileNotEmpty")
            | ("Unit", "ConditionPathIsMountPoint")
            | ("Unit", "ConditionPathIsSymbolicLink")
            | ("Unit", "ConditionKernelCommandLine")
            | ("Unit", "ConditionVirtualization")
            | ("Unit", "ConditionSecurity")
            | ("Unit", "ConditionACPower")
            | ("Unit", "ConditionCapability")
            | ("Unit", "ConditionKernelModuleLoaded")
            | ("Unit", "ConditionFirmware")
            | ("Unit", "ConditionFirstBoot")
            | ("Unit", "ConditionCredential")
            | ("Unit", "ConditionControlGroupController")
            | ("Unit", "AssertPathExists")
            | ("Unit", "AssertPathExistsGlob")
            | ("Unit", "AssertDirectoryNotEmpty")
            | ("Unit", "AssertFileIsExecutable")
            | ("Unit", "AssertPathIsReadWrite")
            | ("Unit", "AssertPathIsDirectory")
            | ("Unit", "AssertFileNotEmpty")
            | ("Unit", "AssertPathIsMountPoint")
            | ("Unit", "AssertPathIsSymbolicLink")
            | ("Unit", "AssertKernelCommandLine")
            | ("Unit", "AssertVirtualization")
            | ("Unit", "AssertSecurity")
            | ("Unit", "AssertACPower")
            | ("Unit", "AssertCapability")
            | ("Unit", "AssertKernelModuleLoaded")
            | ("Unit", "AssertFirmware")
            | ("Unit", "AssertFirstBoot")
            | ("Unit", "AssertCredential")
            | ("Unit", "AssertControlGroupController")
            | ("Socket", "ListenStream")
            | ("Socket", "ListenDatagram")
            | ("Timer", "OnCalendar")
            | ("Path", "PathChanged")
            | ("Path", "PathModified")
            | ("Path", "PathExists")
            | ("Path", "PathExistsGlob")
            | ("Path", "DirectoryNotEmpty")
    )
}

fn parse_command(value: &str, line: usize) -> Result<CommandSpec, UnitError> {
    let mut words = split_words(value, line)?;
    let first = words
        .first_mut()
        .ok_or_else(|| UnitError::at(line, "command is empty"))?;

    let mut prefix_end = 0;
    let mut ignore_failure = false;
    let mut argv0 = false;
    let mut expand_environment = true;
    for (index, character) in first.char_indices() {
        match character {
            '-' => {
                ignore_failure = true;
                prefix_end = index + character.len_utf8();
            }
            '@' => {
                argv0 = true;
                prefix_end = index + character.len_utf8();
            }
            ':' => {
                expand_environment = false;
                prefix_end = index + character.len_utf8();
            }
            '+' | '!' => prefix_end = index + character.len_utf8(),
            _ => break,
        }
    }
    if prefix_end > 0 {
        first.drain(..prefix_end);
    }
    if first.is_empty() {
        return Err(UnitError::at(line, "command has no executable"));
    }
    let program = PathBuf::from(first.clone());
    let (argv0, args_start) = if argv0 {
        let value = words
            .get(1)
            .ok_or_else(|| UnitError::at(line, "@ command prefix requires an argv[0] argument"))?;
        (Some(OsString::from(value)), 2)
    } else {
        (None, 1)
    };
    Ok(CommandSpec {
        program,
        args: words
            .into_iter()
            .skip(args_start)
            .map(OsString::from)
            .collect(),
        ignore_failure,
        argv0,
        expand_environment,
    })
}

fn split_words(value: &str, line: usize) -> Result<Vec<String>, UnitError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    let mut characters = value.chars().peekable();

    while let Some(character) = characters.next() {
        if escaped {
            if character == 'x' {
                let first = characters.next();
                let second = characters.next();
                match (first.and_then(hex_value), second.and_then(hex_value)) {
                    (Some(high), Some(low)) => {
                        current.push(char::from((high << 4) | low));
                    }
                    _ => return Err(UnitError::at(line, "invalid hexadecimal escape")),
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
        if let Some(active_quote) = quote {
            if character == active_quote {
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
        return Err(UnitError::at(line, "command ends with an escape"));
    }
    if quote.is_some() {
        return Err(UnitError::at(line, "command has an unterminated quote"));
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

fn split_unit_names(value: &str, line: usize) -> Result<Vec<String>, UnitError> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    let mut characters = value.chars().peekable();

    while let Some(character) = characters.next() {
        if escaped {
            if character == 'x' {
                let first = characters.next();
                let second = characters.next();
                match (first.and_then(hex_value), second.and_then(hex_value)) {
                    (Some(_), Some(_)) => {
                        current.push('\\');
                        current.push('x');
                        current.push(first.expect("checked first hexadecimal digit"));
                        current.push(second.expect("checked second hexadecimal digit"));
                    }
                    _ => return Err(UnitError::at(line, "invalid hexadecimal escape")),
                }
            } else {
                current.push('\\');
                current.push(character);
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
        if let Some(active_quote) = quote {
            if character == active_quote {
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
        return Err(UnitError::at(line, "unit name ends with an escape"));
    }
    if quote.is_some() {
        return Err(UnitError::at(line, "unit name has an unterminated quote"));
    }
    if started {
        words.push(current);
    }
    Ok(words)
}

fn parse_service_type(value: &str) -> Result<ServiceType, UnitError> {
    match value {
        "simple" | "exec" => Ok(ServiceType::Simple),
        "forking" => Ok(ServiceType::Forking),
        "oneshot" => Ok(ServiceType::Oneshot),
        "notify" | "notify-reload" => Ok(ServiceType::Notify),
        "dbus" => Ok(ServiceType::Dbus),
        "idle" => Ok(ServiceType::Idle),
        other => Err(UnitError::message(format!(
            "unsupported service Type={other}"
        ))),
    }
}

fn valid_bus_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with('.')
        && !value.ends_with('.')
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

fn parse_restart(value: &str) -> Result<RestartPolicy, UnitError> {
    match value {
        "no" | "never" => Ok(RestartPolicy::Never),
        "on-success" => Ok(RestartPolicy::OnSuccess),
        "on-failure" => Ok(RestartPolicy::OnFailure),
        "on-abnormal" | "on-watchdog" => Ok(RestartPolicy::OnAbnormal),
        "on-abort" => Ok(RestartPolicy::OnAbort),
        "always" => Ok(RestartPolicy::Always),
        other => Err(UnitError::message(format!("unsupported Restart={other}"))),
    }
}

fn parse_oom_policy(value: &str) -> Result<OomPolicy, UnitError> {
    match value {
        "continue" => Ok(OomPolicy::Continue),
        "stop" => Ok(OomPolicy::Stop),
        "kill" => Ok(OomPolicy::Kill),
        other => Err(UnitError::message(format!("unsupported OOMPolicy={other}"))),
    }
}

fn parse_kill_mode(value: &str) -> Result<KillMode, UnitError> {
    match value {
        "control-group" => Ok(KillMode::ControlGroup),
        "process" => Ok(KillMode::Process),
        "mixed" => Ok(KillMode::Mixed),
        "none" => Ok(KillMode::None),
        other => Err(UnitError::message(format!("unsupported KillMode={other}"))),
    }
}

fn parse_device_policy(value: &str) -> Result<DevicePolicy, UnitError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" => Ok(DevicePolicy::Auto),
        "closed" => Ok(DevicePolicy::Closed),
        "strict" => Ok(DevicePolicy::Strict),
        other => Err(UnitError::message(format!(
            "unsupported DevicePolicy={other}"
        ))),
    }
}

fn parse_device_access(value: &str, line: usize) -> Result<u8, UnitError> {
    let mut access = 0_u8;
    for character in value.chars() {
        let bit = match character {
            'r' => 2,
            'w' => 4,
            'm' => 1,
            _ => {
                return Err(UnitError::at(
                    line,
                    format!("invalid DeviceAllow access mode {value}"),
                ));
            }
        };
        if access & bit != 0 {
            return Err(UnitError::at(
                line,
                format!("duplicate DeviceAllow access mode {value}"),
            ));
        }
        access |= bit;
    }
    if access == 0 {
        return Err(UnitError::at(
            line,
            "DeviceAllow access mode cannot be empty",
        ));
    }
    Ok(access)
}

fn parse_delegate(value: &str, line: usize) -> Result<DelegateMode, UnitError> {
    let value = value.trim();
    if value.is_empty() || matches!(value.to_ascii_lowercase().as_str(), "yes" | "true") {
        return Ok(DelegateMode::All);
    }
    if matches!(value.to_ascii_lowercase().as_str(), "no" | "false") {
        return Ok(DelegateMode::No);
    }
    let mut controllers = BTreeSet::new();
    for controller in split_words(value, line)? {
        let controller = controller.to_ascii_lowercase();
        if !matches!(
            controller.as_str(),
            "cpu" | "cpuset" | "io" | "memory" | "pids" | "hugetlb" | "rdma" | "misc" | "dmem"
        ) {
            return Err(UnitError::at(
                line,
                format!("unsupported Delegate controller {controller}"),
            ));
        }
        controllers.insert(controller);
    }
    Ok(DelegateMode::Controllers(controllers))
}

fn parse_private_users(value: &str, line: usize) -> Result<PrivateUsersMode, UnitError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "yes" | "true" | "self" => Ok(PrivateUsersMode::SelfMapping),
        "identity" => Ok(PrivateUsersMode::Identity),
        "full" => Ok(PrivateUsersMode::Full),
        "no" | "false" | "" => Ok(PrivateUsersMode::No),
        other => Err(UnitError::at(
            line,
            format!("unsupported PrivateUsers={other}"),
        )),
    }
}

fn parse_capability(value: &str, line: usize) -> Result<u64, UnitError> {
    let capability = value.to_ascii_uppercase();
    let bit = match capability.as_str() {
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
        _ => {
            return Err(UnitError::at(
                line,
                format!("unsupported capability {value}"),
            ));
        }
    };
    Ok(1_u64 << bit)
}

fn parse_address_family(value: &str, line: usize) -> Result<u64, UnitError> {
    let family = value.to_ascii_uppercase();
    let bit = match family.as_str() {
        "AF_UNIX" | "AF_LOCAL" => 1,
        "AF_INET" => 2,
        "AF_INET6" => 10,
        "AF_PACKET" => 17,
        "AF_NETLINK" => 16,
        "AF_ALG" => 38,
        "AF_VSOCK" => 40,
        "AF_QIPCRTR" => 42,
        "NONE" => return Ok(0),
        _ => {
            return Err(UnitError::at(
                line,
                format!("unsupported address family {value}"),
            ));
        }
    };
    Ok(1_u64 << bit)
}

fn parse_system_call_filter(file: &UnitFile) -> Result<Option<SystemCallFilter>, UnitError> {
    let directives = file.directives("Service", "SystemCallFilter");
    if directives.is_empty() {
        return Ok(None);
    }

    let mut default_allow = None;
    let mut rules = BTreeMap::new();
    for directive in directives {
        if directive.value.trim().is_empty() {
            default_allow = None;
            rules.clear();
            continue;
        }
        let mut words = split_words(&directive.value, directive.line)?;
        let first = words.first_mut().ok_or_else(|| {
            UnitError::at(directive.line, "SystemCallFilter contains an empty value")
        })?;
        let deny_list = first.strip_prefix('~').is_some();
        if deny_list {
            *first = first[1..].to_owned();
            if first.is_empty() {
                return Err(UnitError::at(
                    directive.line,
                    "SystemCallFilter deny list is missing a system call",
                ));
            }
        }
        if default_allow.is_none() {
            default_allow = Some(deny_list);
        }
        let list_changes_default = deny_list == default_allow.unwrap();
        for word in words {
            if word.starts_with('~') {
                return Err(UnitError::at(
                    directive.line,
                    "SystemCallFilter may use '~' only on the first value",
                ));
            }
            let (name, errno) = if let Some((name, errno)) = word.split_once(':') {
                (name, Some(parse_errno(errno, "SystemCallFilter")?))
            } else {
                (word.as_str(), None)
            };
            if name.is_empty() {
                return Err(UnitError::at(
                    directive.line,
                    "SystemCallFilter contains an empty system call",
                ));
            }
            let names = if let Some(names) = syscall_groups::expand_group(name) {
                names.to_vec()
            } else {
                if !syscall_groups::is_known_syscall(name) {
                    return Err(UnitError::at(
                        directive.line,
                        format!("unsupported system call {name}"),
                    ));
                }
                vec![name]
            };
            if !deny_list && errno.is_some() {
                return Err(UnitError::at(
                    directive.line,
                    "SystemCallFilter errno suffixes require a deny list",
                ));
            }
            let action = if deny_list {
                errno.map_or(
                    SystemCallRuleAction::Deny,
                    SystemCallRuleAction::DenyWithErrno,
                )
            } else {
                SystemCallRuleAction::Allow
            };
            for name in names {
                if list_changes_default {
                    rules.insert(name.to_owned(), action);
                } else {
                    rules.remove(name);
                }
            }
        }
    }

    let Some(default_allow) = default_allow else {
        return Ok(None);
    };
    Ok(Some(SystemCallFilter {
        default_allow,
        rules: rules
            .into_iter()
            .map(|(name, action)| SystemCallRule { name, action })
            .collect(),
    }))
}

fn parse_system_call_architectures(value: &str) -> Result<SystemCallArchitectures, UnitError> {
    let words = split_words(value, 0)?;
    let mut architecture = None;
    for word in words {
        let candidate = match word.as_str() {
            "native" => SystemCallArchitectures::Native,
            "x86-64" | "x86_64" => SystemCallArchitectures::X86_64,
            "all" => SystemCallArchitectures::All,
            other => {
                return Err(UnitError::message(format!(
                    "unsupported SystemCallArchitectures={other}"
                )));
            }
        };
        if let Some(existing) = architecture {
            if !matches!(
                (existing, candidate),
                (
                    SystemCallArchitectures::Native,
                    SystemCallArchitectures::Native
                ) | (SystemCallArchitectures::All, SystemCallArchitectures::All)
                    | (
                        SystemCallArchitectures::All,
                        SystemCallArchitectures::Native
                    )
                    | (
                        SystemCallArchitectures::Native,
                        SystemCallArchitectures::All
                    )
            ) {
                return Err(UnitError::message(format!(
                    "conflicting SystemCallArchitectures={value}"
                )));
            }
            architecture = Some(if matches!(candidate, SystemCallArchitectures::All) {
                candidate
            } else {
                existing
            });
        } else {
            architecture = Some(candidate);
        }
    }
    architecture.ok_or_else(|| UnitError::message("SystemCallArchitectures is empty"))
}

fn parse_errno(value: &str, key: &str) -> Result<i32, UnitError> {
    let errno = match value {
        "kill" => 0,
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
        "ENOTDIR" => 20,
        "EISDIR" => 21,
        "EINVAL" => 22,
        "ENFILE" => 23,
        "EMFILE" => 24,
        "ENOTTY" => 25,
        "ETXTBSY" => 26,
        "EFBIG" => 27,
        "ENOSPC" => 28,
        "ESPIPE" => 29,
        "EROFS" => 30,
        "EMLINK" => 31,
        "EPIPE" => 32,
        "EDOM" => 33,
        "ERANGE" => 34,
        "EDEADLK" => 35,
        "ENAMETOOLONG" => 36,
        "ENOLCK" => 37,
        "ENOSYS" => 38,
        "ENOTEMPTY" => 39,
        "ELOOP" => 40,
        "ENOMSG" => 42,
        "EIDRM" => 43,
        "ECHRNG" => 44,
        "EL2NSYNC" => 45,
        "EL3HLT" => 46,
        "EL3RST" => 47,
        "ELNRNG" => 48,
        "EUNATCH" => 49,
        "ENOCSI" => 50,
        "EL2HLT" => 51,
        "EBADE" => 52,
        "EBADR" => 53,
        "EXFULL" => 54,
        "ENOANO" => 55,
        "EBADRQC" => 56,
        "EBADSLT" => 57,
        "EDEADLOCK" => 35,
        "EBFONT" => 59,
        "ENOSTR" => 60,
        "ENODATA" => 61,
        "ETIME" => 62,
        "ENOSR" => 63,
        "ENONET" => 64,
        "ENOPKG" => 65,
        "EREMOTE" => 66,
        "ENOLINK" => 67,
        "EADV" => 68,
        "ESRMNT" => 69,
        "ECOMM" => 70,
        "EPROTO" => 71,
        "EMULTIHOP" => 72,
        "EDOTDOT" => 73,
        "EBADMSG" => 74,
        "EOVERFLOW" => 75,
        "ENOTUNIQ" => 76,
        "EBADFD" => 77,
        "EREMCHG" => 78,
        "ELIBACC" => 79,
        "ELIBBAD" => 80,
        "ELIBSCN" => 81,
        "ELIBMAX" => 82,
        "ELIBEXEC" => 83,
        "EILSEQ" => 84,
        "ERESTART" => 85,
        "ESTRPIPE" => 86,
        "EUSERS" => 87,
        "ENOTSOCK" => 88,
        "EDESTADDRREQ" => 89,
        "EMSGSIZE" => 90,
        "EPROTOTYPE" => 91,
        "ENOPROTOOPT" => 92,
        "EPROTONOSUPPORT" => 93,
        "ESOCKTNOSUPPORT" => 94,
        "EOPNOTSUPP" => 95,
        "EPFNOSUPPORT" => 96,
        "EAFNOSUPPORT" => 97,
        "EADDRINUSE" => 98,
        "EADDRNOTAVAIL" => 99,
        "ENETDOWN" => 100,
        "ENETUNREACH" => 101,
        "ENETRESET" => 102,
        "ECONNABORTED" => 103,
        "ECONNRESET" => 104,
        "ENOBUFS" => 105,
        "EISCONN" => 106,
        "ENOTCONN" => 107,
        "ESHUTDOWN" => 108,
        "ETOOMANYREFS" => 109,
        "ETIMEDOUT" => 110,
        "ECONNREFUSED" => 111,
        "EHOSTDOWN" => 112,
        "EHOSTUNREACH" => 113,
        "EALREADY" => 114,
        "EINPROGRESS" => 115,
        "ESTALE" => 116,
        "EUCLEAN" => 117,
        "ENOTNAM" => 118,
        "ENAVAIL" => 119,
        "EISNAM" => 120,
        "EREMOTEIO" => 121,
        "EDQUOT" => 122,
        "ENOMEDIUM" => 123,
        "EMEDIUMTYPE" => 124,
        "ECANCELED" => 125,
        "ENOKEY" => 126,
        "EKEYEXPIRED" => 127,
        "EKEYREVOKED" => 128,
        "EKEYREJECTED" => 129,
        "EOWNERDEAD" => 130,
        "ENOTRECOVERABLE" => 131,
        "ERFKILL" => 132,
        "EHWPOISON" => 133,
        other => other
            .parse::<i32>()
            .ok()
            .filter(|errno| (1..=4095).contains(errno))
            .ok_or_else(|| UnitError::message(format!("invalid {key}={value}")))?,
    };
    Ok(errno)
}

fn parse_bool(value: &str, key: &str) -> Result<bool, UnitError> {
    let normalized = value.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "y" | "yes" | "true" | "on" | "1" => Ok(true),
        "n" | "no" | "false" | "off" | "0" => Ok(false),
        other => Err(UnitError::message(format!("invalid {key}={other}"))),
    }
}

fn canonical_filesystem_type(value: &str) -> String {
    let value = value.to_ascii_lowercase();
    match value.as_str() {
        "fat" | "msdos" => "vfat".to_owned(),
        "ext" => "ext4".to_owned(),
        _ => value,
    }
}

fn parse_manager_action(value: &str, key: &str) -> Result<ManagerAction, UnitError> {
    let normalized = value.trim().to_ascii_lowercase();
    let action = match normalized.as_str() {
        "none" => ManagerAction::None,
        "exit" => ManagerAction::Exit,
        "exit-force" => ManagerAction::Exit,
        "poweroff" => ManagerAction::Poweroff,
        "poweroff-force" | "poweroff-immediate" => ManagerAction::PoweroffForce,
        "reboot" => ManagerAction::Reboot,
        "reboot-force" | "reboot-immediate" => ManagerAction::RebootForce,
        "halt" => ManagerAction::Halt,
        "halt-force" | "halt-immediate" => ManagerAction::HaltForce,
        "kexec" => ManagerAction::Kexec,
        "kexec-force" => ManagerAction::KexecForce,
        "soft-reboot" => ManagerAction::SoftReboot,
        "soft-reboot-force" => ManagerAction::SoftRebootForce,
        _ => {
            return Err(UnitError::message(format!("invalid {key}={value}")));
        }
    };
    Ok(action)
}

fn parse_notify_access(value: &str) -> Result<NotifyAccess, UnitError> {
    match value {
        "none" => Ok(NotifyAccess::None),
        "main" => Ok(NotifyAccess::Main),
        "exec" => Ok(NotifyAccess::Exec),
        "all" => Ok(NotifyAccess::All),
        other => Err(UnitError::message(format!("invalid NotifyAccess={other}"))),
    }
}

fn is_valid_file_descriptor_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && !value.contains(':')
        && !value.chars().any(char::is_control)
}

fn valid_environment_name(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

fn parse_environment_condition(
    value: &str,
    key: &str,
    line: usize,
) -> Result<(String, Option<String>), UnitError> {
    let (name, expected) = value
        .split_once('=')
        .map_or((value, None), |(name, expected)| (name, Some(expected)));
    if !valid_environment_name(name) {
        return Err(UnitError::at(
            line,
            format!("invalid {key} environment name {name}"),
        ));
    }
    Ok((name.to_owned(), expected.map(str::to_owned)))
}

fn parse_needs_update_path(value: &str, key: &str, line: usize) -> Result<PathBuf, UnitError> {
    match value {
        "/etc" | "/var" => Ok(PathBuf::from(value)),
        _ => Err(UnitError::at(line, format!("{key} must be /etc or /var"))),
    }
}

fn parse_private_tmp(value: &str) -> Result<PrivateTmpMode, UnitError> {
    match value {
        "yes" | "true" | "on" | "1" => Ok(PrivateTmpMode::Yes),
        "no" | "false" | "off" | "0" => Ok(PrivateTmpMode::No),
        "disconnected" => Ok(PrivateTmpMode::Disconnected),
        other => Err(UnitError::message(format!("invalid PrivateTmp={other}"))),
    }
}

fn parse_protect_system(value: &str) -> Result<ProtectSystemMode, UnitError> {
    match value {
        "no" | "false" | "off" | "0" => Ok(ProtectSystemMode::No),
        "yes" | "true" | "on" | "1" => Ok(ProtectSystemMode::Yes),
        "full" => Ok(ProtectSystemMode::Full),
        "strict" => Ok(ProtectSystemMode::Strict),
        other => Err(UnitError::message(format!("invalid ProtectSystem={other}"))),
    }
}

fn parse_protect_home(value: &str) -> Result<ProtectHomeMode, UnitError> {
    match value {
        "no" | "false" | "off" | "0" => Ok(ProtectHomeMode::No),
        "yes" | "true" | "on" | "1" => Ok(ProtectHomeMode::Yes),
        "read-only" => Ok(ProtectHomeMode::ReadOnly),
        "tmpfs" => Ok(ProtectHomeMode::Tmpfs),
        other => Err(UnitError::message(format!("invalid ProtectHome={other}"))),
    }
}

fn parse_protect_proc(value: &str) -> Result<ProtectProcMode, UnitError> {
    match value {
        "default" => Ok(ProtectProcMode::Default),
        "noaccess" => Ok(ProtectProcMode::NoAccess),
        "invisible" => Ok(ProtectProcMode::Invisible),
        "ptraceable" => Ok(ProtectProcMode::Ptraceable),
        other => Err(UnitError::message(format!("invalid ProtectProc={other}"))),
    }
}

fn parse_proc_subset(value: &str) -> Result<ProcSubsetMode, UnitError> {
    match value {
        "all" => Ok(ProcSubsetMode::All),
        "pid" => Ok(ProcSubsetMode::Pid),
        other => Err(UnitError::message(format!("invalid ProcSubset={other}"))),
    }
}

fn parse_restrict_namespaces(directives: Vec<&Directive>) -> Result<u32, UnitError> {
    let mut allowed = None;
    for directive in directives {
        let value = directive.value.trim();
        if value.is_empty() || matches!(value, "no" | "false" | "off" | "0") {
            allowed = Some(ALL_LINUX_NAMESPACE_FLAGS);
            continue;
        }
        if matches!(value, "yes" | "true" | "on" | "1") {
            allowed = Some(0);
            continue;
        }
        let mut words = split_words(value, directive.line)?;
        let first = words.first_mut().ok_or_else(|| {
            UnitError::at(directive.line, "RestrictNamespaces contains an empty value")
        })?;
        let inverted = first.strip_prefix('~').is_some();
        if inverted {
            *first = first[1..].to_owned();
            if first.is_empty() {
                return Err(UnitError::at(
                    directive.line,
                    "RestrictNamespaces deny list is missing a namespace",
                ));
            }
        }
        let mut flags = 0_u32;
        for word in words {
            if word.starts_with('~') {
                return Err(UnitError::at(
                    directive.line,
                    "RestrictNamespaces may use '~' only on the first value",
                ));
            }
            flags |= parse_namespace_flag(&word, directive.line)?;
        }
        let current = allowed.unwrap_or(if inverted {
            ALL_LINUX_NAMESPACE_FLAGS
        } else {
            0
        });
        allowed = Some(if inverted {
            current & !flags
        } else {
            current | flags
        });
    }
    allowed.ok_or_else(|| UnitError::message("RestrictNamespaces has no policy"))
}

fn parse_namespace_flag(value: &str, line: usize) -> Result<u32, UnitError> {
    let flag = match value {
        "cgroup" => 0x0200_0000,
        "ipc" => 0x0800_0000,
        "net" => 0x4000_0000,
        "mnt" => 0x0002_0000,
        "pid" => 0x2000_0000,
        "time" => 0x0000_0080,
        "user" => 0x1000_0000,
        "uts" => 0x0400_0000,
        _ => {
            return Err(UnitError::at(
                line,
                format!("unsupported namespace type {value}"),
            ));
        }
    };
    Ok(flag)
}

fn parse_signal(value: &str) -> Result<i32, UnitError> {
    let normalized = value.strip_prefix("SIG").unwrap_or(value);
    let signal = match normalized {
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
        "STKFLT" => 16,
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
            .map_err(|_| UnitError::message(format!("invalid KillSignal={value}")))?,
    };
    if signal <= 0 {
        return Err(UnitError::message(format!("invalid KillSignal={value}")));
    }
    Ok(signal)
}

fn parse_exit_status(value: &str, key: &str) -> Result<i32, UnitError> {
    if value.starts_with("SIG") {
        return parse_signal(value)
            .map_err(|error| UnitError::message(format!("invalid {key}={value}: {error}")));
    }
    if value
        .chars()
        .all(|character| character.is_ascii_alphabetic())
    {
        if let Some(status) = sysexits_status(value) {
            return Ok(status);
        }
        return parse_signal(value)
            .map_err(|error| UnitError::message(format!("invalid {key}={value}: {error}")));
    }
    let status = value
        .parse::<i32>()
        .map_err(|_| UnitError::message(format!("invalid {key}={value}")))?;
    if !(0..=255).contains(&status) {
        return Err(UnitError::message(format!("invalid {key}={value}")));
    }
    Ok(status)
}

fn sysexits_status(value: &str) -> Option<i32> {
    let value = value.strip_prefix("EX_").unwrap_or(value);
    Some(match value {
        "OK" => 0,
        "USAGE" => 64,
        "DATAERR" => 65,
        "NOINPUT" => 66,
        "NOUSER" => 67,
        "NOHOST" => 68,
        "UNAVAILABLE" => 69,
        "SOFTWARE" => 70,
        "OSERR" => 71,
        "OSFILE" => 72,
        "CANTCREAT" => 73,
        "IOERR" => 74,
        "TEMPFAIL" => 75,
        "PROTOCOL" => 76,
        "NOPERM" => 77,
        "CONFIG" => 78,
        _ => return None,
    })
}

fn parse_duration(value: &str, key: &str) -> Result<Duration, UnitError> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("infinity") {
        return Ok(Duration::MAX);
    }

    let suffixes = [
        ("min", 60.0),
        ("us", 1e-6),
        ("µs", 1e-6),
        ("ms", 1e-3),
        ("sec", 1.0),
        ("s", 1.0),
        ("m", 60.0),
        ("h", 3_600.0),
        ("d", 86_400.0),
        ("w", 604_800.0),
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
        .map_err(|_| UnitError::message(format!("invalid {key}={value}")))?;
    let nanos = number * multiplier * 1_000_000_000.0;
    if !nanos.is_finite() || nanos < 0.0 || nanos > u64::MAX as f64 {
        return Err(UnitError::message(format!("invalid {key}={value}")));
    }
    Ok(Duration::from_nanos(nanos.round() as u64))
}

fn parse_limit(value: &str, key: &str, bytes: bool) -> Result<LimitValue, UnitError> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("infinity") || value.eq_ignore_ascii_case("max") {
        return Ok(LimitValue::Max);
    }
    let (number, multiplier) = if bytes {
        let suffixes = [
            ("KiB", 1_u64 << 10),
            ("MiB", 1_u64 << 20),
            ("GiB", 1_u64 << 30),
            ("TiB", 1_u64 << 40),
            ("KB", 1_000),
            ("MB", 1_000_000),
            ("GB", 1_000_000_000),
            ("TB", 1_000_000_000_000),
            ("K", 1_u64 << 10),
            ("M", 1_u64 << 20),
            ("G", 1_u64 << 30),
            ("T", 1_u64 << 40),
        ];
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
        .map_err(|_| UnitError::message(format!("invalid {key}={value}")))?;
    let value = number
        .checked_mul(multiplier)
        .ok_or_else(|| UnitError::message(format!("invalid {key}={value}")))?;
    Ok(LimitValue::Value(value))
}

fn parse_cpu_quota(unit: &UnitFile, section: &str) -> Result<Option<CpuQuota>, UnitError> {
    const DEFAULT_PERIOD: Duration = Duration::from_millis(100);
    const MIN_PERIOD: Duration = Duration::from_millis(1);
    const MAX_PERIOD: Duration = Duration::from_secs(1);

    let period = unit
        .value(section, "CPUQuotaPeriodSec")
        .filter(|value| !value.trim().is_empty())
        .map(|value| parse_duration(value, "CPUQuotaPeriodSec"))
        .transpose()?
        .unwrap_or(DEFAULT_PERIOD);
    if !(MIN_PERIOD..=MAX_PERIOD).contains(&period) {
        return Err(UnitError::message(format!(
            "CPUQuotaPeriodSec must be between 1ms and 1s (got {})",
            period.as_micros()
        )));
    }
    let period_usec = u64::try_from(period.as_micros())
        .map_err(|_| UnitError::message("CPUQuotaPeriodSec is too large"))?;
    let Some(value) = unit.value(section, "CPUQuota") else {
        return Ok(None);
    };
    parse_cpu_quota_value(value, period_usec)
}

fn parse_cpu_quota_value(value: &str, period_usec: u64) -> Result<Option<CpuQuota>, UnitError> {
    const FRACTION_SCALE: u64 = 1_000_000;
    const PERCENT_SCALE: u128 = 100 * FRACTION_SCALE as u128;
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("infinity") {
        return Ok(None);
    }
    let number = value
        .strip_suffix('%')
        .ok_or_else(|| UnitError::message(format!("CPUQuota must be a percentage: {value}")))?;
    let (whole, fraction) = number
        .split_once('.')
        .map_or((number, ""), |(left, right)| (left, right));
    if whole.is_empty() && fraction.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 6
    {
        return Err(UnitError::message(format!("invalid CPUQuota={value}")));
    }
    let whole = whole
        .parse::<u64>()
        .map_err(|_| UnitError::message(format!("invalid CPUQuota={value}")))?;
    let fraction_value = if fraction.is_empty() {
        0
    } else {
        let parsed = fraction
            .parse::<u64>()
            .map_err(|_| UnitError::message(format!("invalid CPUQuota={value}")))?;
        parsed * 10_u64.pow(6 - fraction.len() as u32)
    };
    let percentage_micro = whole
        .checked_mul(FRACTION_SCALE)
        .and_then(|value| value.checked_add(fraction_value))
        .ok_or_else(|| UnitError::message(format!("invalid CPUQuota={value}")))?;
    if percentage_micro == 0 {
        return Ok(None);
    }
    let quota_usec =
        (u128::from(period_usec) * u128::from(percentage_micro)).div_ceil(PERCENT_SCALE);
    let quota_usec = u64::try_from(quota_usec)
        .map_err(|_| UnitError::message(format!("invalid CPUQuota={value}")))?;
    Ok(Some(CpuQuota {
        quota_usec,
        period_usec,
    }))
}

fn parse_limit_range(value: &str, key: &str, bytes: bool) -> Result<LimitRange, UnitError> {
    let (soft, hard) = value
        .split_once(':')
        .map_or((value, value), |(soft, hard)| (soft, hard));
    let soft = parse_limit(soft, key, bytes)?;
    let hard = parse_limit(hard, key, bytes)?;
    let invalid_order = match (soft, hard) {
        (LimitValue::Max, LimitValue::Value(_)) => true,
        (LimitValue::Value(soft), LimitValue::Value(hard)) => soft > hard,
        _ => false,
    };
    if invalid_order {
        return Err(UnitError::message(format!("invalid {key}={value}")));
    }
    Ok(LimitRange { soft, hard })
}

fn hex_value(character: char) -> Option<u8> {
    character.to_digit(16).map(|value| value as u8)
}

fn has_continuation(line: &str) -> bool {
    let mut backslashes = 0;
    for character in line.chars().rev() {
        if character == '\\' {
            backslashes += 1;
        } else {
            break;
        }
    }
    backslashes % 2 == 1
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitError {
    pub line: Option<usize>,
    pub message: String,
}

impl UnitError {
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

impl fmt::Display for UnitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(formatter, "line {line}: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for UnitError {}

#[cfg(test)]
mod tests {
    use super::*;
    use fractald_core::{
        CredentialSource, DelegateMode, DevicePolicy, DirectoryKind, InputMode, ManagerAction,
        NotifyAccess, OomPolicy, OutputMode, PathWatch, PrivateTmpMode, PrivateUsersMode,
        ProcSubsetMode, ProtectHomeMode, ProtectProcMode, ProtectSystemMode, RestartPolicy,
        ServiceType, TriggerSpec,
    };

    #[test]
    fn parses_common_service_directives() {
        let source = r#"
[Unit]
Description=Example service
DefaultDependencies=no
AllowIsolate=YES
IgnoreOnIsolate=True
Requires=network.target
Wants=metrics.service
After=network.target
Before=metrics.service
Conflicts=shutdown.target
PartOf=app.target
BindsTo=mount.service
Requisite=network.target
OnFailure=recovery.service
OnSuccess=cleanup.service
RequiresMountsFor=/var/lib/example /run/example
WantsMountsFor=/var/cache/example
RefuseManualStart=yes
RefuseManualStop=true
StopWhenUnneeded=yes
StartLimitIntervalSec=2s
StartLimitBurst=3

[Service]
Type=notify
ExecStart=/usr/bin/example --name "FractalD service"
ExecStartPre=/usr/bin/example --prepare
ExecStartPre=/usr/bin/example --prepare-again
ExecStartPost=/usr/bin/example --started
ExecStop=/usr/bin/example --shutdown
ExecStopPost=/usr/bin/example --clean
ExecReload=/usr/bin/example --reload
Environment=MODE=production 'MESSAGE=hello world'
LoadCredential=token:/etc/example.token
SetCredential=inline:fixture-value
ImportCredential=example.*:imported.
UnsetEnvironment=INHERITED_VARIABLE
Restart=on-failure
RestartSec=250ms
TimeoutStartSec=3s
TimeoutStopSec=4s
KillSignal=SIGINT
RestartKillSignal=SIGUSR1
SendSIGHUP=yes
IgnoreSIGPIPE=no
KillMode=mixed
WorkingDirectory=/var/lib/example
User=example
Group=example
DynamicUser=yes
SupplementaryGroups=audio video
NoNewPrivileges=yes
MemoryDenyWriteExecute=yes
RestrictRealtime=yes
UMask=0027
Nice=5
OOMScoreAdjust=-100
OOMPolicy=kill
LimitNOFILE=4096:8192
LimitMEMLOCK=64K:128K
LimitNPROC=128:256
WatchdogSec=20ms
NotifyAccess=all
RuntimeMaxSec=5min
Sockets=example.socket
PrivateTmp=yes
PrivateMounts=yes
PrivateIPC=yes
PrivateNetwork=yes
DevicePolicy=closed
DeviceAllow=/dev/null rw
Delegate=cpu memory
PrivateUsers=self
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=/var/lib/example /run/example
ReadOnlyPaths=/sys/fs/cgroup
InaccessiblePaths=-/etc/private
StandardInput=file:/var/lib/example/input
TTYPath=/dev/tty1
StandardOutput=append:/var/log/example.log
StandardError=null
RemainAfterExit=yes

[Install]
Alias=example-alias.service example-short
"#;

        let spec = parse_systemd_service(source, "example.service").expect("valid unit");
        assert_eq!(spec.name, "example.service");
        assert_eq!(spec.program, PathBuf::from("/usr/bin/example"));
        assert_eq!(
            spec.args,
            vec![OsString::from("--name"), OsString::from("FractalD service")]
        );
        assert_eq!(spec.service_type, ServiceType::Notify);
        assert!(!spec.default_dependencies);
        assert!(spec.allow_isolate);
        assert!(spec.ignore_on_isolate);
        assert_eq!(spec.restart, RestartPolicy::OnFailure);
        assert!(spec.refuse_manual_start);
        assert!(spec.refuse_manual_stop);
        assert!(spec.stop_when_unneeded);
        assert_eq!(spec.restart_backoff, Duration::from_millis(250));
        assert_eq!(spec.start_limit_interval, Some(Duration::from_secs(2)));
        assert_eq!(spec.start_limit_burst, Some(3));
        assert_eq!(spec.start_timeout, Duration::from_secs(3));
        assert_eq!(spec.stop_timeout, Duration::from_secs(4));
        assert_eq!(spec.runtime_max, Some(Duration::from_secs(300)));
        assert!(spec.dependencies.wants.contains("example.socket"));
        assert!(spec.dependencies.after.contains("example.socket"));
        assert_eq!(spec.kill_signal, 2);
        assert_eq!(spec.restart_kill_signal, Some(10));
        assert!(spec.send_sighup);
        assert!(!spec.ignore_sigpipe);
        assert_eq!(spec.kill_mode, KillMode::Mixed);
        assert_eq!(spec.device_policy, DevicePolicy::Closed);
        assert_eq!(spec.device_allow.len(), 1);
        assert_eq!(spec.device_allow[0].device, "/dev/null");
        assert_eq!(spec.device_allow[0].access, 6);
        assert_eq!(
            spec.delegate,
            DelegateMode::Controllers(["cpu".to_owned(), "memory".to_owned()].into())
        );
        assert_eq!(spec.private_users, PrivateUsersMode::SelfMapping);
        assert_eq!(spec.user.as_deref(), Some("example"));
        assert!(spec.dynamic_user);
        assert_eq!(
            spec.supplementary_groups,
            Some(vec!["audio".to_owned(), "video".to_owned()])
        );
        assert!(
            spec.unset_environment
                .contains(&OsString::from("INHERITED_VARIABLE"))
        );
        assert!(spec.no_new_privileges);
        assert!(spec.memory_deny_write_execute);
        assert!(spec.restrict_realtime);
        assert_eq!(spec.umask, Some(0o027));
        assert_eq!(spec.nice, Some(5));
        assert_eq!(spec.oom_score_adjust, Some(-100));
        assert_eq!(spec.oom_policy, OomPolicy::Kill);
        assert_eq!(
            spec.nofile,
            Some(LimitRange {
                soft: LimitValue::Value(4096),
                hard: LimitValue::Value(8192),
            })
        );
        assert_eq!(
            spec.memlock,
            Some(LimitRange {
                soft: LimitValue::Value(64 * 1024),
                hard: LimitValue::Value(128 * 1024),
            })
        );
        assert_eq!(
            spec.nproc,
            Some(LimitRange {
                soft: LimitValue::Value(128),
                hard: LimitValue::Value(256),
            })
        );
        assert_eq!(spec.watchdog, Some(Duration::from_millis(20)));
        assert_eq!(spec.notify_access, NotifyAccess::All);
        assert_eq!(spec.private_tmp, PrivateTmpMode::Yes);
        assert!(spec.private_mounts);
        assert!(spec.private_ipc);
        assert!(spec.private_network);
        assert_eq!(spec.protect_system, ProtectSystemMode::Strict);
        assert_eq!(spec.protect_home, ProtectHomeMode::ReadOnly);
        assert_eq!(
            spec.read_write_paths,
            vec![
                PathBuf::from("/var/lib/example"),
                PathBuf::from("/run/example")
            ]
        );
        assert_eq!(spec.read_only_paths, vec![PathBuf::from("/sys/fs/cgroup")]);
        assert_eq!(spec.inaccessible_paths, vec![PathBuf::from("/etc/private")]);
        assert_eq!(
            spec.aliases,
            [
                "example-alias.service".to_owned(),
                "example-short".to_owned()
            ]
            .into()
        );
        assert_eq!(
            spec.standard_input,
            InputMode::File(PathBuf::from("/var/lib/example/input"))
        );
        assert_eq!(spec.tty_path, Some(PathBuf::from("/dev/tty1")));
        assert_eq!(
            spec.stdout,
            OutputMode::File {
                path: PathBuf::from("/var/log/example.log"),
                append: true,
            }
        );
        assert_eq!(spec.stderr, OutputMode::Null);
        assert_eq!(spec.environment[&OsString::from("MESSAGE")], "hello world");
        assert_eq!(spec.credentials.len(), 2);
        assert_eq!(spec.credentials[0].name, "token");
        assert_eq!(
            spec.credentials[0].source,
            CredentialSource::File(PathBuf::from("/etc/example.token"))
        );
        assert_eq!(
            spec.credentials[1].source,
            CredentialSource::Value(b"fixture-value".to_vec())
        );
        assert_eq!(
            spec.credential_imports,
            vec![CredentialImportSpec {
                pattern: "example.*".to_owned(),
                rename: Some("imported.".to_owned()),
            }]
        );
        assert_eq!(
            spec.dependencies.requires,
            ["network.target".to_owned()].into()
        );
        assert_eq!(
            spec.dependencies.conflicts,
            ["shutdown.target".to_owned()].into()
        );
        assert_eq!(spec.dependencies.part_of, ["app.target".to_owned()].into());
        assert_eq!(
            spec.dependencies.binds_to,
            ["mount.service".to_owned()].into()
        );
        assert_eq!(
            spec.dependencies.requisite,
            ["network.target".to_owned()].into()
        );
        assert_eq!(
            spec.dependencies.on_success,
            ["cleanup.service".to_owned()].into()
        );
        assert_eq!(
            spec.dependencies.on_failure,
            ["recovery.service".to_owned()].into()
        );
        assert_eq!(
            spec.requires_mounts_for,
            vec![
                PathBuf::from("/var/lib/example"),
                PathBuf::from("/run/example")
            ]
        );
        assert_eq!(
            spec.wants_mounts_for,
            vec![PathBuf::from("/var/cache/example")]
        );
        assert_eq!(
            spec.stop.as_ref().expect("stop command").args,
            vec![OsString::from("--shutdown")]
        );
        assert_eq!(spec.start_pre.len(), 2);
        assert_eq!(spec.start_post.len(), 1);
        assert_eq!(spec.stop_post.len(), 1);
    }

    #[test]
    fn preserves_escaped_unit_names_in_dependency_lists() {
        let source = r#"
[Unit]
BindsTo=dev-disk-by\x2duuid-1234.device
After=dev-disk-by\x2duuid-1234.device

[Service]
ExecStart=/bin/true
"#;
        let spec = parse_systemd_service(source, "device-dependent.service")
            .expect("device dependency unit");
        assert!(
            spec.dependencies
                .binds_to
                .contains(r"dev-disk-by\x2duuid-1234.device")
        );
        assert!(
            spec.dependencies
                .after
                .contains(r"dev-disk-by\x2duuid-1234.device")
        );
        assert!(
            !spec
                .dependencies
                .binds_to
                .contains("dev-disk-by-uuid-1234.device")
        );
    }

    #[test]
    fn parses_disconnected_private_tmp() {
        let spec = parse_systemd_service(
            "[Service]\nPrivateTmp=disconnected\nExecStart=/bin/true\n",
            "tmp.service",
        )
        .expect("valid unit");
        assert_eq!(spec.private_tmp, PrivateTmpMode::Disconnected);
    }

    #[test]
    fn parses_dbus_bus_names() {
        let spec = parse_systemd_service(
            "[Service]\nType=dbus\nBusName=org.example.First org.example.Second\nExecStart=/bin/true\n",
            "dbus.service",
        )
        .expect("valid D-Bus unit");
        assert_eq!(spec.service_type, ServiceType::Dbus);
        assert_eq!(
            spec.bus_names,
            vec![
                "org.example.First".to_owned(),
                "org.example.Second".to_owned()
            ]
        );
    }

    #[test]
    fn requires_a_bus_name_for_dbus_services() {
        let error = parse_systemd_service(
            "[Service]\nType=dbus\nExecStart=/bin/true\n",
            "dbus.service",
        )
        .expect_err("D-Bus unit without a name");
        assert!(error.to_string().contains("requires BusName"));
    }

    #[test]
    fn distinguishes_protect_system_levels() {
        let yes = parse_systemd_service(
            "[Service]\nProtectSystem=yes\nExecStart=/bin/true\n",
            "yes.service",
        )
        .expect("ProtectSystem=yes");
        let full = parse_systemd_service(
            "[Service]\nProtectSystem=full\nExecStart=/bin/true\n",
            "full.service",
        )
        .expect("ProtectSystem=full");
        assert_eq!(yes.protect_system, ProtectSystemMode::Yes);
        assert_eq!(full.protect_system, ProtectSystemMode::Full);
    }

    #[test]
    fn handles_continuations_and_systemd_escapes() {
        let source = "[Service]\nExecStart=/bin/echo one \\\n            two\\x20words\n";
        let spec = parse_systemd_service(source, "echo.service").expect("valid unit");
        assert_eq!(
            spec.args,
            vec![OsString::from("one"), OsString::from("two words")]
        );
    }

    #[test]
    fn rejects_malformed_units_with_source_line() {
        let error =
            UnitFile::parse("[Service]\nExecStart /bin/example\n").expect_err("invalid unit");
        assert_eq!(error.line, Some(2));
        assert!(error.message.contains("missing '='"));
    }

    #[test]
    fn rejects_multiple_long_running_commands() {
        let error = parse_systemd_service(
            "[Service]\nExecStart=/bin/one\nExecStart=/bin/two\n",
            "duplicate.service",
        )
        .expect_err("ambiguous service");
        assert!(error.message.contains("multiple ExecStart"));
    }

    #[test]
    fn accepts_stop_only_oneshot_units() {
        let spec = parse_systemd_service(
            "[Service]\nType=oneshot\nRemainAfterExit=yes\nExecStop=/bin/echo cleanup\n",
            "cleanup.service",
        )
        .expect("stop-only oneshot unit");
        assert_eq!(spec.program, PathBuf::from("/bin/true"));
        assert!(spec.args.is_empty());
        assert_eq!(
            spec.stop.as_ref().expect("stop command").program,
            PathBuf::from("/bin/echo")
        );
        assert_eq!(
            spec.stop.as_ref().expect("stop command").args,
            vec![OsString::from("cleanup")]
        );
    }

    #[test]
    fn accepts_standard_linux_kill_signal_names() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/true\nKillSignal=SIGWINCH\n",
            "apache.service",
        )
        .expect("standard signal name");
        assert_eq!(spec.kill_signal, 28);
    }

    #[test]
    fn parses_duration_without_a_suffix_as_seconds() {
        assert_eq!(
            parse_duration("1.5", "TimeoutSec").expect("duration"),
            Duration::from_millis(1500)
        );
        assert_eq!(
            parse_duration("infinity", "TimeoutSec").expect("duration"),
            Duration::MAX
        );
    }

    #[test]
    fn merges_drop_in_values_and_honors_command_resets() {
        let mut base = UnitFile::parse(
            "[Unit]\nAfter=network.target\n[Service]\nExecStart=/bin/old\nRestart=no\n",
        )
        .expect("base unit");
        let overlay = UnitFile::parse(
            "[Unit]\nAfter=storage.target\n[Service]\nExecStart=\nExecStart=/bin/new\nRestart=always\n",
        )
        .expect("drop-in");
        base.merge(overlay);

        let spec = base
            .to_service_spec("example.service")
            .expect("merged unit");
        assert_eq!(spec.program, PathBuf::from("/bin/new"));
        assert_eq!(spec.restart, RestartPolicy::Always);
        assert_eq!(
            spec.dependencies.after,
            ["network.target", "storage.target"]
                .into_iter()
                .map(str::to_owned)
                .collect()
        );
    }

    #[test]
    fn adapts_openrc_scripts_with_dependencies_and_lifecycle_actions() {
        let source = r#"
depend() {
    need net
    use logger
    after firewall
    before app
}

reload() {
    ebegin "Reloading"
}
"#;
        let spec =
            parse_openrc_script(source, "example", "/etc/init.d/example").expect("OpenRC script");
        assert_eq!(spec.program, PathBuf::from("/bin/sh"));
        assert_eq!(spec.args[0], OsString::from("-c"));
        assert_eq!(spec.args[2], OsString::from("/etc/init.d/example"));
        assert_eq!(spec.args[3], OsString::from("start"));
        assert!(spec.remain_after_exit);
        assert_eq!(spec.stop.as_ref().expect("stop action").args[1], "stop");
        assert_eq!(
            spec.reload.as_ref().expect("reload action").args[1],
            "reload"
        );
        assert_eq!(spec.dependencies.requires, ["net".to_owned()].into());
        assert_eq!(spec.dependencies.wants, ["logger".to_owned()].into());
        assert_eq!(spec.dependencies.after, ["firewall".to_owned()].into());
        assert_eq!(spec.dependencies.before, ["app".to_owned()].into());
    }

    #[test]
    fn parses_cgroup_resource_limits() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nMemoryMax=64M\nMemoryHigh=32M\nMemoryMin=4M\nMemoryLow=8M\nMemorySwapMax=16M\nCPUWeight=500\nCPUQuota=125.5%\nCPUQuotaPeriodSec=250ms\nIOWeight=200\nTasksMax=infinity\nProtectControlGroups=yes\nProtectKernelModules=yes\nProtectKernelTunables=yes\nProtectKernelLogs=yes\nProtectClock=yes\nProtectProc=invisible\nProcSubset=pid\nProtectHostname=yes\nLockPersonality=yes\nPrivateDevices=yes\nCapabilityBoundingSet=CAP_CHOWN CAP_NET_BIND_SERVICE\nCapabilityBoundingSet=~CAP_NET_BIND_SERVICE\nAmbientCapabilities=CAP_NET_BIND_SERVICE\nRestrictAddressFamilies=AF_UNIX AF_INET6\nRestrictAddressFamilies=~AF_INET6\nRestrictSUIDSGID=yes\n",
            "worker.service",
        )
        .expect("resource limits");
        assert_eq!(
            spec.resources.memory_max,
            Some(LimitValue::Value(64 * 1024 * 1024))
        );
        assert_eq!(
            spec.resources.memory_high,
            Some(LimitValue::Value(32 * 1024 * 1024))
        );
        assert_eq!(
            spec.resources.memory_min,
            Some(LimitValue::Value(4 * 1024 * 1024))
        );
        assert_eq!(
            spec.resources.memory_low,
            Some(LimitValue::Value(8 * 1024 * 1024))
        );
        assert_eq!(
            spec.resources.memory_swap_max,
            Some(LimitValue::Value(16 * 1024 * 1024))
        );
        assert_eq!(spec.resources.cpu_weight, Some(500));
        assert_eq!(
            spec.resources.cpu_quota,
            Some(CpuQuota {
                quota_usec: 313_750,
                period_usec: 250_000,
            })
        );
        assert_eq!(spec.resources.io_weight, Some(200));
        assert_eq!(spec.resources.tasks_max, Some(LimitValue::Max));
        assert!(spec.protect_control_groups);
        assert!(spec.protect_kernel_modules);
        assert!(spec.protect_kernel_tunables);
        assert!(spec.protect_kernel_logs);
        assert!(spec.protect_clock);
        assert_eq!(spec.protect_proc, ProtectProcMode::Invisible);
        assert_eq!(spec.proc_subset, ProcSubsetMode::Pid);
        assert!(spec.protect_hostname);
        assert!(spec.lock_personality);
        assert!(spec.private_devices);
        assert!(spec.restrict_suid_sgid);
        assert_eq!(spec.capability_bounding_set, Some(1));
        assert_eq!(spec.ambient_capabilities, Some(1_u64 << 10));
        assert_eq!(spec.restrict_address_families, Some(1_u64 << 1));
    }

    #[test]
    fn parses_and_resets_ambient_capabilities() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nAmbientCapabilities=CAP_NET_BIND_SERVICE CAP_SYS_CHROOT\nAmbientCapabilities=\nAmbientCapabilities=CAP_NET_RAW\n",
            "ambient.service",
        )
        .expect("ambient capabilities");
        assert_eq!(spec.ambient_capabilities, Some(1_u64 << 13));

        let error = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nAmbientCapabilities=~CAP_NET_RAW\n",
            "ambient-inverted.service",
        )
        .expect_err("inverted ambient capabilities should fail");
        assert!(error.to_string().contains("does not support inverted"));
    }

    #[test]
    fn parses_system_call_filters_and_architecture_policy() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nSystemCallArchitectures=native\nSystemCallErrorNumber=EPERM\nSystemCallFilter=@basic-io\nSystemCallFilter=~write\n",
            "syscall-filter.service",
        )
        .expect("system call filter");
        assert_eq!(
            spec.system_call_architectures,
            Some(SystemCallArchitectures::Native)
        );
        assert_eq!(spec.system_call_error_number, Some(1));
        let filter = spec.system_call_filter.expect("filter policy");
        assert!(!filter.default_allow);
        assert!(
            filter
                .rules
                .iter()
                .any(|rule| { rule.name == "read" && rule.action == SystemCallRuleAction::Allow })
        );
        assert!(!filter.rules.iter().any(|rule| rule.name == "write"));

        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nSystemCallFilter=~@clock:EPERM\n",
            "syscall-deny.service",
        )
        .expect("deny filter");
        let filter = spec.system_call_filter.expect("deny policy");
        assert!(filter.default_allow);
        assert!(filter.rules.iter().any(|rule| {
            rule.name == "clock_settime" && rule.action == SystemCallRuleAction::DenyWithErrno(1)
        }));

        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nSystemCallErrorNumber=kill\nSystemCallFilter=~write:kill\n",
            "syscall-kill.service",
        )
        .expect("kill syscall action");
        assert_eq!(spec.system_call_error_number, Some(0));
        assert!(
            spec.system_call_filter
                .expect("kill policy")
                .rules
                .iter()
                .any(|rule| {
                    rule.name == "write" && rule.action == SystemCallRuleAction::DenyWithErrno(0)
                })
        );
    }

    #[test]
    fn parses_namespace_restriction_masks() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nRestrictNamespaces=net mnt\nRestrictNamespaces=~mnt\n",
            "namespace-filter.service",
        )
        .expect("namespace filter");
        assert_eq!(spec.restrict_namespaces, Some(0x4000_0000));

        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/worker\nRestrictNamespaces=~uts\n",
            "namespace-deny.service",
        )
        .expect("namespace deny filter");
        assert_eq!(spec.restrict_namespaces, Some(0x7a02_0080));
    }

    #[test]
    fn preserves_command_prefix_semantics() {
        let source = "[Service]\nType=oneshot\nExecStartPre=-:/bin/echo literal-$MODE\nExecStart=@/bin/echo custom-argv0 hello\n";
        let spec = parse_systemd_service(source, "prefix.service").expect("prefix unit");
        assert_eq!(spec.start_pre.len(), 1);
        assert!(spec.start_pre[0].ignore_failure);
        assert!(!spec.start_pre[0].expand_environment);
        assert_eq!(spec.program, PathBuf::from("/bin/echo"));
        assert_eq!(spec.main_argv0, Some(OsString::from("custom-argv0")));
        assert_eq!(spec.args, vec![OsString::from("hello")]);
    }

    #[test]
    fn allows_multiple_exec_start_commands_for_oneshot_units() {
        let source =
            "[Service]\nType=oneshot\nExecStart=/bin/echo first\nExecStart=/bin/echo second\n";
        let spec = parse_systemd_service(source, "multi.service").expect("oneshot unit");
        assert_eq!(spec.start_pre.len(), 1);
        assert_eq!(spec.start_pre[0].args, vec![OsString::from("first")]);
        assert_eq!(spec.args, vec![OsString::from("second")]);
    }

    #[test]
    fn parses_target_dependencies_into_a_persistent_unit() {
        let source =
            "[Unit]\nWants=network.service\nRequires=storage.target\nAfter=network.service\n";
        let spec = UnitFile::parse(source)
            .expect("target")
            .to_target_spec("default.target")
            .expect("target spec");
        assert_eq!(spec.service_type, ServiceType::Oneshot);
        assert!(spec.remain_after_exit);
        assert_eq!(spec.program, PathBuf::from("/bin/true"));
        assert_eq!(
            spec.dependencies.wants,
            ["network.service".to_owned()].into()
        );
        assert_eq!(
            spec.dependencies.requires,
            ["storage.target".to_owned()].into()
        );
    }

    #[test]
    fn accepts_common_notify_alias_and_exit_status_directives() {
        let source = "[Service]\nType=notify-reload\nExecStart=/bin/example\nSuccessExitStatus=143 SIGTERM\nRestartPreventExitStatus=2\n";
        let spec = parse_systemd_service(source, "notify.service").expect("notify unit");
        assert_eq!(spec.service_type, ServiceType::Notify);
        assert!(spec.success_exit_status.contains(&143));
        assert!(spec.success_exit_status.contains(&15));
        assert!(spec.restart_prevent_exit_status.contains(&2));
    }

    #[test]
    fn derives_and_parses_notify_access_defaults() {
        let notify = parse_systemd_service(
            "[Service]\nType=notify\nExecStart=/bin/worker\n",
            "notify.service",
        )
        .expect("notify unit");
        assert_eq!(notify.notify_access, NotifyAccess::Main);

        let watchdog = parse_systemd_service(
            "[Service]\nWatchdogSec=30s\nExecStart=/bin/worker\n",
            "watchdog.service",
        )
        .expect("watchdog unit");
        assert_eq!(watchdog.notify_access, NotifyAccess::Main);

        let access = parse_systemd_service(
            "[Service]\nNotifyAccess=exec\nExecStart=/bin/worker\n",
            "access.service",
        )
        .expect("notify access unit");
        assert_eq!(access.notify_access, NotifyAccess::Exec);
    }

    #[test]
    fn accepts_sysexits_names_in_success_status() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/example\nSuccessExitStatus=DATAERR CANTCREAT\n",
            "sysexits.service",
        )
        .expect("sysexits unit");
        assert!(spec.success_exit_status.contains(&65));
        assert!(spec.success_exit_status.contains(&73));
    }

    #[test]
    fn parses_a_forking_pid_file() {
        let spec = parse_systemd_service(
            "[Service]\nType=forking\nPIDFile=/run/example.pid\nExecStart=/usr/bin/example\n",
            "example.service",
        )
        .expect("forking unit");
        assert_eq!(spec.pid_file, Some(PathBuf::from("/run/example.pid")));
    }

    #[test]
    fn parses_path_conditions_and_negation() {
        let spec = parse_systemd_service(
            "[Unit]\nConditionPathExists=/etc/example\nConditionPathExists=!/etc/missing\nConditionPathExistsGlob=/var/lib/example/*\n[Service]\nExecStart=/bin/true\n",
            "conditions.service",
        )
        .expect("condition unit");
        assert_eq!(spec.conditions.len(), 3);
        assert!(matches!(
            &spec.conditions[1],
            Condition::PathExists { path, negate: true } if path == &PathBuf::from("/etc/missing")
        ));
    }

    #[test]
    fn parses_host_conditions_and_or_groups() {
        let source = "[Unit]\nConditionPathIsReadWrite=/etc\nConditionPathIsDirectory=/etc\nConditionFileNotEmpty=/etc/hostname\nConditionPathIsMountPoint=/\nConditionPathIsSymbolicLink=!/etc/hostname\nConditionKernelCommandLine=never-present\nConditionKernelCommandLine=|also-never-present\nConditionVirtualization=!container\nConditionSecurity=!selinux\nConditionACPower=yes\nConditionCapability=CAP_SYS_ADMIN\nConditionKernelModuleLoaded=!nouveau\nConditionFirmware=uefi\nConditionFirstBoot=no\nConditionCredential=example\nConditionControlGroupController=memory\nConditionEnvironment=PATH\nConditionEnvironment=LANG=C\nConditionNeedsUpdate=/etc\nAssertPathExists=/etc/hostname\nAssertPathExists=!/this/assertion-is-absent\nAssertEnvironment=!FRACTALD_TEST_ABSENT\nAssertNeedsUpdate=/var\n[Service]\nExecStart=/bin/true\n";
        let spec =
            parse_systemd_service(source, "host-conditions.service").expect("host conditions");
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::PathIsReadWrite { path, negate: false }
                if path == &PathBuf::from("/etc")
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::PathIsDirectory { path, negate: false }
                if path == &PathBuf::from("/etc")
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::FileNotEmpty { path, negate: false }
                if path == &PathBuf::from("/etc/hostname")
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::Any(conditions)
                if conditions.len() == 2
                    && conditions.iter().any(|condition| matches!(
                        condition,
                        Condition::KernelCommandLine { argument, negate: false }
                            if argument == "never-present"
                    ))
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::ACPower {
                on_ac_power: true,
                negate: false
            }
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::Capability { capability, negate: false }
                if capability == "CAP_SYS_ADMIN"
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::ControlGroupController { controller, negate: false }
                if controller == "memory"
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::Environment {
                name,
                value: None,
                negate: false
            } if name == "PATH"
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::Environment {
                name,
                value: Some(value),
                negate: false
            } if name == "LANG" && value == "C"
        )));
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::NeedsUpdate { path, negate: false } if path == &PathBuf::from("/etc")
        )));
        assert_eq!(spec.assertions.len(), 4);
        assert!(matches!(
            &spec.assertions[1],
            Condition::PathExists { path, negate: true }
                if path == &PathBuf::from("/this/assertion-is-absent")
        ));
        assert!(matches!(
            &spec.assertions[2],
            Condition::Environment {
                name,
                value: None,
                negate: true
            } if name == "FRACTALD_TEST_ABSENT"
        ));
        assert!(matches!(
            &spec.assertions[3],
            Condition::NeedsUpdate { path, negate: false } if path == &PathBuf::from("/var")
        ));
    }

    #[test]
    fn rejects_invalid_environment_and_needs_update_conditions() {
        let invalid_environment =
            "[Unit]\nConditionEnvironment=not-valid\n[Service]\nExecStart=/bin/true\n";
        let error = parse_systemd_service(invalid_environment, "invalid.service")
            .expect_err("invalid environment condition");
        assert!(error.to_string().contains("ConditionEnvironment"));

        let invalid_path = "[Unit]\nConditionNeedsUpdate=/usr\n[Service]\nExecStart=/bin/true\n";
        let error = parse_systemd_service(invalid_path, "invalid-update.service")
            .expect_err("invalid needs-update path");
        assert!(error.to_string().contains("ConditionNeedsUpdate"));
    }

    #[test]
    fn parses_stream_and_datagram_socket_units() {
        let source = "[Unit]\nAfter=network.target\n[Socket]\nService=example.service\nRemoveOnStop=yes\nSocketMode=0600\nSocketUser=example\nSocketGroup=example\nFileDescriptorName=example-listener\nListenStream=/run/example.sock\nListenDatagram=127.0.0.1:5353\n";
        let spec = UnitFile::parse(source)
            .expect("socket")
            .to_socket_spec("example.socket")
            .expect("socket spec");
        assert_eq!(spec.service_type, ServiceType::Socket);
        assert_eq!(spec.socket_service.as_deref(), Some("example.service"));
        assert_eq!(spec.socket_mode, 0o600);
        assert_eq!(spec.socket_user.as_deref(), Some("example"));
        assert_eq!(spec.socket_group.as_deref(), Some("example"));
        assert!(spec.remove_on_stop);
        assert_eq!(
            spec.file_descriptor_name.as_deref(),
            Some("example-listener")
        );
        assert_eq!(spec.listeners.len(), 2);
        assert_eq!(spec.listeners[0].kind, ListenerKind::Stream);
        assert_eq!(spec.listeners[0].address, "/run/example.sock");
        assert_eq!(spec.listeners[1].kind, ListenerKind::Datagram);
    }

    #[test]
    fn parses_per_connection_socket_activation() {
        let socket = UnitFile::parse("[Socket]\nAccept=yes\nListenStream=/run/example.sock\n")
            .expect("socket")
            .to_socket_spec("example.socket")
            .expect("accepting socket spec");
        assert!(socket.socket_accept);
        assert_eq!(socket.socket_service.as_deref(), Some("example@.service"));

        let service = parse_systemd_service(
            "[Service]\nStandardInput=socket\nStandardOutput=socket\nExecStart=/bin/example\n",
            "example@.service",
        )
        .expect("socket service");
        assert_eq!(service.standard_input, InputMode::Socket);
        assert_eq!(service.stdout, OutputMode::Socket);
    }

    #[test]
    fn parses_timer_schedules_and_associated_service() {
        let source = "[Unit]\nAfter=network.target\n[Timer]\nUnit=cleanup.service\nOnBootSec=10ms\nOnUnitActiveSec=1s\nOnCalendar=hourly\nPersistent=yes\nRandomizedDelaySec=250ms\nAccuracySec=500ms\n";
        let spec = UnitFile::parse(source)
            .expect("timer")
            .to_timer_spec("cleanup.timer")
            .expect("timer spec");
        assert_eq!(spec.service_type, ServiceType::Timer);
        assert_eq!(
            spec.dependencies.after,
            ["network.target".to_owned()].into()
        );
        assert!(matches!(
            spec.trigger,
            Some(TriggerSpec::Timer {
                service,
                on_boot: Some(boot),
                on_unit_active: Some(active),
                on_calendar,
                persistent,
                randomized_delay: Some(randomized_delay),
                accuracy: Some(accuracy),
                ..
            }) if service == "cleanup.service"
                && boot == Duration::from_millis(10)
                && active == Duration::from_secs(1)
                && on_calendar == vec!["hourly"]
                && persistent
                && randomized_delay == Duration::from_millis(250)
                && accuracy == Duration::from_millis(500)
        ));
    }

    #[test]
    fn parses_legacy_start_limit_interval_alias() {
        let spec = parse_systemd_service(
            "[Unit]\nStartLimitInterval=4s\nStartLimitBurst=2\n[Service]\nExecStart=/bin/true\n",
            "limited.service",
        )
        .expect("start limit");
        assert_eq!(spec.start_limit_interval, Some(Duration::from_secs(4)));
        assert_eq!(spec.start_limit_burst, Some(2));
    }

    #[test]
    fn parses_path_watches_and_associated_service() {
        let source = "[Path]\nUnit=import.service\nPathExists=/run/import.ready\nPathChanged=/var/lib/import\nPathExistsGlob=/var/lib/import/*.ready\nDirectoryNotEmpty=/var/spool/import\n";
        let spec = UnitFile::parse(source)
            .expect("path")
            .to_path_spec("import.path")
            .expect("path spec");
        assert_eq!(spec.service_type, ServiceType::Path);
        assert!(matches!(
            &spec.trigger,
            Some(TriggerSpec::Path { service, watches })
                if service == "import.service"
                    && watches == &vec![
                        PathWatch::Changed(PathBuf::from("/var/lib/import")),
                        PathWatch::Exists(PathBuf::from("/run/import.ready")),
                        PathWatch::ExistsGlob(PathBuf::from("/var/lib/import/*.ready")),
                        PathWatch::DirectoryNotEmpty(PathBuf::from("/var/spool/import")),
                    ]
        ));
    }

    #[test]
    fn parses_mount_units_into_supervised_mount_and_unmount_commands() {
        let source = "[Unit]\nAfter=local-fs-pre.target\n[Mount]\nWhat=/dev/mapper/data\nWhere=/srv/data\nType=ext4\nOptions=noatime\nDirectoryMode=0750\nLazyUnmount=yes\nForceUnmount=yes\nTimeoutSec=4s\n";
        let spec = UnitFile::parse(source)
            .expect("mount")
            .to_mount_spec("srv-data.mount")
            .expect("mount spec");
        assert_eq!(spec.service_type, ServiceType::Mount);
        assert!(spec.remain_after_exit);
        assert_eq!(spec.program, PathBuf::from("mount"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("-t"),
                OsString::from("ext4"),
                OsString::from("-o"),
                OsString::from("noatime"),
                OsString::from("--"),
                OsString::from("/dev/mapper/data"),
                OsString::from("/srv/data"),
            ]
        );
        assert_eq!(spec.start_pre[0].program, PathBuf::from("mkdir"));
        assert_eq!(spec.start_pre[0].args[2], OsString::from("750"));
        assert_eq!(
            spec.stop.as_ref().expect("unmount").args,
            vec![
                OsString::from("--lazy"),
                OsString::from("--force"),
                OsString::from("--"),
                OsString::from("/srv/data"),
            ]
        );
        assert_eq!(spec.start_timeout, Duration::from_secs(4));
        assert_eq!(spec.stop_timeout, Duration::from_secs(4));
        assert_eq!(spec.mount_where, Some(PathBuf::from("/srv/data")));
        assert_eq!(spec.mount_filesystem.as_deref(), Some("ext4"));
    }

    #[test]
    fn canonicalizes_filesystem_aliases_in_mount_units() {
        for (index, (requested, expected)) in [("FAT", "vfat"), ("msdos", "vfat"), ("Ext", "ext4")]
            .into_iter()
            .enumerate()
        {
            let source = format!(
                "[Mount]\nWhat=/dev/vd{index}\nWhere=/mnt/alias{index}\nType={requested}\n"
            );
            let spec = UnitFile::parse(&source)
                .expect("mount")
                .to_mount_spec(format!("alias{index}.mount"))
                .expect("mount spec");
            assert_eq!(spec.mount_filesystem.as_deref(), Some(expected));
            assert_eq!(spec.args[1], OsString::from(expected));
        }
    }

    #[test]
    fn parses_automount_units_as_eager_mount_transactions() {
        let source = "[Unit]\nConditionPathExists=/proc/sys/fs/binfmt_misc\nDefaultDependencies=no\nBefore=sysinit.target\n[Automount]\nWhere=/proc/sys/fs/binfmt_misc\nDirectoryMode=0750\nTimeoutIdleSec=4s\n";
        let spec = UnitFile::parse(source)
            .expect("automount")
            .to_automount_spec("proc-sys-fs-binfmt_misc.automount")
            .expect("automount spec");
        assert_eq!(spec.service_type, ServiceType::Oneshot);
        assert!(spec.remain_after_exit);
        assert_eq!(spec.start_pre[0].program, PathBuf::from("mkdir"));
        assert_eq!(spec.start_pre[0].args[2], OsString::from("750"));
        assert_eq!(
            spec.start_pre[0].args.last(),
            Some(&OsString::from("/proc/sys/fs/binfmt_misc"))
        );
        assert!(
            spec.dependencies
                .wants
                .contains("proc-sys-fs-binfmt_misc.mount")
        );
        assert!(
            spec.dependencies
                .after
                .contains("proc-sys-fs-binfmt_misc.mount")
        );
        assert!(spec.conditions.iter().any(|condition| matches!(
            condition,
            Condition::PathExists { path, negate: false }
                if path == &PathBuf::from("/proc/sys/fs/binfmt_misc")
        )));
    }

    #[test]
    fn parses_slice_units_as_active_lifecycle_nodes() {
        let source = "[Unit]\nDescription=Application slice\nBefore=slices.target\n[Slice]\nCPUQuota=200%\nCPUQuotaPeriodSec=1s\n";
        let spec = UnitFile::parse(source)
            .expect("slice")
            .to_slice_spec("app.slice")
            .expect("slice spec");
        assert_eq!(spec.service_type, ServiceType::Oneshot);
        assert!(spec.remain_after_exit);
        assert_eq!(spec.program, PathBuf::from("/bin/true"));
        assert!(spec.dependencies.before.contains("slices.target"));
        assert_eq!(
            spec.resources.cpu_quota,
            Some(CpuQuota {
                quota_usec: 2_000_000,
                period_usec: 1_000_000,
            })
        );
    }

    #[test]
    fn rejects_invalid_cpu_quota_values_and_periods() {
        for (key, value) in [("CPUQuota", "20"), ("CPUQuota", "1.2345678%")]
            .into_iter()
            .chain([("CPUQuotaPeriodSec", "500us")])
        {
            let source = format!("[Service]\nExecStart=/bin/true\n{key}={value}\n");
            assert!(
                parse_systemd_service(&source, "invalid-cpu.service").is_err(),
                "{key}={value} should be rejected"
            );
        }
    }

    #[test]
    fn parses_manager_shutdown_action_units_without_a_service_section() {
        let source = "[Unit]\nDefaultDependencies=no\nRequires=shutdown.target umount.target final.target\nAfter=shutdown.target umount.target final.target\nSuccessAction=reboot-force\n";
        let spec = UnitFile::parse(source)
            .expect("action unit")
            .to_action_spec("systemd-reboot.service")
            .expect("action spec");
        assert_eq!(spec.service_type, ServiceType::Oneshot);
        assert_eq!(spec.program, PathBuf::from("/bin/true"));
        assert!(spec.args.is_empty());
        assert_eq!(spec.success_action, ManagerAction::RebootForce);
        assert!(spec.dependencies.requires.contains("final.target"));
    }

    #[test]
    fn parses_service_manager_actions_without_delegating_to_systemd() {
        let source = "[Unit]\nFailureAction=poweroff-force\nSuccessAction=reboot-immediate\n[Service]\nExecStart=/bin/true\n";
        let spec = parse_systemd_service(source, "manager-action.service")
            .expect("manager action service");
        assert_eq!(spec.failure_action, ManagerAction::PoweroffForce);
        assert_eq!(spec.success_action, ManagerAction::RebootForce);
    }

    #[test]
    fn parses_job_timeout_and_manager_action() {
        let source = "[Unit]\nJobTimeoutSec=1500ms\nJobTimeoutAction=poweroff-force\n[Service]\nExecStart=/bin/true\n";
        let spec =
            parse_systemd_service(source, "job-timeout.service").expect("job timeout service");
        assert_eq!(spec.job_timeout, Some(Duration::from_millis(1500)));
        assert_eq!(spec.job_timeout_action, ManagerAction::PoweroffForce);

        let disabled = parse_systemd_service(
            "[Unit]\nJobTimeoutSec=0\n[Service]\nExecStart=/bin/true\n",
            "job-timeout-disabled.service",
        )
        .expect("disabled job timeout service");
        assert_eq!(disabled.job_timeout, None);
    }

    #[test]
    fn rejects_unknown_service_manager_actions() {
        let source = "[Unit]\nFailureAction=teleport\n[Service]\nExecStart=/bin/true\n";
        assert!(parse_systemd_service(source, "invalid-action.service").is_err());
    }

    #[test]
    fn rejects_wildcards_in_device_node_paths() {
        let source = r#"
[Service]
ExecStart=/bin/true
DeviceAllow=/dev/tty* rw
"#;
        assert!(parse_systemd_service(source, "device.service").is_err());
    }

    #[test]
    fn expands_requires_mounts_for_unit_specifiers() {
        let spec = parse_systemd_service(
            "[Unit]\nRequiresMountsFor=/var/lib/%n\n[Service]\nExecStart=/bin/true\n",
            "worker@alpha.service",
        )
        .expect("specifier unit");
        assert_eq!(
            spec.expanded_requires_mounts_for(),
            vec![PathBuf::from("/var/lib/worker@alpha.service")]
        );
    }

    #[test]
    fn expands_wants_mounts_for_unit_specifiers() {
        let spec = parse_systemd_service(
            "[Unit]\nWantsMountsFor=/var/cache/%n\n[Service]\nExecStart=/bin/true\n",
            "worker@alpha.service",
        )
        .expect("specifier unit");
        assert_eq!(
            spec.expanded_wants_mounts_for(),
            vec![PathBuf::from("/var/cache/worker@alpha.service")]
        );
    }

    #[test]
    fn parses_swap_units_into_supervised_swapon_and_swapoff_commands() {
        let source = "[Swap]\nWhat=/swapfile\nPriority=10\nOptions=discard\nTimeoutSec=2s\n";
        let spec = UnitFile::parse(source)
            .expect("swap")
            .to_swap_spec("swapfile.swap")
            .expect("swap spec");
        assert_eq!(spec.service_type, ServiceType::Swap);
        assert!(spec.remain_after_exit);
        assert_eq!(spec.program, PathBuf::from("swapon"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("--priority"),
                OsString::from("10"),
                OsString::from("-o"),
                OsString::from("discard"),
                OsString::from("--"),
                OsString::from("/swapfile"),
            ]
        );
        assert_eq!(
            spec.stop.as_ref().expect("swapoff").args,
            vec![OsString::from("--"), OsString::from("/swapfile")]
        );
        assert_eq!(spec.start_timeout, Duration::from_secs(2));
    }

    #[test]
    fn creates_a_waiting_spec_for_synthetic_device_units() {
        let unit = UnitFile::parse("[Unit]\nAfter=local-fs-pre.target\n").expect("device unit");
        let spec = unit
            .to_device_spec("dev-vda.device", "/dev/vda")
            .expect("device spec");
        assert_eq!(spec.program, PathBuf::from("/bin/sh"));
        assert_eq!(spec.service_type, ServiceType::Oneshot);
        assert_eq!(spec.device_path, Some(PathBuf::from("/dev/vda")));
        assert_eq!(spec.start_timeout, Duration::MAX);
        assert!(spec.remain_after_exit);
        assert!(spec.dependencies.after.contains("local-fs-pre.target"));
        let wait_script = spec.args[1].to_string_lossy();
        assert!(wait_script.contains("/usr/bin/blkid"));
        assert!(wait_script.contains("PARTUUID|PARTLABEL"));
        assert!(wait_script.contains("-o device"));
    }

    #[test]
    fn rejects_mount_units_without_an_absolute_mount_point() {
        let source = "[Mount]\nWhat=/dev/data\nWhere=relative\n";
        let error = UnitFile::parse(source)
            .expect("mount")
            .to_mount_spec("data.mount")
            .expect_err("relative mount point");
        assert!(error.to_string().contains("absolute path"));
    }

    #[test]
    fn parses_exec_conditions_and_service_directories() {
        let source = "[Service]\nExecCondition=/usr/bin/test -x /usr/bin/example\nExecStart=/usr/bin/example\nConfigurationDirectory=example-config\nConfigurationDirectoryMode=0750\nRuntimeDirectory=example worker\nRuntimeDirectoryMode=0700\nRuntimeDirectoryPreserve=yes\nStateDirectory=example-state\nCacheDirectory=example-cache\nLogsDirectory=example-log\n";
        let spec = parse_systemd_service(source, "example.service").expect("directory service");
        assert_eq!(spec.exec_conditions.len(), 1);
        assert_eq!(spec.directories.len(), 6);
        assert!(spec.directories.iter().any(|directory| {
            directory.kind == DirectoryKind::Configuration
                && directory.path == PathBuf::from("example-config")
                && directory.mode == 0o750
                && !directory.preserve
        }));
        assert!(spec.directories.iter().any(|directory| {
            directory.kind == DirectoryKind::Runtime
                && directory.path == PathBuf::from("example")
                && directory.mode == 0o700
                && directory.preserve
        }));
        assert!(
            spec.directories
                .iter()
                .any(|directory| directory.kind == DirectoryKind::State)
        );
        assert!(
            spec.directories
                .iter()
                .any(|directory| directory.kind == DirectoryKind::Cache)
        );
        assert!(
            spec.directories
                .iter()
                .any(|directory| directory.kind == DirectoryKind::Logs)
        );
    }

    #[test]
    fn parses_service_slice_placement() {
        let spec = parse_systemd_service(
            "[Service]\nSlice=user-%i.slice\nExecStart=/usr/bin/example\n",
            "user@1000.service",
        )
        .expect("slice service");
        assert_eq!(spec.cgroup_slice.as_deref(), Some("user-%i.slice"));
    }

    #[test]
    fn credential_directives_reset_and_validate_entries() {
        let mut file = UnitFile::parse(
            "[Service]\nExecStart=/bin/true\nLoadCredential=old:/tmp/old\nSetCredential=one:1\nImportCredential=old.*\n",
        )
        .expect("credential unit");
        file.merge(
            UnitFile::parse(
                "[Service]\nLoadCredential=\nLoadCredential=new:/tmp/new\nSetCredential=\nSetCredential=two:2\nImportCredential=\nImportCredential=new.*\n",
            )
            .expect("credential drop-in"),
        );
        let spec = file
            .to_service_spec("credentials.service")
            .expect("parsed credentials");
        assert_eq!(spec.credentials.len(), 2);
        assert_eq!(spec.credentials[0].name, "new");
        assert_eq!(spec.credentials[1].name, "two");
        assert_eq!(
            spec.credential_imports,
            vec![CredentialImportSpec {
                pattern: "new.*".to_owned(),
                rename: None,
            }]
        );

        let error = parse_systemd_service(
            "[Service]\nExecStart=/bin/true\nSetCredential=bad/name:value\n",
            "invalid-credentials.service",
        )
        .expect_err("unsafe credential name");
        assert!(error.to_string().contains("invalid credential name"));

        let error = parse_systemd_service(
            "[Service]\nExecStart=/bin/true\nImportCredential=bad?name\n",
            "invalid-import-credential.service",
        )
        .expect_err("unsafe import pattern");
        assert!(
            error
                .to_string()
                .contains("invalid ImportCredential pattern")
        );

        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/true\nImportCredential=tty.%I.agetty.*:agetty.\n",
            "getty@tty1.service",
        )
        .expect("specifier import pattern");
        assert_eq!(spec.credential_imports[0].pattern, "tty.%I.agetty.*");
    }

    #[test]
    fn parses_load_credential_store_forms() {
        let spec = parse_systemd_service(
            "[Service]\nExecStart=/bin/true\nLoadCredential=manager-secret\nLoadCredential=renamed:shared-secret\n",
            "store-credentials.service",
        )
        .expect("store credentials");
        assert_eq!(
            spec.credentials[0].source,
            CredentialSource::Store("manager-secret".to_owned())
        );
        assert_eq!(
            spec.credentials[1].source,
            CredentialSource::Store("shared-secret".to_owned())
        );
    }
}
