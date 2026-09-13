use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

mod graph;

pub use graph::{DependencyGraph, GraphError};

pub const SIGKILL: i32 = 9;
pub const SIGABRT: i32 = 6;
pub const SIGHUP: i32 = 1;
pub const SIGPIPE: i32 = 13;
pub const SIGTERM: i32 = 15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitValue {
    Max,
    Value(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitRange {
    pub soft: LimitValue,
    pub hard: LimitValue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CpuQuota {
    /// The cgroup v2 quota in microseconds for one period.
    pub quota_usec: u64,
    /// The cgroup v2 period in microseconds.
    pub period_usec: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    pub memory_max: Option<LimitValue>,
    pub memory_high: Option<LimitValue>,
    pub memory_min: Option<LimitValue>,
    pub memory_low: Option<LimitValue>,
    pub memory_swap_max: Option<LimitValue>,
    pub cpu_weight: Option<u64>,
    pub cpu_quota: Option<CpuQuota>,
    pub io_weight: Option<u64>,
    pub tasks_max: Option<LimitValue>,
}

impl ResourceLimits {
    pub const fn is_empty(&self) -> bool {
        self.memory_max.is_none()
            && self.memory_high.is_none()
            && self.memory_min.is_none()
            && self.memory_low.is_none()
            && self.memory_swap_max.is_none()
            && self.cpu_weight.is_none()
            && self.cpu_quota.is_none()
            && self.io_weight.is_none()
            && self.tasks_max.is_none()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DependencySet {
    pub requires: std::collections::BTreeSet<String>,
    pub wants: std::collections::BTreeSet<String>,
    pub after: std::collections::BTreeSet<String>,
    pub before: std::collections::BTreeSet<String>,
    pub conflicts: std::collections::BTreeSet<String>,
    pub part_of: std::collections::BTreeSet<String>,
    pub binds_to: std::collections::BTreeSet<String>,
    pub requisite: std::collections::BTreeSet<String>,
    pub on_success: std::collections::BTreeSet<String>,
    pub on_failure: std::collections::BTreeSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartPolicy {
    Never,
    OnSuccess,
    OnFailure,
    OnAbnormal,
    OnAbort,
    Always,
}

impl RestartPolicy {
    pub const fn should_restart(self, reason: ExitReason) -> bool {
        match self {
            Self::Never => false,
            Self::OnSuccess => reason.is_success(),
            Self::OnFailure => reason.is_failure(),
            Self::OnAbnormal => reason.is_signaled(),
            Self::OnAbort => reason.is_core_dumped(),
            Self::Always => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OomPolicy {
    Continue,
    Stop,
    Kill,
}

/// An action requested from the manager when a unit reaches a terminal
/// success or failure state.
///
/// The force and immediate forms retain the unit-file intent.  The platform
/// boundary currently exposes one power transition for each destination, so
/// those forms use the same kernel power operation after FractalD has stopped
/// its services in dependency order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManagerAction {
    None,
    Exit,
    Halt,
    HaltForce,
    Poweroff,
    PoweroffForce,
    Reboot,
    RebootForce,
    Kexec,
    KexecForce,
    SoftReboot,
    SoftRebootForce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceType {
    Simple,
    Forking,
    Oneshot,
    Notify,
    Dbus,
    Idle,
    Socket,
    Timer,
    Path,
    Mount,
    Swap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KillMode {
    ControlGroup,
    Process,
    Mixed,
    None,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevicePolicy {
    Auto,
    Closed,
    Strict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceAccessRule {
    /// A device path or a `char-`/`block-` device group pattern.
    pub device: String,
    /// Linux device access bits: read, write, and mknod.
    pub access: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DelegateMode {
    No,
    All,
    Controllers(BTreeSet<String>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateUsersMode {
    No,
    SelfMapping,
    Identity,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotifyAccess {
    None,
    Main,
    Exec,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Ignore a non-zero exit status for this command.
    ///
    /// A leading marker in a native command line can request this behavior.
    pub ignore_failure: bool,
    /// Use the first argument as argv[0] when the command is launched.
    ///
    /// Use the configured first argument as the launched program name.
    pub argv0: Option<OsString>,
    /// Keep environment variables in the command arguments literal.
    ///
    /// Keep environment expressions literal when this is disabled.
    pub expand_environment: bool,
}

impl CommandSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            ignore_failure: false,
            argv0: None,
            expand_environment: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvironmentFileSpec {
    pub path: PathBuf,
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CredentialSource {
    File(PathBuf),
    /// Resolve the credential from the manager's credential stores.
    Store(String),
    Value(Vec<u8>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialSpec {
    pub name: String,
    pub source: CredentialSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialImportSpec {
    pub pattern: String,
    pub rename: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputMode {
    Journal,
    Null,
    Inherit,
    Tty,
    Socket,
    File { path: PathBuf, append: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputMode {
    Null,
    Inherit,
    Tty,
    File(PathBuf),
    Socket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrivateTmpMode {
    No,
    Yes,
    Disconnected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectSystemMode {
    No,
    Yes,
    Full,
    Strict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectHomeMode {
    No,
    Yes,
    ReadOnly,
    Tmpfs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtectProcMode {
    Default,
    NoAccess,
    Invisible,
    Ptraceable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcSubsetMode {
    All,
    Pid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemCallRuleAction {
    Allow,
    Deny,
    DenyWithErrno(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemCallRule {
    pub name: String,
    pub action: SystemCallRuleAction,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemCallFilter {
    pub default_allow: bool,
    pub rules: Vec<SystemCallRule>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemCallArchitectures {
    Native,
    X86_64,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    Any(Vec<Condition>),
    PathExists {
        path: PathBuf,
        negate: bool,
    },
    PathExistsGlob {
        pattern: PathBuf,
        negate: bool,
    },
    DirectoryNotEmpty {
        path: PathBuf,
        negate: bool,
    },
    FileIsExecutable {
        path: PathBuf,
        negate: bool,
    },
    PathIsReadWrite {
        path: PathBuf,
        negate: bool,
    },
    PathIsDirectory {
        path: PathBuf,
        negate: bool,
    },
    FileNotEmpty {
        path: PathBuf,
        negate: bool,
    },
    PathIsMountPoint {
        path: PathBuf,
        negate: bool,
    },
    PathIsSymbolicLink {
        path: PathBuf,
        negate: bool,
    },
    KernelCommandLine {
        argument: String,
        negate: bool,
    },
    Virtualization {
        value: String,
        negate: bool,
    },
    Security {
        value: String,
        negate: bool,
    },
    ACPower {
        on_ac_power: bool,
        negate: bool,
    },
    Capability {
        capability: String,
        negate: bool,
    },
    KernelModuleLoaded {
        module: String,
        negate: bool,
    },
    Firmware {
        value: String,
        negate: bool,
    },
    FirstBoot {
        first_boot: bool,
        negate: bool,
    },
    Credential {
        credential: String,
        negate: bool,
    },
    ControlGroupController {
        controller: String,
        negate: bool,
    },
    Environment {
        name: String,
        value: Option<String>,
        negate: bool,
    },
    NeedsUpdate {
        path: PathBuf,
        negate: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenerKind {
    Stream,
    Datagram,
    SequentialPacket,
    Fifo,
    Netlink,
    Special,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerSpec {
    pub kind: ListenerKind,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TriggerSpec {
    Timer {
        service: String,
        on_boot: Option<Duration>,
        on_unit_active: Option<Duration>,
        on_unit_inactive: Option<Duration>,
        on_calendar: Vec<String>,
        persistent: bool,
        randomized_delay: Option<Duration>,
        accuracy: Option<Duration>,
    },
    Path {
        service: String,
        watches: Vec<PathWatch>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathWatch {
    Changed(PathBuf),
    Modified(PathBuf),
    Exists(PathBuf),
    ExistsGlob(PathBuf),
    DirectoryNotEmpty(PathBuf),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryKind {
    Configuration,
    Runtime,
    State,
    Cache,
    Logs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectorySpec {
    pub kind: DirectoryKind,
    pub path: PathBuf,
    pub mode: u32,
    pub preserve: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSpec {
    pub name: String,
    pub aliases: BTreeSet<String>,
    /// Boot or administrator selected profiles that may activate this service.
    pub profiles: BTreeSet<String>,
    /// Whether FractalD should synthesize the native default dependencies.
    pub default_dependencies: bool,
    /// Whether this service may be the root of an isolation transaction.
    pub allow_isolate: bool,
    /// Keep this service active when another profile is isolated.
    pub ignore_on_isolate: bool,
    pub refuse_manual_start: bool,
    pub refuse_manual_stop: bool,
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub main_ignore_failure: bool,
    pub main_argv0: Option<OsString>,
    pub main_expand_environment: bool,
    pub environment: BTreeMap<OsString, OsString>,
    pub unset_environment: BTreeSet<OsString>,
    pub environment_files: Vec<EnvironmentFileSpec>,
    pub credentials: Vec<CredentialSpec>,
    pub credential_imports: Vec<CredentialImportSpec>,
    pub exec_conditions: Vec<CommandSpec>,
    pub directories: Vec<DirectorySpec>,
    pub conditions: Vec<Condition>,
    pub assertions: Vec<Condition>,
    pub listeners: Vec<ListenerSpec>,
    pub file_descriptor_name: Option<String>,
    pub socket_service: Option<String>,
    pub socket_accept: bool,
    pub socket_mode: u32,
    pub socket_user: Option<String>,
    pub socket_group: Option<String>,
    pub remove_on_stop: bool,
    pub trigger: Option<TriggerSpec>,
    pub success_exit_status: BTreeSet<i32>,
    pub restart_prevent_exit_status: BTreeSet<i32>,
    pub restart: RestartPolicy,
    pub restart_limit: u32,
    /// Optional sliding-window start rate limit.
    pub start_limit_interval: Option<Duration>,
    pub start_limit_burst: Option<u32>,
    pub start_timeout: Duration,
    pub stop_timeout: Duration,
    /// Optional maximum time a start job may wait for this unit to complete.
    pub job_timeout: Option<Duration>,
    /// Manager action requested when the unit's start job times out.
    pub job_timeout_action: ManagerAction,
    /// Optional maximum time a service may remain running.
    pub runtime_max: Option<Duration>,
    pub restart_backoff: Duration,
    pub kill_signal: i32,
    /// Optional signal used for a restart transaction instead of KillSignal.
    pub restart_kill_signal: Option<i32>,
    /// Manager action requested when this unit fails.
    pub failure_action: ManagerAction,
    /// Manager action requested when this unit completes successfully.
    pub success_action: ManagerAction,
    /// Signal used after the configured stop signal when requested by the unit.
    pub send_sighup: bool,
    /// Whether lifecycle children inherit an ignored SIGPIPE disposition.
    pub ignore_sigpipe: bool,
    pub kill_mode: KillMode,
    pub dependencies: DependencySet,
    pub stop_when_unneeded: bool,
    /// Paths whose mount units must be active before this unit starts.
    pub requires_mounts_for: Vec<PathBuf>,
    /// Paths whose mount units should be started when this unit starts.
    pub wants_mounts_for: Vec<PathBuf>,
    /// The mount point owned by a mount unit, when this spec represents one.
    pub mount_where: Option<PathBuf>,
    /// The declared kernel filesystem type for a mount unit, when present.
    pub mount_filesystem: Option<String>,
    /// The kernel device path represented by a native `device-*` service.
    pub device_path: Option<PathBuf>,
    pub service_type: ServiceType,
    pub bus_names: Vec<String>,
    pub working_directory: Option<PathBuf>,
    pub pid_file: Option<PathBuf>,
    /// Allocate an ephemeral numeric user and group for each managed unit.
    pub dynamic_user: bool,
    pub user: Option<String>,
    pub group: Option<String>,
    pub supplementary_groups: Option<Vec<String>>,
    pub capability_bounding_set: Option<u64>,
    pub ambient_capabilities: Option<u64>,
    pub restrict_address_families: Option<u64>,
    pub system_call_filter: Option<SystemCallFilter>,
    pub system_call_architectures: Option<SystemCallArchitectures>,
    pub system_call_error_number: Option<i32>,
    pub no_new_privileges: bool,
    pub memory_deny_write_execute: bool,
    pub restrict_realtime: bool,
    pub restrict_suid_sgid: bool,
    pub protect_control_groups: bool,
    pub protect_kernel_modules: bool,
    pub protect_kernel_tunables: bool,
    pub protect_kernel_logs: bool,
    pub protect_clock: bool,
    pub protect_hostname: bool,
    pub lock_personality: bool,
    pub umask: Option<u32>,
    pub nice: Option<i32>,
    pub oom_score_adjust: Option<i32>,
    pub oom_policy: OomPolicy,
    pub nofile: Option<LimitRange>,
    pub memlock: Option<LimitRange>,
    pub nproc: Option<LimitRange>,
    pub watchdog: Option<Duration>,
    pub notify_access: NotifyAccess,
    pub private_tmp: PrivateTmpMode,
    pub private_devices: bool,
    pub device_policy: DevicePolicy,
    pub device_allow: Vec<DeviceAccessRule>,
    pub delegate: DelegateMode,
    pub private_users: PrivateUsersMode,
    pub private_mounts: bool,
    pub private_ipc: bool,
    pub private_network: bool,
    pub restrict_namespaces: Option<u32>,
    pub protect_system: ProtectSystemMode,
    pub protect_home: ProtectHomeMode,
    pub protect_proc: ProtectProcMode,
    pub proc_subset: ProcSubsetMode,
    pub read_write_paths: Vec<PathBuf>,
    pub read_only_paths: Vec<PathBuf>,
    pub inaccessible_paths: Vec<PathBuf>,
    pub standard_input: InputMode,
    pub tty_path: Option<PathBuf>,
    pub stdout: OutputMode,
    pub stderr: OutputMode,
    pub remain_after_exit: bool,
    pub stop: Option<CommandSpec>,
    pub reload: Option<CommandSpec>,
    pub start_pre: Vec<CommandSpec>,
    pub start_post: Vec<CommandSpec>,
    pub stop_post: Vec<CommandSpec>,
    /// Optional cgroup v2 slice placement, such as `system.slice`.
    pub cgroup_slice: Option<String>,
    pub resources: ResourceLimits,
}

impl ServiceSpec {
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            aliases: BTreeSet::new(),
            profiles: BTreeSet::new(),
            default_dependencies: true,
            allow_isolate: false,
            ignore_on_isolate: false,
            refuse_manual_start: false,
            refuse_manual_stop: false,
            program: program.into(),
            args: Vec::new(),
            main_ignore_failure: false,
            main_argv0: None,
            main_expand_environment: true,
            environment: BTreeMap::new(),
            unset_environment: BTreeSet::new(),
            environment_files: Vec::new(),
            credentials: Vec::new(),
            credential_imports: Vec::new(),
            exec_conditions: Vec::new(),
            directories: Vec::new(),
            conditions: Vec::new(),
            assertions: Vec::new(),
            listeners: Vec::new(),
            file_descriptor_name: None,
            socket_service: None,
            socket_accept: false,
            socket_mode: 0o666,
            socket_user: None,
            socket_group: None,
            remove_on_stop: false,
            trigger: None,
            success_exit_status: BTreeSet::new(),
            restart_prevent_exit_status: BTreeSet::new(),
            restart: RestartPolicy::Never,
            restart_limit: 5,
            start_limit_interval: None,
            start_limit_burst: None,
            start_timeout: Duration::from_secs(30),
            stop_timeout: Duration::from_secs(5),
            job_timeout: None,
            job_timeout_action: ManagerAction::None,
            runtime_max: None,
            restart_backoff: Duration::from_millis(250),
            kill_signal: SIGTERM,
            restart_kill_signal: None,
            failure_action: ManagerAction::None,
            success_action: ManagerAction::None,
            send_sighup: false,
            ignore_sigpipe: true,
            kill_mode: KillMode::ControlGroup,
            dependencies: DependencySet::default(),
            stop_when_unneeded: false,
            requires_mounts_for: Vec::new(),
            wants_mounts_for: Vec::new(),
            mount_where: None,
            mount_filesystem: None,
            device_path: None,
            service_type: ServiceType::Simple,
            bus_names: Vec::new(),
            working_directory: None,
            pid_file: None,
            dynamic_user: false,
            user: None,
            group: None,
            supplementary_groups: None,
            capability_bounding_set: None,
            ambient_capabilities: None,
            restrict_address_families: None,
            system_call_filter: None,
            system_call_architectures: None,
            system_call_error_number: None,
            no_new_privileges: false,
            memory_deny_write_execute: false,
            restrict_realtime: false,
            restrict_suid_sgid: false,
            protect_control_groups: false,
            protect_kernel_modules: false,
            protect_kernel_tunables: false,
            protect_kernel_logs: false,
            protect_clock: false,
            protect_hostname: false,
            lock_personality: false,
            umask: None,
            nice: None,
            oom_score_adjust: None,
            oom_policy: OomPolicy::Continue,
            nofile: None,
            memlock: None,
            nproc: None,
            watchdog: None,
            notify_access: NotifyAccess::None,
            private_tmp: PrivateTmpMode::No,
            private_devices: false,
            device_policy: DevicePolicy::Auto,
            device_allow: Vec::new(),
            delegate: DelegateMode::No,
            private_users: PrivateUsersMode::No,
            private_mounts: false,
            private_ipc: false,
            private_network: false,
            restrict_namespaces: None,
            protect_system: ProtectSystemMode::No,
            protect_home: ProtectHomeMode::No,
            protect_proc: ProtectProcMode::Default,
            proc_subset: ProcSubsetMode::All,
            read_write_paths: Vec::new(),
            read_only_paths: Vec::new(),
            inaccessible_paths: Vec::new(),
            standard_input: InputMode::Null,
            tty_path: None,
            stdout: OutputMode::Journal,
            stderr: OutputMode::Journal,
            remain_after_exit: false,
            stop: None,
            reload: None,
            start_pre: Vec::new(),
            start_post: Vec::new(),
            stop_post: Vec::new(),
            cgroup_slice: None,
            resources: ResourceLimits::default(),
        }
    }

    pub fn validate(&self) -> Result<(), SpecError> {
        if self.name.trim().is_empty() {
            return Err(SpecError::EmptyName);
        }
        if self
            .name
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
            || self.name.contains('/')
            || self.name == "."
            || self.name == ".."
        {
            return Err(SpecError::InvalidName(self.name.clone()));
        }
        if self.program.as_os_str().is_empty() {
            return Err(SpecError::EmptyProgram);
        }
        if let Some(alias) = self.aliases.iter().find(|alias| {
            alias.trim().is_empty()
                || alias
                    .chars()
                    .any(|character| character.is_whitespace() || character.is_control())
                || alias.contains('/')
                || *alias == "."
                || *alias == ".."
        }) {
            return Err(SpecError::InvalidAlias(alias.clone()));
        }
        if let Some(slice) = self.cgroup_slice.as_ref() {
            if !valid_slice_name(slice) {
                return Err(SpecError::InvalidSlice(slice.clone()));
            }
        }
        if let Some(name) = self.file_descriptor_name.as_deref() {
            if !valid_file_descriptor_name(name) {
                return Err(SpecError::InvalidFileDescriptorName(name.to_owned()));
            }
        }
        let mut credential_names = BTreeSet::new();
        for credential in &self.credentials {
            if !valid_credential_name(&credential.name)
                || matches!(&credential.source, CredentialSource::File(path) if path.as_os_str().is_empty())
                || matches!(&credential.source, CredentialSource::Store(name) if !valid_credential_name_template(name))
                || !credential_names.insert(&credential.name)
            {
                return Err(SpecError::InvalidCredential(credential.name.clone()));
            }
        }
        for import in &self.credential_imports {
            if !valid_credential_import_pattern(&import.pattern)
                || import
                    .rename
                    .as_deref()
                    .is_some_and(|rename| !valid_credential_import_rename(rename))
            {
                return Err(SpecError::InvalidCredential(import.pattern.clone()));
            }
        }
        if let Some(path) = self.read_write_paths.iter().find(|path| {
            !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
        }) {
            return Err(SpecError::InvalidReadWritePath(path.display().to_string()));
        }
        if let Some(path) = self.read_only_paths.iter().find(|path| {
            !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
        }) {
            return Err(SpecError::InvalidReadOnlyPath(path.display().to_string()));
        }
        if let Some(path) = self.inaccessible_paths.iter().find(|path| {
            !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
        }) {
            return Err(SpecError::InvalidInaccessiblePath(
                path.display().to_string(),
            ));
        }
        if let Some(path) = self.device_path.as_ref() {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                return Err(SpecError::InvalidDevicePath(path.display().to_string()));
            }
        }
        if self.kill_signal <= 0 {
            return Err(SpecError::InvalidKillSignal(self.kill_signal));
        }
        if let Some(signal) = self.restart_kill_signal {
            if signal <= 0 {
                return Err(SpecError::InvalidKillSignal(signal));
            }
        }
        Ok(())
    }

    /// Expand the path specifiers supported by `RequiresMountsFor=`.
    pub fn expanded_requires_mounts_for(&self) -> Vec<PathBuf> {
        self.requires_mounts_for
            .iter()
            .map(|path| expand_mount_path(path, self))
            .collect()
    }

    /// Expand the path specifiers supported by `WantsMountsFor=`.
    pub fn expanded_wants_mounts_for(&self) -> Vec<PathBuf> {
        self.wants_mounts_for
            .iter()
            .map(|path| expand_mount_path(path, self))
            .collect()
    }
}

fn expand_mount_path(path: &std::path::Path, spec: &ServiceSpec) -> PathBuf {
    let value = path.to_string_lossy();
    let service_stem = spec.name.strip_suffix(".svc").unwrap_or(&spec.name);
    let (prefix, instance) = match service_stem.rsplit_once('@') {
        Some((prefix, instance)) => (prefix, Some(instance)),
        None => (service_stem, None),
    };
    let runtime = env::var_os("FRACTALD_RUNTIME_DIR")
        .or_else(|| env::var_os("XDG_RUNTIME_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run"));
    let state = env::var_os("FRACTALD_STATE_DIR")
        .or_else(|| env::var_os("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/fractald"));
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"));
    let mut output = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '%' {
            output.push(character);
            continue;
        }
        let Some(specifier) = characters.next() else {
            output.push('%');
            break;
        };
        if specifier == '%' {
            output.push('%');
            continue;
        }
        let replacement = match specifier {
            'n' => Some(spec.name.clone()),
            'N' => Some(service_stem.to_owned()),
            'p' | 'P' => Some(prefix.to_owned()),
            'i' | 'I' | 'f' => Some(instance.unwrap_or_default().to_owned()),
            't' => Some(runtime.to_string_lossy().into_owned()),
            'S' => Some(state.to_string_lossy().into_owned()),
            'h' => Some(home.to_string_lossy().into_owned()),
            _ => None,
        };
        if let Some(replacement) = replacement {
            output.push_str(&replacement);
        } else {
            output.push('%');
            output.push(specifier);
        }
    }
    PathBuf::from(output)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecError {
    EmptyName,
    InvalidName(String),
    EmptyProgram,
    ZeroStartTimeout,
    ZeroStopTimeout,
    InvalidKillSignal(i32),
    InvalidReadWritePath(String),
    InvalidReadOnlyPath(String),
    InvalidInaccessiblePath(String),
    InvalidDevicePath(String),
    InvalidAlias(String),
    InvalidSlice(String),
    InvalidFileDescriptorName(String),
    InvalidCredential(String),
}

fn valid_slice_name(value: &str) -> bool {
    if value == "-.slice" {
        return true;
    }
    value.ends_with(".slice")
        && value.len() > ".slice".len()
        && !value[..value.len() - ".slice".len()].is_empty()
        && !value.contains('/')
        && !value.contains('\0')
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
}

fn valid_file_descriptor_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.is_ascii()
        && !value.contains(':')
        && !value.chars().any(char::is_control)
}

fn valid_credential_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value != ".fractald-owner"
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_credential_import_pattern(value: &str) -> bool {
    let wildcard_count = value.bytes().filter(|byte| *byte == b'*').count();
    if wildcard_count > 1 || value.bytes().any(|byte| matches!(byte, b'?' | b'[' | b']')) {
        return false;
    }
    if let Some(prefix) = value.strip_suffix('*') {
        prefix.is_empty() || valid_credential_name_template(prefix)
    } else {
        wildcard_count == 0 && valid_credential_name_template(value)
    }
}

fn valid_credential_import_rename(value: &str) -> bool {
    valid_credential_name_template(value) && !value.contains('*')
}

fn valid_credential_name_template(value: &str) -> bool {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitReason {
    Exited(i32),
    Signaled(i32),
    CoreDumped(i32),
}

impl ExitReason {
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Exited(0))
    }

    pub const fn is_failure(self) -> bool {
        !self.is_success()
    }

    pub const fn is_signaled(self) -> bool {
        matches!(self, Self::Signaled(_) | Self::CoreDumped(_))
    }

    pub const fn is_core_dumped(self) -> bool {
        matches!(self, Self::CoreDumped(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    Defined,
    Starting,
    Running,
    Active,
    Stopping,
    Backoff,
    Exited,
    Skipped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    StartRequested,
    Spawned {
        pid: u32,
        generation: u64,
    },
    Adopted {
        pid: u32,
        generation: u64,
    },
    Ready {
        pid: u32,
        generation: u64,
    },
    Activated {
        generation: u64,
    },
    ConditionSkipped {
        generation: u64,
    },
    StartFailed {
        generation: u64,
    },
    SpawnFailed {
        generation: u64,
    },
    StopRequested,
    StopCompleted,
    Exited {
        pid: u32,
        generation: u64,
        reason: ExitReason,
    },
    StartTimedOut {
        generation: u64,
    },
    JobTimedOut,
    StopTimedOut {
        generation: u64,
    },
    RuntimeTimedOut {
        generation: u64,
    },
    StartLimitHit {
        generation: u64,
    },
    RestartDue {
        generation: u64,
    },
    Reset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Spawn { generation: u64 },
    SendSignal { pid: u32, signal: i32 },
    ScheduleRestart { generation: u64, after: Duration },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransitionError {
    InvalidEvent { state: ServiceState, event: Event },
    InvalidPid,
    StaleGeneration { expected: u64, observed: u64 },
    UnexpectedPid { expected: u32, observed: u32 },
}

#[derive(Clone, Debug)]
pub struct ServiceRecord {
    spec: ServiceSpec,
    state: ServiceState,
    pid: Option<u32>,
    generation: u64,
    restart_count: u32,
    last_exit: Option<ExitReason>,
    start_failed: bool,
    runtime_failed: bool,
}

impl ServiceRecord {
    pub fn new(spec: ServiceSpec) -> Result<Self, SpecError> {
        spec.validate()?;
        Ok(Self {
            spec,
            state: ServiceState::Defined,
            pid: None,
            generation: 0,
            restart_count: 0,
            last_exit: None,
            start_failed: false,
            runtime_failed: false,
        })
    }

    pub const fn state(&self) -> ServiceState {
        self.state
    }

    pub const fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn restart_count(&self) -> u32 {
        self.restart_count
    }

    pub const fn last_exit(&self) -> Option<ExitReason> {
        self.last_exit
    }

    pub const fn spec(&self) -> &ServiceSpec {
        &self.spec
    }

    pub fn transition(&mut self, event: Event) -> Result<Vec<Action>, TransitionError> {
        match event {
            Event::StartRequested => self.start(),
            Event::Spawned { pid, generation } => self.spawned(pid, generation),
            Event::Adopted { pid, generation } => self.adopted(pid, generation),
            Event::Ready { pid, generation } => self.ready(pid, generation),
            Event::Activated { generation } => self.activated(generation),
            Event::ConditionSkipped { generation } => self.condition_skipped(generation),
            Event::StartFailed { generation } => self.start_failed(generation),
            Event::SpawnFailed { generation } => self.spawn_failed(generation),
            Event::StopRequested => self.stop(),
            Event::StopCompleted => self.stop_completed(),
            Event::Exited {
                pid,
                generation,
                reason,
            } => self.exited(pid, generation, reason),
            Event::StartTimedOut { generation } => self.start_timed_out(generation),
            Event::JobTimedOut => self.job_timed_out(),
            Event::StopTimedOut { generation } => self.stop_timed_out(generation),
            Event::RuntimeTimedOut { generation } => self.runtime_timed_out(generation),
            Event::StartLimitHit { generation } => self.start_limit_hit(generation),
            Event::RestartDue { generation } => self.restart_due(generation),
            Event::Reset => self.reset(),
        }
    }

    fn start(&mut self) -> Result<Vec<Action>, TransitionError> {
        match self.state {
            ServiceState::Defined
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed
            | ServiceState::Backoff => {
                self.advance_generation();
                self.state = ServiceState::Starting;
                self.pid = None;
                self.last_exit = None;
                self.restart_count = 0;
                self.start_failed = false;
                self.runtime_failed = false;
                Ok(vec![Action::Spawn {
                    generation: self.generation,
                }])
            }
            ServiceState::Active => Ok(Vec::new()),
            state => Err(TransitionError::InvalidEvent {
                state,
                event: Event::StartRequested,
            }),
        }
    }

    fn spawned(&mut self, pid: u32, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if pid == 0 {
            return Err(TransitionError::InvalidPid);
        }
        if !matches!(self.state, ServiceState::Starting | ServiceState::Stopping) {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::Spawned { pid, generation },
            });
        }
        if generation != self.generation {
            return Err(TransitionError::StaleGeneration {
                expected: self.generation,
                observed: generation,
            });
        }
        self.pid = Some(pid);
        if self.state == ServiceState::Starting {
            if !matches!(
                self.spec.service_type,
                ServiceType::Notify | ServiceType::Dbus
            ) {
                self.state = ServiceState::Running;
            }
            Ok(Vec::new())
        } else {
            Ok(vec![Action::SendSignal {
                pid,
                signal: self.spec.kill_signal,
            }])
        }
    }

    fn adopted(&mut self, pid: u32, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if pid == 0 {
            return Err(TransitionError::InvalidPid);
        }
        if self.state != ServiceState::Running {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::Adopted { pid, generation },
            });
        }
        self.check_generation(generation)?;
        self.pid = Some(pid);
        Ok(Vec::new())
    }

    fn ready(&mut self, pid: u32, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Starting {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::Ready { pid, generation },
            });
        }
        if pid == 0 {
            return Err(TransitionError::InvalidPid);
        }
        self.check_generation(generation)?;
        let expected_pid = self.pid.ok_or(TransitionError::UnexpectedPid {
            expected: 0,
            observed: pid,
        })?;
        if expected_pid != pid {
            return Err(TransitionError::UnexpectedPid {
                expected: expected_pid,
                observed: pid,
            });
        }
        self.state = ServiceState::Running;
        Ok(Vec::new())
    }

    fn activated(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Starting {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::Activated { generation },
            });
        }
        self.check_generation(generation)?;
        self.pid = None;
        self.state = ServiceState::Active;
        self.restart_count = 0;
        self.start_failed = false;
        Ok(Vec::new())
    }

    fn spawn_failed(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Starting {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::SpawnFailed { generation },
            });
        }
        self.check_generation(generation)?;
        let reason = ExitReason::Exited(127);
        self.last_exit = Some(reason);
        if self.spec.restart.should_restart(reason) && self.restart_count < self.spec.restart_limit
        {
            self.restart_count += 1;
            self.state = ServiceState::Backoff;
            Ok(vec![Action::ScheduleRestart {
                generation: self.generation,
                after: self.spec.restart_backoff,
            }])
        } else {
            self.state = ServiceState::Failed;
            Ok(Vec::new())
        }
    }

    fn condition_skipped(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Starting {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::ConditionSkipped { generation },
            });
        }
        self.check_generation(generation)?;
        self.pid = None;
        self.state = ServiceState::Skipped;
        self.restart_count = 0;
        self.start_failed = false;
        Ok(Vec::new())
    }

    fn start_failed(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if !matches!(self.state, ServiceState::Starting | ServiceState::Running) {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::StartFailed { generation },
            });
        }
        self.check_generation(generation)?;
        self.start_failed = true;
        match self.pid {
            Some(pid) => {
                self.state = ServiceState::Stopping;
                Ok(vec![Action::SendSignal {
                    pid,
                    signal: self.spec.kill_signal,
                }])
            }
            None => {
                self.state = ServiceState::Failed;
                self.advance_generation();
                Ok(Vec::new())
            }
        }
    }

    fn stop(&mut self) -> Result<Vec<Action>, TransitionError> {
        match self.state {
            ServiceState::Starting | ServiceState::Running => {
                self.state = ServiceState::Stopping;
                self.start_failed = false;
                self.runtime_failed = false;
                if self.spec.stop.is_some() {
                    Ok(Vec::new())
                } else {
                    Ok(self
                        .pid
                        .map(|pid| {
                            vec![Action::SendSignal {
                                pid,
                                signal: self.spec.kill_signal,
                            }]
                        })
                        .unwrap_or_default())
                }
            }
            ServiceState::Active => {
                self.state = ServiceState::Exited;
                self.restart_count = 0;
                Ok(Vec::new())
            }
            ServiceState::Backoff => {
                self.state = ServiceState::Exited;
                self.restart_count = 0;
                Ok(Vec::new())
            }
            ServiceState::Defined
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed => Ok(Vec::new()),
            ServiceState::Stopping => Ok(Vec::new()),
        }
    }

    fn stop_completed(&mut self) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Stopping || self.pid.is_some() {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::StopCompleted,
            });
        }
        self.state = ServiceState::Exited;
        self.restart_count = 0;
        self.start_failed = false;
        self.runtime_failed = false;
        Ok(Vec::new())
    }

    fn exited(
        &mut self,
        pid: u32,
        generation: u64,
        reason: ExitReason,
    ) -> Result<Vec<Action>, TransitionError> {
        if !matches!(
            self.state,
            ServiceState::Starting | ServiceState::Running | ServiceState::Stopping
        ) {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::Exited {
                    pid,
                    generation,
                    reason,
                },
            });
        }
        self.check_generation(generation)?;
        let expected_pid = self.pid.ok_or(TransitionError::UnexpectedPid {
            expected: 0,
            observed: pid,
        })?;
        if expected_pid != pid {
            return Err(TransitionError::UnexpectedPid {
                expected: expected_pid,
                observed: pid,
            });
        }

        let was_stopping = self.state == ServiceState::Stopping;
        let failed_during_start = self.start_failed;
        let failed_during_runtime = self.runtime_failed;

        self.pid = None;
        self.last_exit = Some(reason);
        self.start_failed = false;
        self.runtime_failed = false;

        let successful = self.successful_exit(reason);

        if was_stopping {
            if failed_during_start || failed_during_runtime {
                let failure = ExitReason::Exited(1);
                let prevented =
                    self.exit_status_matches(&self.spec.restart_prevent_exit_status, failure);
                if !prevented
                    && self.spec.restart.should_restart(failure)
                    && self.restart_count < self.spec.restart_limit
                {
                    self.restart_count += 1;
                    self.state = ServiceState::Backoff;
                    return Ok(vec![Action::ScheduleRestart {
                        generation: self.generation,
                        after: self.spec.restart_backoff,
                    }]);
                }
                self.state = ServiceState::Failed;
                return Ok(Vec::new());
            }
            self.state = ServiceState::Exited;
            self.restart_count = 0;
            return Ok(Vec::new());
        }

        if successful && self.spec.remain_after_exit {
            self.state = ServiceState::Active;
            self.restart_count = 0;
            return Ok(Vec::new());
        }

        let prevented = self.exit_status_matches(&self.spec.restart_prevent_exit_status, reason);
        let should_restart = !prevented
            && if successful && !reason.is_success() {
                matches!(
                    self.spec.restart,
                    RestartPolicy::OnSuccess | RestartPolicy::Always
                )
            } else {
                self.spec.restart.should_restart(reason)
            };

        if should_restart && self.restart_count < self.spec.restart_limit {
            self.restart_count += 1;
            self.state = ServiceState::Backoff;
            return Ok(vec![Action::ScheduleRestart {
                generation: self.generation,
                after: self.spec.restart_backoff,
            }]);
        }

        self.state = if successful {
            ServiceState::Exited
        } else {
            ServiceState::Failed
        };
        if self.state == ServiceState::Exited {
            self.restart_count = 0;
        }
        Ok(Vec::new())
    }

    fn successful_exit(&self, reason: ExitReason) -> bool {
        reason.is_success() || self.exit_status_matches(&self.spec.success_exit_status, reason)
    }

    fn exit_status_matches(&self, statuses: &BTreeSet<i32>, reason: ExitReason) -> bool {
        match reason {
            ExitReason::Exited(code) => statuses.contains(&code),
            ExitReason::Signaled(signal) | ExitReason::CoreDumped(signal) => {
                statuses.contains(&signal)
            }
        }
    }

    fn start_timed_out(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Starting {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::StartTimedOut { generation },
            });
        }
        self.check_generation(generation)?;
        match self.pid {
            Some(pid) => {
                self.state = ServiceState::Stopping;
                self.start_failed = true;
                Ok(vec![Action::SendSignal {
                    pid,
                    signal: self.spec.kill_signal,
                }])
            }
            None => {
                self.state = ServiceState::Failed;
                self.start_failed = false;
                self.runtime_failed = false;
                self.advance_generation();
                Ok(Vec::new())
            }
        }
    }

    fn job_timed_out(&mut self) -> Result<Vec<Action>, TransitionError> {
        if !matches!(
            self.state,
            ServiceState::Defined
                | ServiceState::Exited
                | ServiceState::Skipped
                | ServiceState::Failed
                | ServiceState::Backoff
        ) {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::JobTimedOut,
            });
        }
        self.pid = None;
        self.last_exit = Some(ExitReason::Exited(124));
        self.start_failed = false;
        self.runtime_failed = false;
        self.restart_count = 0;
        self.state = ServiceState::Failed;
        self.advance_generation();
        Ok(Vec::new())
    }

    fn runtime_timed_out(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Running {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::RuntimeTimedOut { generation },
            });
        }
        self.check_generation(generation)?;
        match self.pid {
            Some(pid) => {
                self.state = ServiceState::Stopping;
                self.runtime_failed = true;
                Ok(vec![Action::SendSignal {
                    pid,
                    signal: self.spec.kill_signal,
                }])
            }
            None => {
                self.state = ServiceState::Failed;
                self.runtime_failed = false;
                self.advance_generation();
                Ok(Vec::new())
            }
        }
    }

    fn stop_timed_out(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Stopping {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::StopTimedOut { generation },
            });
        }
        self.check_generation(generation)?;
        match self.pid {
            Some(pid) => Ok(vec![Action::SendSignal {
                pid,
                signal: SIGKILL,
            }]),
            None => {
                self.state = ServiceState::Exited;
                self.advance_generation();
                Ok(Vec::new())
            }
        }
    }

    fn start_limit_hit(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if !matches!(
            self.state,
            ServiceState::Defined
                | ServiceState::Starting
                | ServiceState::Backoff
                | ServiceState::Exited
                | ServiceState::Skipped
                | ServiceState::Failed
        ) {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::StartLimitHit { generation },
            });
        }
        self.check_generation(generation)?;
        self.pid = None;
        self.last_exit = Some(ExitReason::Exited(1));
        self.start_failed = false;
        self.runtime_failed = false;
        self.restart_count = 0;
        self.state = ServiceState::Failed;
        self.advance_generation();
        Ok(Vec::new())
    }

    fn restart_due(&mut self, generation: u64) -> Result<Vec<Action>, TransitionError> {
        if self.state != ServiceState::Backoff {
            return Err(TransitionError::InvalidEvent {
                state: self.state,
                event: Event::RestartDue { generation },
            });
        }
        self.check_generation(generation)?;
        self.advance_generation();
        self.state = ServiceState::Starting;
        Ok(vec![Action::Spawn {
            generation: self.generation,
        }])
    }

    fn reset(&mut self) -> Result<Vec<Action>, TransitionError> {
        match self.state {
            ServiceState::Defined
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed => {
                self.state = ServiceState::Defined;
                self.pid = None;
                self.restart_count = 0;
                self.last_exit = None;
                self.start_failed = false;
                self.runtime_failed = false;
                Ok(Vec::new())
            }
            state => Err(TransitionError::InvalidEvent {
                state,
                event: Event::Reset,
            }),
        }
    }

    fn check_generation(&self, observed: u64) -> Result<(), TransitionError> {
        if observed == self.generation {
            Ok(())
        } else {
            Err(TransitionError::StaleGeneration {
                expected: self.generation,
                observed,
            })
        }
    }

    fn advance_generation(&mut self) {
        self.generation = self.generation.checked_add(1).unwrap_or(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(restart: RestartPolicy) -> ServiceRecord {
        let mut spec = ServiceSpec::new("demo", "/usr/bin/demo");
        spec.restart = restart;
        ServiceRecord::new(spec).expect("valid service spec")
    }

    fn start_running(record: &mut ServiceRecord, pid: u32) {
        let actions = record
            .transition(Event::StartRequested)
            .expect("start request");
        assert_eq!(actions, vec![Action::Spawn { generation: 1 }]);
        record
            .transition(Event::Spawned { pid, generation: 1 })
            .expect("spawn event");
        assert_eq!(record.state(), ServiceState::Running);
    }

    #[test]
    fn normal_exit_reaches_exited() {
        let mut record = record(RestartPolicy::Never);
        start_running(&mut record, 42);

        let actions = record
            .transition(Event::Exited {
                pid: 42,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("exit event");

        assert!(actions.is_empty());
        assert_eq!(record.state(), ServiceState::Exited);
        assert_eq!(record.pid(), None);
        assert_eq!(record.last_exit(), Some(ExitReason::Exited(0)));
    }

    #[test]
    fn start_timeout_remains_a_failure_after_the_process_exits() {
        let mut spec = ServiceSpec::new("notify", "/usr/bin/notify");
        spec.service_type = ServiceType::Notify;
        let mut record = ServiceRecord::new(spec).expect("valid service spec");
        record
            .transition(Event::StartRequested)
            .expect("start request");
        record
            .transition(Event::Spawned {
                pid: 43,
                generation: 1,
            })
            .expect("spawn event");

        let actions = record
            .transition(Event::StartTimedOut { generation: 1 })
            .expect("start timeout");
        assert_eq!(record.state(), ServiceState::Stopping);
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 43,
                signal: SIGTERM,
            }]
        );

        record
            .transition(Event::Exited {
                pid: 43,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("timed out process exit");
        assert_eq!(record.state(), ServiceState::Failed);
    }

    #[test]
    fn job_timeout_fails_a_unit_that_has_not_started() {
        let mut record = record(RestartPolicy::Never);
        let actions = record.transition(Event::JobTimedOut).expect("job timeout");
        assert!(actions.is_empty());
        assert_eq!(record.state(), ServiceState::Failed);
        assert_eq!(record.last_exit(), Some(ExitReason::Exited(124)));
        assert_eq!(record.generation(), 1);
    }

    #[test]
    fn runtime_timeout_remains_a_failure_after_a_clean_exit() {
        let mut record = record(RestartPolicy::OnFailure);
        start_running(&mut record, 45);

        let actions = record
            .transition(Event::RuntimeTimedOut { generation: 1 })
            .expect("runtime timeout");
        assert_eq!(record.state(), ServiceState::Stopping);
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 45,
                signal: SIGTERM,
            }]
        );

        let actions = record
            .transition(Event::Exited {
                pid: 45,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("runtime timed out process exit");
        assert_eq!(record.state(), ServiceState::Backoff);
        assert_eq!(record.restart_count(), 1);
        assert_eq!(
            actions,
            vec![Action::ScheduleRestart {
                generation: 1,
                after: Duration::from_millis(250),
            }]
        );
    }

    #[test]
    fn failed_start_post_is_recorded_as_a_failure() {
        let mut record = record(RestartPolicy::Never);
        start_running(&mut record, 44);

        let actions = record
            .transition(Event::StartFailed { generation: 1 })
            .expect("start failure");
        assert_eq!(record.state(), ServiceState::Stopping);
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 44,
                signal: SIGTERM,
            }]
        );
        record
            .transition(Event::Exited {
                pid: 44,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("failed start exit");
        assert_eq!(record.state(), ServiceState::Failed);
    }

    #[test]
    fn failed_exit_schedules_bounded_restart() {
        let mut record = record(RestartPolicy::OnFailure);
        start_running(&mut record, 7);

        let actions = record
            .transition(Event::Exited {
                pid: 7,
                generation: 1,
                reason: ExitReason::Exited(1),
            })
            .expect("exit event");

        assert_eq!(record.state(), ServiceState::Backoff);
        assert_eq!(record.restart_count(), 1);
        assert_eq!(
            actions,
            vec![Action::ScheduleRestart {
                generation: 1,
                after: Duration::from_millis(250),
            }]
        );

        let actions = record
            .transition(Event::RestartDue { generation: 1 })
            .expect("restart deadline");
        assert_eq!(record.state(), ServiceState::Starting);
        assert_eq!(actions, vec![Action::Spawn { generation: 2 }]);
    }

    #[test]
    fn start_limit_hit_moves_a_pending_start_to_failed() {
        let mut record = record(RestartPolicy::OnFailure);
        record
            .transition(Event::StartRequested)
            .expect("start request");
        record
            .transition(Event::StartLimitHit { generation: 1 })
            .expect("start limit");
        assert_eq!(record.state(), ServiceState::Failed);
        assert_eq!(record.generation(), 2);
        assert_eq!(record.last_exit(), Some(ExitReason::Exited(1)));
    }

    #[test]
    fn remain_after_exit_keeps_a_successful_service_active() {
        let mut spec = ServiceSpec::new("oneshot", "/usr/bin/oneshot");
        spec.remain_after_exit = true;
        let mut record = ServiceRecord::new(spec).expect("valid service spec");
        start_running(&mut record, 17);

        record
            .transition(Event::Exited {
                pid: 17,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("exit event");
        assert_eq!(record.state(), ServiceState::Active);
        assert_eq!(record.pid(), None);

        record
            .transition(Event::StopRequested)
            .expect("stop active service");
        assert_eq!(record.state(), ServiceState::Exited);
    }

    #[test]
    fn explicit_stop_cancels_backoff() {
        let mut record = record(RestartPolicy::Always);
        start_running(&mut record, 11);
        record
            .transition(Event::Exited {
                pid: 11,
                generation: 1,
                reason: ExitReason::Exited(0),
            })
            .expect("exit event");
        assert_eq!(record.state(), ServiceState::Backoff);

        record
            .transition(Event::StopRequested)
            .expect("stop during backoff");
        assert_eq!(record.state(), ServiceState::Exited);
        assert_eq!(record.restart_count(), 0);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let mut record = record(RestartPolicy::Never);
        record
            .transition(Event::StartRequested)
            .expect("start request");

        let error = record
            .transition(Event::Spawned {
                pid: 99,
                generation: 0,
            })
            .expect_err("stale spawn must fail");

        assert_eq!(
            error,
            TransitionError::StaleGeneration {
                expected: 1,
                observed: 0,
            }
        );
        assert_eq!(record.state(), ServiceState::Starting);
    }

    #[test]
    fn wrong_pid_is_rejected() {
        let mut record = record(RestartPolicy::Never);
        start_running(&mut record, 101);

        let error = record
            .transition(Event::Exited {
                pid: 202,
                generation: 1,
                reason: ExitReason::Signaled(SIGTERM),
            })
            .expect_err("wrong process must fail");

        assert_eq!(
            error,
            TransitionError::UnexpectedPid {
                expected: 101,
                observed: 202,
            }
        );
        assert_eq!(record.state(), ServiceState::Running);
    }

    #[test]
    fn stale_exit_generation_is_rejected() {
        let mut record = record(RestartPolicy::Never);
        start_running(&mut record, 303);

        let error = record
            .transition(Event::Exited {
                pid: 303,
                generation: 0,
                reason: ExitReason::Exited(0),
            })
            .expect_err("stale exit must fail");

        assert_eq!(
            error,
            TransitionError::StaleGeneration {
                expected: 1,
                observed: 0,
            }
        );
        assert_eq!(record.state(), ServiceState::Running);
    }

    #[test]
    fn stop_during_spawn_is_delivered_to_late_child() {
        let mut record = record(RestartPolicy::Never);
        record
            .transition(Event::StartRequested)
            .expect("start request");
        record
            .transition(Event::StopRequested)
            .expect("stop request");
        assert_eq!(record.state(), ServiceState::Stopping);

        let actions = record
            .transition(Event::Spawned {
                pid: 404,
                generation: 1,
            })
            .expect("late spawn event");
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 404,
                signal: SIGTERM,
            }]
        );
        record
            .transition(Event::Exited {
                pid: 404,
                generation: 1,
                reason: ExitReason::Signaled(SIGTERM),
            })
            .expect("exit event");
        assert_eq!(record.state(), ServiceState::Exited);
    }

    #[test]
    fn stop_escalates_to_kill() {
        let mut record = record(RestartPolicy::Always);
        start_running(&mut record, 55);
        let actions = record
            .transition(Event::StopRequested)
            .expect("stop request");
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 55,
                signal: SIGTERM,
            }]
        );
        let actions = record
            .transition(Event::StopTimedOut { generation: 1 })
            .expect("stop timeout");
        assert_eq!(
            actions,
            vec![Action::SendSignal {
                pid: 55,
                signal: SIGKILL,
            }]
        );
        assert_eq!(record.state(), ServiceState::Stopping);
    }

    #[test]
    fn validation_rejects_empty_name() {
        let error = ServiceRecord::new(ServiceSpec::new(" ", "/bin/true"))
            .expect_err("empty name must fail");
        assert_eq!(error, SpecError::EmptyName);
    }

    #[test]
    fn validation_rejects_unsafe_writable_paths() {
        let mut spec = ServiceSpec::new("demo", "/bin/true");
        spec.read_write_paths.push(PathBuf::from("relative/path"));
        let error = ServiceRecord::new(spec).expect_err("relative writable path must fail");
        assert_eq!(
            error,
            SpecError::InvalidReadWritePath("relative/path".to_owned())
        );
    }

    #[test]
    fn validation_rejects_unsafe_read_only_and_inaccessible_paths() {
        let mut read_only = ServiceSpec::new("demo-read-only", "/bin/true");
        read_only.read_only_paths.push(PathBuf::from("../relative"));
        assert_eq!(
            ServiceRecord::new(read_only).expect_err("unsafe read-only path must fail"),
            SpecError::InvalidReadOnlyPath("../relative".to_owned())
        );

        let mut inaccessible = ServiceSpec::new("demo-inaccessible", "/bin/true");
        inaccessible
            .inaccessible_paths
            .push(PathBuf::from("relative"));
        assert_eq!(
            ServiceRecord::new(inaccessible).expect_err("unsafe inaccessible path must fail"),
            SpecError::InvalidInaccessiblePath("relative".to_owned())
        );
    }

    #[test]
    fn validation_rejects_invalid_slice_names() {
        let mut spec = ServiceSpec::new("demo", "/bin/true");
        spec.cgroup_slice = Some("../escape.slice".to_owned());
        assert_eq!(
            ServiceRecord::new(spec).expect_err("invalid slice must fail"),
            SpecError::InvalidSlice("../escape.slice".to_owned())
        );
    }

    #[test]
    fn configured_success_status_is_used_for_oneshot_completion() {
        let mut spec = ServiceSpec::new("compat", "/usr/bin/compat");
        spec.service_type = ServiceType::Oneshot;
        spec.success_exit_status.insert(143);
        let mut record = ServiceRecord::new(spec).expect("valid service spec");
        start_running(&mut record, 88);
        record
            .transition(Event::Exited {
                pid: 88,
                generation: 1,
                reason: ExitReason::Exited(143),
            })
            .expect("configured status");
        assert_eq!(record.state(), ServiceState::Exited);
    }

    #[test]
    fn restart_prevent_status_wins_over_an_always_policy() {
        let mut spec = ServiceSpec::new("compat", "/usr/bin/compat");
        spec.restart = RestartPolicy::Always;
        spec.restart_prevent_exit_status.insert(7);
        let mut record = ServiceRecord::new(spec).expect("valid service spec");
        start_running(&mut record, 89);
        let actions = record
            .transition(Event::Exited {
                pid: 89,
                generation: 1,
                reason: ExitReason::Exited(7),
            })
            .expect("prevented status");
        assert!(actions.is_empty());
        assert_eq!(record.state(), ServiceState::Failed);
    }

    #[test]
    fn notify_services_require_a_ready_event() {
        let mut spec = ServiceSpec::new("notify", "/usr/bin/notify");
        spec.service_type = ServiceType::Notify;
        let mut record = ServiceRecord::new(spec).expect("valid service spec");
        record
            .transition(Event::StartRequested)
            .expect("start request");
        record
            .transition(Event::Spawned {
                pid: 90,
                generation: 1,
            })
            .expect("spawn event");
        assert_eq!(record.state(), ServiceState::Starting);
        record
            .transition(Event::Ready {
                pid: 90,
                generation: 1,
            })
            .expect("ready event");
        assert_eq!(record.state(), ServiceState::Running);
    }

    #[test]
    fn dbus_services_require_bus_ownership_before_running() {
        let mut spec = ServiceSpec::new("dbus", "/usr/bin/dbus");
        spec.service_type = ServiceType::Dbus;
        spec.bus_names.push("org.example.Service".to_owned());
        let mut record = ServiceRecord::new(spec).expect("valid D-Bus service");
        record
            .transition(Event::StartRequested)
            .expect("start request");
        record
            .transition(Event::Spawned {
                pid: 91,
                generation: 1,
            })
            .expect("spawn event");
        assert_eq!(record.state(), ServiceState::Starting);
        record
            .transition(Event::Ready {
                pid: 91,
                generation: 1,
            })
            .expect("D-Bus ownership event");
        assert_eq!(record.state(), ServiceState::Running);
    }
}
