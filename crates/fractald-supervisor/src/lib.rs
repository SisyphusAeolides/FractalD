use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{SocketAddr, UnixDatagram, UnixListener};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fractald_core::{
    Action, Condition, CpuQuota, CredentialImportSpec, CredentialSource, DelegateMode,
    DependencyGraph, DeviceAccessRule, DevicePolicy, DirectoryKind, Event, ExitReason, GraphError,
    InputMode, KillMode, LimitRange, LimitValue, ManagerAction, NotifyAccess, OomPolicy,
    OutputMode, PathWatch, PrivateTmpMode, PrivateUsersMode, ProcSubsetMode, ProtectHomeMode,
    ProtectProcMode, ProtectSystemMode, SIGABRT, SIGHUP, SIGKILL, SIGPIPE, ServiceRecord,
    ServiceSpec, ServiceState, ServiceType, SystemCallArchitectures, TriggerSpec,
};
use fractald_journal::NativeJournal;
use fractald_platform::{DeviceFilterRule, ExitKind, PidFd};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceSnapshot {
    pub name: String,
    pub state: ServiceState,
    pub pid: Option<u32>,
    pub generation: u64,
    pub restart_count: u32,
    pub last_exit: Option<ExitReason>,
}

#[derive(Debug)]
pub struct Supervisor {
    services: BTreeMap<String, ManagedService>,
    aliases: BTreeMap<String, String>,
    start_jobs: Vec<StartJob>,
    pending_manager_action: Option<ManagerAction>,
    progressing_socket_activation: bool,
}

#[derive(Debug)]
struct StartJob {
    root: String,
    selected: BTreeSet<String>,
    required: BTreeSet<String>,
    order: Vec<String>,
    conflicts: BTreeSet<String>,
    attempted: BTreeSet<String>,
    started: BTreeSet<String>,
    deadline: Option<Instant>,
    timeout_action: ManagerAction,
}

#[derive(Debug)]
enum ManagedListener {
    Tcp(TcpListener),
    Udp(UdpSocket),
    UnixStream {
        listener: UnixListener,
        path: PathBuf,
    },
    UnixDatagram {
        socket: UnixDatagram,
        path: PathBuf,
    },
    Fifo {
        file: fs::File,
        path: PathBuf,
    },
    SeqPacket {
        file: fs::File,
        path: PathBuf,
    },
    Netlink {
        file: fs::File,
    },
    Special {
        file: fs::File,
    },
}

impl ManagedListener {
    fn raw_fd(&self) -> RawFd {
        match self {
            Self::Tcp(listener) => listener.as_raw_fd(),
            Self::Udp(socket) => socket.as_raw_fd(),
            Self::UnixStream { listener, .. } => listener.as_raw_fd(),
            Self::UnixDatagram { socket, .. } => socket.as_raw_fd(),
            Self::Fifo { file, .. }
            | Self::SeqPacket { file, .. }
            | Self::Netlink { file }
            | Self::Special { file } => file.as_raw_fd(),
        }
    }

    fn remove_path(&self) {
        match self {
            Self::UnixStream { path, .. }
            | Self::UnixDatagram { path, .. }
            | Self::Fifo { path, .. }
            | Self::SeqPacket { path, .. } => {
                if !path.as_os_str().is_empty() {
                    let _ = fs::remove_file(path);
                }
            }
            Self::Tcp(_) | Self::Udp(_) | Self::Netlink { .. } | Self::Special { .. } => {}
        }
    }

    fn disarm_path_removal(&mut self) {
        match self {
            Self::UnixStream { path, .. }
            | Self::UnixDatagram { path, .. }
            | Self::Fifo { path, .. }
            | Self::SeqPacket { path, .. } => path.clear(),
            Self::Tcp(_) | Self::Udp(_) | Self::Netlink { .. } | Self::Special { .. } => {}
        }
    }

    fn accept_connection(&self) -> std::io::Result<Option<RawFd>> {
        match self {
            Self::Tcp(listener) => match listener.accept() {
                Ok((stream, _)) => Ok(Some(stream.into_raw_fd())),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(error),
            },
            Self::UnixStream { listener, .. } => match listener.accept() {
                Ok((stream, _)) => Ok(Some(stream.into_raw_fd())),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(error),
            },
            Self::SeqPacket { file, .. } => match fractald_platform::accept_fd(file.as_raw_fd()) {
                Ok(fd) => Ok(Some(fd)),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
                Err(error) => Err(error),
            },
            Self::Udp(_)
            | Self::UnixDatagram { .. }
            | Self::Fifo { .. }
            | Self::Netlink { .. }
            | Self::Special { .. } => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Accept=yes requires a stream listener",
            )),
        }
    }
}

impl Drop for ManagedListener {
    fn drop(&mut self) {
        self.remove_path();
    }
}

#[derive(Debug)]
struct ManagedDirectory {
    path: PathBuf,
    kind: DirectoryKind,
    cleanup: bool,
}

const CREDENTIAL_OWNER_MARKER: &str = ".fractald-owner";
const MAX_CREDENTIAL_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
struct PrivateTmpPaths {
    tmp: PathBuf,
    var_tmp: PathBuf,
}

impl PrivateTmpPaths {
    fn cleanup(self) {
        let _ = fs::remove_dir_all(self.tmp);
        let _ = fs::remove_dir_all(self.var_tmp);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PathObservation {
    exists: bool,
    is_directory: bool,
    length: u64,
    modified: Option<SystemTime>,
    directory_nonempty: bool,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        Self {
            services: BTreeMap::new(),
            aliases: BTreeMap::new(),
            start_jobs: Vec::new(),
            pending_manager_action: None,
            progressing_socket_activation: false,
        }
    }

    pub fn add(&mut self, spec: ServiceSpec) -> Result<(), SupervisorError> {
        let name = spec.name.clone();
        let aliases = spec
            .aliases
            .iter()
            .map(|alias| alias.strip_suffix(".svc").unwrap_or(alias).to_owned())
            .collect::<BTreeSet<_>>();
        let identity = if spec.dynamic_user {
            Some(allocate_dynamic_identity(&name, &self.dynamic_user_ids())?)
        } else {
            None
        };
        let service = ManagedService::new_with_dynamic_identity(spec, identity)?;
        if self.services.contains_key(&name) || self.aliases.contains_key(&name) {
            return Err(SupervisorError::new(format!(
                "service {name} is defined more than once"
            )));
        }
        for alias in &aliases {
            if alias == &name {
                continue;
            }
            if self.services.contains_key(alias) || self.aliases.contains_key(alias) {
                return Err(SupervisorError::new(format!(
                    "service alias {alias} conflicts with an existing service"
                )));
            }
        }
        let canonical = name.clone();
        self.services.insert(name, service);
        for alias in aliases {
            if alias != canonical {
                self.aliases.insert(alias, canonical.clone());
            }
        }
        Ok(())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.services.keys().map(String::as_str)
    }

    pub fn resolve_name(&self, name: &str) -> Option<&str> {
        if self.services.contains_key(name) {
            return self
                .services
                .get_key_value(name)
                .map(|(name, _)| name.as_str());
        }
        if let Some(canonical) = self.aliases.get(name) {
            return Some(canonical.as_str());
        }
        let name = name.strip_suffix(".svc").unwrap_or(name);
        if self.services.contains_key(name) {
            return self
                .services
                .get_key_value(name)
                .map(|(name, _)| name.as_str());
        }
        if let Some(canonical) = self.aliases.get(name) {
            return Some(canonical.as_str());
        }
        self.services
            .keys()
            .find(|candidate| candidate.strip_suffix(".svc") == Some(name))
            .map(String::as_str)
    }

    pub fn specification(&self, name: &str) -> Option<&ServiceSpec> {
        self.resolve_name(name)
            .and_then(|name| self.services.get(name))
            .map(ManagedService::spec)
    }

    pub fn snapshot(&self, name: &str) -> Option<ServiceSnapshot> {
        self.resolve_name(name)
            .and_then(|name| self.services.get(name))
            .map(ManagedService::snapshot)
    }

    pub fn snapshots(&self) -> Vec<ServiceSnapshot> {
        self.services
            .values()
            .map(ManagedService::snapshot)
            .collect()
    }

    pub fn reload_state(&self, name: &str) -> Option<&'static str> {
        self.resolve_name(name)
            .and_then(|name| self.services.get(name))
            .map(ManagedService::reload_state)
    }

    pub fn reload_service(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        self.service_mut(&name)?.reload()
    }

    pub fn reset_failed(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        self.service_mut(&name)?.reset_failed()
    }

    pub fn reset_failed_all(&mut self) {
        for service in self.services.values_mut() {
            let _ = service.reset_failed();
        }
    }

    /// Return and clear the strongest manager action requested by a unit.
    ///
    /// A failure storm can produce several requests in one polling interval.
    /// Keeping one action prevents a later, weaker request from replacing a
    /// more protective power transition while still leaving the control loop
    /// in charge of the ordered service shutdown.
    pub fn take_manager_action(&mut self) -> Option<ManagerAction> {
        self.pending_manager_action.take()
    }

    pub fn reload(
        &mut self,
        specs: impl IntoIterator<Item = ServiceSpec>,
    ) -> Result<(), SupervisorError> {
        let mut incoming = BTreeMap::new();
        let mut reserved_dynamic_ids = self.dynamic_user_ids();
        for spec in specs {
            let name = spec.name.clone();
            let identity = if spec.dynamic_user {
                if let Some(identity) = self.dynamic_identity_for(&name) {
                    Some(identity)
                } else {
                    Some(allocate_dynamic_identity(&name, &reserved_dynamic_ids)?)
                }
            } else {
                None
            };
            if let Some(identity) = identity {
                reserved_dynamic_ids.insert(identity.uid);
            }
            let service = ManagedService::new_with_dynamic_identity(spec, identity)?;
            if incoming.insert(name.clone(), service).is_some() {
                return Err(SupervisorError::new(format!(
                    "service {name} is defined more than once"
                )));
            }
        }
        let incoming_aliases = collect_aliases(&incoming)?;

        for (name, existing) in &self.services {
            let Some(replacement) = incoming.get(name) else {
                if !existing.is_stopped() {
                    return Err(SupervisorError::new(format!(
                        "cannot remove active service {name} during reload"
                    )));
                }
                continue;
            };
            if existing.spec() != replacement.spec() && !existing.is_stopped() {
                return Err(SupervisorError::new(format!(
                    "cannot change active service {name} during reload"
                )));
            }
        }

        let mut old = std::mem::take(&mut self.services);
        let mut next = BTreeMap::new();
        for (name, replacement) in incoming {
            if let Some(existing) = old.remove(&name) {
                if existing.spec() == replacement.spec() {
                    next.insert(name, existing);
                } else {
                    next.insert(name, replacement);
                }
            } else {
                next.insert(name, replacement);
            }
        }
        self.services = next;
        self.aliases = incoming_aliases;
        Ok(())
    }

    pub fn start(&mut self, name: &str) -> Result<(), SupervisorError> {
        self.start_with_origin(name, true)
    }

    /// Start an isolatable target and stop units outside its dependency
    /// closure.  Isolation is a manager transaction: manual stop refusals do
    /// not prevent the manager from converging on the requested target.
    pub fn isolate(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        let Some(service) = self.services.get(&name) else {
            return Err(SupervisorError::new(format!("unknown service {name}")));
        };
        if !service.spec().allow_isolate {
            return Err(SupervisorError::new(format!(
                "service profile {name} does not allow isolation"
            )));
        }

        let graph = self.graph()?;
        let target_order = graph.plan_start(&name).map_err(|error| {
            SupervisorError::new(format!("cannot plan isolation for {name}: {error:?}"))
        })?;
        let mut keep = target_order.into_iter().collect::<BTreeSet<_>>();

        // An ignored unit remains part of the isolated manager state.  Keep
        // its own dependency closure as well so a pending activation is not
        // torn down halfway through the transaction.
        let ignored = self
            .services
            .iter()
            .filter_map(|(candidate, service)| {
                service
                    .spec()
                    .ignore_on_isolate
                    .then_some(candidate.clone())
            })
            .collect::<Vec<_>>();
        for candidate in ignored {
            keep.insert(candidate.clone());
            if let Ok(order) = graph.plan_start(&candidate) {
                keep.extend(order);
            }
        }

        self.cancel_isolation_jobs(&keep);
        self.start_with_origin(&name, true)?;

        let now = Instant::now();
        let stop_names = self
            .services
            .iter()
            .filter_map(|(candidate, service)| {
                (!keep.contains(candidate) && !service.spec().ignore_on_isolate)
                    .then_some(candidate.clone())
            })
            .collect::<Vec<_>>();
        for candidate in stop_names {
            self.cancel_start_jobs(&candidate)?;
            self.service_mut(&candidate)?.stop(now)?;
        }
        self.reap_unneeded(now)
    }

    fn start_with_origin(&mut self, name: &str, manual: bool) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        self.refresh_lost_mount(&name)?;
        if manual
            && self
                .services
                .get(&name)
                .is_some_and(|service| service.spec().refuse_manual_start)
        {
            return Err(SupervisorError::new(format!(
                "manual start is refused for {name}"
            )));
        }
        self.check_requisites(&name)?;
        let before = self
            .services
            .iter()
            .map(|(service_name, service)| (service_name.clone(), service.record.state()))
            .collect::<BTreeMap<_, _>>();
        let graph = self.graph()?;
        let required = graph.required_closure(&name).map_err(|error| {
            SupervisorError::new(format!("cannot plan start for {name}: {error:?}"))
        })?;
        let order = graph.plan_start(&name).map_err(|error| {
            SupervisorError::new(format!("cannot plan start for {name}: {error:?}"))
        })?;
        let selected = order.iter().cloned().collect();
        let conflicts = self.conflicts_for(&selected)?;
        if conflicts.contains(&name) {
            return Err(SupervisorError::new(format!(
                "service {name} conflicts with itself"
            )));
        }
        let now = Instant::now();
        let (job_timeout, timeout_action) = self
            .services
            .get(&name)
            .map(|service| {
                (
                    service.spec().job_timeout,
                    service.spec().job_timeout_action,
                )
            })
            .unwrap_or((None, ManagerAction::None));
        for conflict in &conflicts {
            self.cancel_start_jobs(conflict)?;
            if let Some(service) = self.services.get_mut(conflict) {
                service.stop(now)?;
            }
            self.stop_dependents(conflict, true, now)?;
        }
        self.start_jobs.push(StartJob {
            root: name.to_owned(),
            selected,
            required,
            order,
            conflicts,
            attempted: BTreeSet::new(),
            started: BTreeSet::new(),
            deadline: job_timeout.and_then(|timeout| timeout_deadline(now, timeout)),
            timeout_action,
        });
        self.progress_start_jobs(now)?;
        self.propagate_bindings(&before, now)?;
        self.propagate_successes(&before)?;
        self.propagate_failures(&before)?;
        self.reap_unneeded(now)
    }

    pub fn stop(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        if self
            .services
            .get(&name)
            .is_some_and(|service| service.spec().refuse_manual_stop)
        {
            return Err(SupervisorError::new(format!(
                "manual stop is refused for {name}"
            )));
        }
        self.cancel_start_jobs(&name)?;
        let now = Instant::now();
        self.service_mut(&name)?.stop(now)?;
        self.stop_dependents(&name, true, now)
    }

    /// Stop a service while replacing its native descriptor during a manager
    /// reload.  Configuration changes are manager initiated, so a descriptor
    /// cannot block its own replacement with a manual-stop refusal.
    pub fn stop_for_reconfigure(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        self.cancel_start_jobs(&name)?;
        let now = Instant::now();
        self.service_mut(&name)?.stop(now)?;
        self.stop_dependents(&name, true, now)
    }

    pub fn restart(&mut self, name: &str) -> Result<(), SupervisorError> {
        let name = self.canonical_name(name);
        self.refresh_lost_mount(&name)?;
        if let Some(service) = self.services.get(&name) {
            if service.spec().refuse_manual_start {
                return Err(SupervisorError::new(format!(
                    "manual start is refused for {name}"
                )));
            }
            if service.spec().refuse_manual_stop {
                return Err(SupervisorError::new(format!(
                    "manual stop is refused for {name}"
                )));
            }
        }
        let before = self
            .services
            .iter()
            .map(|(service_name, service)| (service_name.clone(), service.record.state()))
            .collect::<BTreeMap<_, _>>();
        self.cancel_start_jobs(&name)?;
        let now = Instant::now();
        self.stop_dependents(&name, true, now)?;
        self.service_mut(&name)?.restart(now)?;
        self.propagate_bindings(&before, now)?;
        self.propagate_failures(&before)?;
        self.reap_unneeded(now)
    }

    pub fn stop_all(&mut self) -> Result<(), SupervisorError> {
        self.start_jobs.clear();
        let graph = self.graph()?;
        let names = match graph.plan_all_stop() {
            Ok(names) => names,
            Err(GraphError::Cycle(cycle)) => {
                eprintln!(
                    "fractald: dependency ordering contains a shutdown cycle involving {} units; relaxing cycle edges for shutdown",
                    cycle.len()
                );
                graph.plan_all_stop_best_effort()
            }
            Err(error) => {
                return Err(SupervisorError::new(format!(
                    "cannot plan shutdown: {error:?}"
                )));
            }
        };
        let now = Instant::now();
        for name in names {
            self.service_mut(&name)?.stop(now)?;
            self.stop_dependents(&name, true, now)?;
        }
        Ok(())
    }

    pub fn reconcile_orphans(&self) -> Result<usize, SupervisorError> {
        let configured_root = env::var_os("FRACTALD_CGROUP_ROOT").map(PathBuf::from);
        if configured_root.is_none()
            && !self.services.values().any(|service| {
                !service.spec().resources.is_empty() || service.spec().cgroup_slice.is_some()
            })
        {
            return Ok(0);
        }
        let root = configured_root.unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"));
        let managed_root = root.join("fractald");
        let entries = match fs::read_dir(&managed_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(SupervisorError::new(format!(
                    "cannot enumerate stale cgroups in {}: {error}",
                    managed_root.display()
                )));
            }
        };
        let mut reconciled = 0;
        for entry in entries {
            let entry = entry.map_err(|error| {
                SupervisorError::new(format!(
                    "cannot enumerate stale cgroups in {}: {error}",
                    managed_root.display()
                ))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                SupervisorError::new(format!(
                    "cannot inspect stale cgroup {}: {error}",
                    path.display()
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            reconcile_cgroup_tree(&path, &mut reconciled)?;
        }
        Ok(reconciled)
    }

    pub fn is_stopped(&self) -> bool {
        self.start_jobs.is_empty() && self.services.values().all(ManagedService::is_stopped)
    }

    pub fn service_is_stopped(&self, name: &str) -> Option<bool> {
        self.resolve_name(name)
            .and_then(|name| self.services.get(name))
            .map(ManagedService::is_stopped)
    }

    pub fn reap_untracked_children(&self) -> Result<usize, SupervisorError> {
        let mut managed = self
            .services
            .values()
            .flat_map(ManagedService::direct_child_pids)
            .collect::<Vec<_>>();
        managed.sort_unstable();
        managed.dedup();
        fractald_platform::reap_untracked_children(&managed).map_err(|error| {
            SupervisorError::new(format!("cannot reap untracked child processes: {error}"))
        })
    }

    pub fn poll(&mut self) -> Result<(), SupervisorError> {
        let names: Vec<String> = self.services.keys().cloned().collect();
        let before = self
            .services
            .iter()
            .map(|(name, service)| (name.clone(), service.record.state()))
            .collect::<BTreeMap<_, _>>();
        let now = Instant::now();
        for name in names {
            self.refresh_lost_mount(&name)?;
            self.service_mut(&name)?.poll(now)?;
        }
        self.progress_start_jobs(now)?;
        self.propagate_bindings(&before, now)?;
        self.propagate_successes(&before)?;
        self.propagate_failures(&before)?;
        self.reap_unneeded(now)
    }

    fn progress_start_jobs(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let mut jobs = std::mem::take(&mut self.start_jobs);
        let mut pending = Vec::new();
        for mut job in jobs.drain(..) {
            if self.progress_start_job(&mut job, now)? {
                pending.push(job);
            }
        }
        self.start_jobs = pending;
        self.progress_socket_activation()?;
        self.progress_trigger_activation(now);
        Ok(())
    }

    fn refresh_lost_mount(&mut self, name: &str) -> Result<(), SupervisorError> {
        let lost = self
            .services
            .get(name)
            .is_some_and(ManagedService::mount_is_lost);
        if lost {
            self.service_mut(name)?.mark_mount_lost()?;
        } else if self
            .services
            .get(name)
            .is_some_and(ManagedService::device_is_lost)
        {
            self.service_mut(name)?.mark_device_lost()?;
        }
        Ok(())
    }

    fn progress_socket_activation(&mut self) -> Result<(), SupervisorError> {
        if self.progressing_socket_activation {
            return Ok(());
        }
        self.progressing_socket_activation = true;
        let result = self.progress_socket_activation_inner();
        self.progressing_socket_activation = false;
        result
    }

    fn progress_socket_activation_inner(&mut self) -> Result<(), SupervisorError> {
        let activations = self
            .services
            .values()
            .filter_map(|socket| {
                if socket.spec().service_type != ServiceType::Socket
                    || socket.record.state() != ServiceState::Active
                    || socket.listeners.is_empty()
                {
                    return None;
                }
                Some((
                    socket.spec().name.clone(),
                    socket.spec().socket_service.clone(),
                    socket.spec().socket_accept,
                    socket
                        .spec()
                        .file_descriptor_name
                        .clone()
                        .unwrap_or_else(|| {
                            if socket.spec().socket_accept {
                                "connection".to_owned()
                            } else {
                                socket.spec().name.clone()
                            }
                        }),
                    socket.listener_fds(),
                ))
            })
            .collect::<Vec<_>>();
        let mut ordinary = BTreeMap::<String, (Vec<RawFd>, Vec<String>, Vec<String>)>::new();
        for (socket_name, target, accept, fd_name, fds) in activations {
            let Some(target) = target else {
                continue;
            };
            let Some(target_name) = self.resolve_name(&target).map(str::to_owned) else {
                continue;
            };
            let should_start = self.services.get(&target_name).is_some_and(|service| {
                matches!(
                    service.record.state(),
                    ServiceState::Defined
                        | ServiceState::Exited
                        | ServiceState::Skipped
                        | ServiceState::Failed
                )
            });
            if !should_start {
                continue;
            }
            if !accept {
                let entry = ordinary.entry(target_name).or_default();
                entry.0.extend(fds.iter().copied());
                entry.1.extend(std::iter::repeat_n(fd_name, fds.len()));
                entry.2.push(socket_name);
                continue;
            }
            let fds = if accept {
                let fd = self
                    .services
                    .get(&socket_name)
                    .and_then(|socket| socket.listeners.first())
                    .map(|listener| listener.accept_connection())
                    .transpose()
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot accept a connection from {socket_name}: {error}"
                        ))
                    })?
                    .flatten();
                let Some(fd) = fd else {
                    continue;
                };
                vec![fd]
            } else {
                fds
            };
            if let Some(service) = self.services.get_mut(&target_name) {
                if accept {
                    service.set_activation_connection(fds[0]);
                } else {
                    service.set_activation_fds(fds, vec![fd_name]);
                }
            }
            if let Err(error) = self.start_with_origin(&target_name, false) {
                if let Some(service) = self.services.get_mut(&target_name) {
                    service.clear_activation_fds();
                }
                eprintln!(
                    "fractald: socket {socket_name} could not activate {target_name}: {error}"
                );
            }
        }
        for (target_name, (fds, names, socket_names)) in ordinary {
            if let Some(service) = self.services.get_mut(&target_name) {
                service.set_activation_fds(fds, names);
            }
            if let Err(error) = self.start_with_origin(&target_name, false) {
                if let Some(service) = self.services.get_mut(&target_name) {
                    service.clear_activation_fds();
                }
                eprintln!(
                    "fractald: sockets {} could not activate {target_name}: {error}",
                    socket_names.join(", ")
                );
            }
        }
        Ok(())
    }

    fn progress_trigger_activation(&mut self, now: Instant) {
        let activations = self
            .services
            .values_mut()
            .filter_map(|trigger| {
                trigger
                    .trigger_due(now)
                    .map(|target| (trigger.spec().name.clone(), target))
            })
            .collect::<Vec<_>>();
        for (trigger_name, target) in activations {
            let Some(target_name) = self.resolve_name(&target).map(str::to_owned) else {
                eprintln!(
                    "fractald: trigger {trigger_name} refers to unavailable service {target}"
                );
                continue;
            };
            let should_start = self.services.get(&target_name).is_some_and(|service| {
                matches!(
                    service.record.state(),
                    ServiceState::Defined
                        | ServiceState::Exited
                        | ServiceState::Skipped
                        | ServiceState::Failed
                )
            });
            if should_start {
                if let Err(error) = self.start_with_origin(&target_name, false) {
                    eprintln!(
                        "fractald: trigger {trigger_name} could not activate {target_name}: {error}"
                    );
                }
            }
        }
    }

    /// Advance one transaction. A true result means the transaction is still pending.
    fn progress_start_job(
        &mut self,
        job: &mut StartJob,
        now: Instant,
    ) -> Result<bool, SupervisorError> {
        let complete = job
            .selected
            .iter()
            .all(|name| job.attempted.contains(name) && self.service_start_complete(name));
        if !complete && job.deadline.is_some_and(|deadline| now >= deadline) {
            self.timeout_start_job(job, now)?;
            return Ok(false);
        }
        if job.conflicts.iter().any(|name| {
            self.services
                .get(name)
                .is_some_and(|service| !service.is_stopped())
        }) {
            return Ok(true);
        }
        let mut made_progress = true;
        while made_progress {
            made_progress = false;
            let names: Vec<String> = job.selected.iter().cloned().collect();
            for name in names {
                if job.attempted.contains(&name) {
                    continue;
                }
                if name != job.root && self.service_is_ready_without_start(&name) {
                    job.attempted.insert(name);
                    made_progress = true;
                    continue;
                }
                if !self.start_dependencies_ready(&name, job) {
                    continue;
                }
                let result = self.service_mut(&name)?.start(now);
                job.attempted.insert(name.clone());
                made_progress = true;
                match result {
                    Ok(()) => {
                        job.started.insert(name);
                    }
                    Err(error) if job.required.contains(&name) => {
                        self.rollback_start_job(job, now);
                        return Err(error);
                    }
                    Err(_) => {}
                }
            }
            if self.required_dependency_failed(job) {
                self.rollback_start_job(job, now);
                return Ok(false);
            }
        }

        if self.required_dependency_failed(job) {
            self.rollback_start_job(job, now);
            return Ok(false);
        }
        let complete = job
            .selected
            .iter()
            .all(|name| job.attempted.contains(name) && self.service_start_complete(name));
        if complete { Ok(false) } else { Ok(true) }
    }

    fn timeout_start_job(&mut self, job: &StartJob, now: Instant) -> Result<(), SupervisorError> {
        eprintln!(
            "fractald: start job for {} timed out; rolling back its transaction",
            job.root
        );
        let root = job.root.clone();
        self.service_mut(&root)?.mark_job_timeout(now)?;
        self.rollback_start_job(job, now);
        self.request_manager_action(job.timeout_action);
        Ok(())
    }

    fn service_is_ready_without_start(&self, name: &str) -> bool {
        let Some(service) = self.services.get(name) else {
            return false;
        };
        matches!(
            service.record.state(),
            ServiceState::Active | ServiceState::Exited | ServiceState::Skipped
        ) || service.ready_for_dependency()
    }

    fn check_requisites(&self, name: &str) -> Result<(), SupervisorError> {
        let Some(service) = self.services.get(name) else {
            return Err(SupervisorError::new(format!(
                "cannot check requisites for unknown service {name}"
            )));
        };
        for requisite in &service.spec().dependencies.requisite {
            let Some(requisite_name) = self.resolve_name(requisite) else {
                return Err(SupervisorError::new(format!(
                    "requisite {requisite} for {name} is unavailable"
                )));
            };
            let Some(requisite_service) = self.services.get(requisite_name) else {
                return Err(SupervisorError::new(format!(
                    "requisite {requisite_name} for {name} is unavailable"
                )));
            };
            let active = matches!(
                requisite_service.record.state(),
                ServiceState::Active | ServiceState::Exited
            ) || requisite_service.ready_for_dependency();
            if !active {
                return Err(SupervisorError::new(format!(
                    "requisite {requisite_name} for {name} is not active"
                )));
            }
        }
        Ok(())
    }

    fn service_start_complete(&self, name: &str) -> bool {
        let Some(service) = self.services.get(name) else {
            return false;
        };
        match service.record.state() {
            ServiceState::Running => service.ready_for_dependency(),
            ServiceState::Active
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed => true,
            _ => false,
        }
    }

    fn start_dependencies_ready(&self, name: &str, job: &StartJob) -> bool {
        let Some(service) = self.services.get(name) else {
            return false;
        };
        let required_blockers = self
            .graph()
            .map(|graph| graph.required_dependencies(name))
            .unwrap_or_else(|_| service.spec().dependencies.requires.clone())
            .into_iter()
            .filter(|dependency| job.selected.contains(dependency))
            .collect::<BTreeSet<_>>();
        let mut blockers = BTreeSet::new();
        blockers.extend(required_blockers.iter().cloned());
        blockers.extend(
            service
                .spec()
                .dependencies
                .after
                .iter()
                .filter(|dependency| job.selected.contains(*dependency))
                .cloned(),
        );
        for other in &job.selected {
            if self
                .services
                .get(other)
                .is_some_and(|other| other.spec().dependencies.before.contains(name))
            {
                blockers.insert(other.clone());
            }
        }
        blockers.into_iter().all(|dependency| {
            if required_blockers.contains(&dependency)
                && self.required_dependency_is_unavailable(&dependency)
            {
                return false;
            }
            self.service_is_ready_without_start(&dependency)
                || (job.attempted.contains(&dependency) && self.service_start_complete(&dependency))
        })
    }

    fn required_dependency_failed(&self, job: &StartJob) -> bool {
        job.required.iter().any(|name| {
            job.attempted.contains(name) && self.required_dependency_is_unavailable(name)
        })
    }

    fn required_dependency_is_unavailable(&self, name: &str) -> bool {
        let Some(service) = self.services.get(name) else {
            return true;
        };
        match service.record.state() {
            ServiceState::Failed => true,
            ServiceState::Skipped => !skipped_mount_is_available(service.spec()),
            _ => false,
        }
    }

    fn rollback_start_job(&mut self, job: &StartJob, now: Instant) {
        for name in job.order.iter().rev() {
            if job.started.contains(name) {
                let _ = self.service_mut(name).and_then(|service| service.stop(now));
            }
        }
    }

    fn cancel_start_jobs(&mut self, name: &str) -> Result<(), SupervisorError> {
        let mut jobs = std::mem::take(&mut self.start_jobs);
        let now = Instant::now();
        let mut pending = Vec::new();
        for job in jobs.drain(..) {
            if job.root == name || job.selected.contains(name) {
                self.rollback_start_job(&job, now);
            } else {
                pending.push(job);
            }
        }
        self.start_jobs = pending;
        Ok(())
    }

    fn cancel_isolation_jobs(&mut self, keep: &BTreeSet<String>) {
        let mut jobs = std::mem::take(&mut self.start_jobs);
        let now = Instant::now();
        let mut pending = Vec::new();
        for job in jobs.drain(..) {
            if job.selected.iter().all(|name| keep.contains(name)) {
                pending.push(job);
            } else {
                self.rollback_start_job(&job, now);
            }
        }
        self.start_jobs = pending;
    }

    fn conflicts_for(
        &self,
        selected: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, SupervisorError> {
        let mut conflicts = BTreeSet::new();
        let graph = self.graph()?;
        for candidate_name in self.services.keys() {
            let candidate_is_selected = selected.contains(candidate_name);
            for dependency in graph.conflict_dependencies(candidate_name) {
                let Some(dependency_name) = self.resolve_name(&dependency) else {
                    continue;
                };
                if candidate_is_selected {
                    conflicts.insert(dependency_name.to_owned());
                } else if selected.contains(dependency_name) {
                    conflicts.insert(candidate_name.clone());
                }
            }
        }
        if conflicts.iter().any(|name| selected.contains(name)) {
            return Err(SupervisorError::new(
                "start transaction contains mutually conflicting services",
            ));
        }
        Ok(conflicts)
    }

    fn stop_dependents(
        &mut self,
        source: &str,
        include_part_of: bool,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        let dependents = self.dependent_names(source, include_part_of);
        for dependent in dependents {
            self.cancel_start_jobs(&dependent)?;
            self.service_mut(&dependent)?.stop(now)?;
        }
        Ok(())
    }

    fn dependent_names(&self, source: &str, include_part_of: bool) -> BTreeSet<String> {
        self.services
            .iter()
            .filter_map(|(name, service)| {
                if name == source {
                    return None;
                }
                let binds_to_source = service
                    .spec()
                    .dependencies
                    .binds_to
                    .iter()
                    .any(|dependency| self.resolve_name(dependency) == Some(source));
                let part_of_source = include_part_of
                    && service
                        .spec()
                        .dependencies
                        .part_of
                        .iter()
                        .any(|dependency| self.resolve_name(dependency) == Some(source));
                (binds_to_source || part_of_source).then_some(name.clone())
            })
            .collect()
    }

    fn propagate_bindings(
        &mut self,
        before: &BTreeMap<String, ServiceState>,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        let mut bindings = BTreeSet::new();
        let mut part_of = BTreeSet::new();
        for (source, previous) in before {
            let Some(service) = self.services.get(source) else {
                continue;
            };
            let current = service.record.state();
            if !service_was_up(*previous) || !service_is_down(current) {
                continue;
            }
            bindings.insert(source.clone());
            if !service.wants_running {
                part_of.insert(source.clone());
            }
        }
        for source in bindings {
            self.stop_dependents(&source, part_of.contains(&source), now)?;
        }
        Ok(())
    }

    fn propagate_failures(
        &mut self,
        before: &BTreeMap<String, ServiceState>,
    ) -> Result<(), SupervisorError> {
        let failed = before
            .iter()
            .filter_map(|(source, previous)| {
                let service = self.services.get(source)?;
                if *previous == ServiceState::Failed
                    || service.record.state() != ServiceState::Failed
                {
                    return None;
                }
                Some((
                    service.spec().failure_action,
                    service
                        .spec()
                        .dependencies
                        .on_failure
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                ))
            })
            .collect::<Vec<_>>();
        for (action, dependencies) in failed {
            self.request_manager_action(action);
            for dependency in dependencies {
                let Some(name) = self.resolve_name(&dependency).map(str::to_owned) else {
                    continue;
                };
                if let Err(error) = self.start_with_origin(&name, false) {
                    eprintln!("fractald: OnFailure unit {name} could not start: {error}");
                }
            }
        }
        Ok(())
    }

    fn reap_unneeded(&mut self, now: Instant) -> Result<(), SupervisorError> {
        if !self.start_jobs.is_empty() {
            return Ok(());
        }
        loop {
            let graph = self.graph()?;
            let unneeded = self
                .services
                .iter()
                .filter_map(|(name, service)| {
                    if !service.spec().stop_when_unneeded
                        || !service_was_up(service.record.state())
                        || self.has_active_dependent(name, &graph)
                    {
                        return None;
                    }
                    Some(name.clone())
                })
                .collect::<Vec<_>>();
            if unneeded.is_empty() {
                return Ok(());
            }
            for name in unneeded {
                if self
                    .services
                    .get(&name)
                    .is_some_and(|service| service.spec().stop_when_unneeded)
                {
                    self.cancel_start_jobs(&name)?;
                    self.service_mut(&name)?.stop(now)?;
                }
            }
        }
    }

    fn has_active_dependent(&self, source: &str, graph: &DependencyGraph) -> bool {
        for (name, service) in &self.services {
            if name == source || !service_was_up(service.record.state()) {
                continue;
            }
            let required = graph.required_dependencies(name);
            if required
                .iter()
                .chain(service.spec().dependencies.wants.iter())
                .any(|dependency| self.resolve_name(dependency) == Some(source))
            {
                return true;
            }
            if service.spec().service_type == ServiceType::Socket
                && service
                    .spec()
                    .socket_service
                    .as_deref()
                    .and_then(|dependency| self.resolve_name(dependency))
                    == Some(source)
            {
                return true;
            }
        }
        false
    }

    fn propagate_successes(
        &mut self,
        before: &BTreeMap<String, ServiceState>,
    ) -> Result<(), SupervisorError> {
        let successful = before
            .iter()
            .filter_map(|(source, previous)| {
                let service = self.services.get(source)?;
                if matches!(previous, ServiceState::Active | ServiceState::Exited)
                    || !matches!(
                        service.record.state(),
                        ServiceState::Active | ServiceState::Exited
                    )
                    || service
                        .record
                        .last_exit()
                        .is_some_and(|exit| !exit.is_success())
                {
                    return None;
                }
                let manager_action = service
                    .record
                    .last_exit()
                    .is_some_and(|exit| exit.is_success())
                    .then_some(service.spec().success_action)
                    .unwrap_or(ManagerAction::None);
                Some((
                    manager_action,
                    service
                        .spec()
                        .dependencies
                        .on_success
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                ))
            })
            .collect::<Vec<_>>();
        for (action, dependencies) in successful {
            self.request_manager_action(action);
            for dependency in dependencies {
                let Some(name) = self.resolve_name(&dependency).map(str::to_owned) else {
                    continue;
                };
                if let Err(error) = self.start_with_origin(&name, false) {
                    eprintln!("fractald: OnSuccess unit {name} could not start: {error}");
                }
            }
        }
        Ok(())
    }

    fn request_manager_action(&mut self, action: ManagerAction) {
        if action == ManagerAction::None {
            return;
        }
        let replace = self.pending_manager_action.is_none_or(|pending| {
            manager_action_priority(action) > manager_action_priority(pending)
        });
        if replace {
            self.pending_manager_action = Some(action);
        }
    }

    fn graph(&self) -> Result<DependencyGraph, SupervisorError> {
        let mut graph = DependencyGraph::default();
        for service in self.services.values() {
            graph.add(service.spec().clone()).map_err(|error| {
                SupervisorError::new(format!("cannot build service graph: {error:?}"))
            })?;
        }
        Ok(graph)
    }

    fn dynamic_user_ids(&self) -> BTreeSet<u32> {
        self.services
            .values()
            .filter_map(|service| dynamic_identity_from_spec(service.spec()))
            .flat_map(|identity| [identity.uid, identity.gid])
            .collect()
    }

    fn dynamic_identity_for(&self, name: &str) -> Option<DynamicIdentity> {
        self.services
            .get(name)
            .and_then(|service| dynamic_identity_from_spec(service.spec()))
    }

    fn service_mut(&mut self, name: &str) -> Result<&mut ManagedService, SupervisorError> {
        self.services
            .get_mut(name)
            .ok_or_else(|| SupervisorError::new(format!("unknown service {name}")))
    }

    fn canonical_name(&self, name: &str) -> String {
        self.resolve_name(name)
            .map(str::to_owned)
            .unwrap_or_else(|| name.to_owned())
    }
}

fn collect_aliases(
    services: &BTreeMap<String, ManagedService>,
) -> Result<BTreeMap<String, String>, SupervisorError> {
    let mut aliases = BTreeMap::new();
    for (canonical, service) in services {
        for alias in &service.spec().aliases {
            if alias == canonical {
                continue;
            }
            if let Some(previous) = aliases.insert(alias.clone(), canonical.clone()) {
                if previous != *canonical {
                    return Err(SupervisorError::new(format!(
                        "service alias {alias} refers to both {previous} and {canonical}"
                    )));
                }
            }
        }
    }
    for canonical in services.keys() {
        if aliases.contains_key(canonical) {
            return Err(SupervisorError::new(format!(
                "service alias {canonical} conflicts with a service name"
            )));
        }
    }
    Ok(aliases)
}

// Keep dynamic identities in FractalD's transient
// users.  FractalD does not edit passwd or group databases; children receive
// the numeric identity directly and the supervisor reserves it for the life
// of the loaded unit set.
const DYNAMIC_USER_MIN: u32 = 61_184;
const DYNAMIC_USER_MAX: u32 = 65_519;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DynamicIdentity {
    uid: u32,
    gid: u32,
}

fn dynamic_identity_from_spec(spec: &ServiceSpec) -> Option<DynamicIdentity> {
    if !spec.dynamic_user {
        return None;
    }
    Some(DynamicIdentity {
        uid: spec.user.as_deref()?.parse().ok()?,
        gid: spec.group.as_deref()?.parse().ok()?,
    })
}

fn allocate_dynamic_identity(
    name: &str,
    reserved: &BTreeSet<u32>,
) -> Result<DynamicIdentity, SupervisorError> {
    let span = DYNAMIC_USER_MAX - DYNAMIC_USER_MIN + 1;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    let start = (hasher.finish() % u64::from(span)) as u32;
    for offset in 0..span {
        let id = DYNAMIC_USER_MIN + (start + offset) % span;
        if reserved.contains(&id) || system_identity_occupied(id) {
            continue;
        }
        return Ok(DynamicIdentity { uid: id, gid: id });
    }
    Err(SupervisorError::new(format!(
        "dynamic user range {DYNAMIC_USER_MIN}-{DYNAMIC_USER_MAX} is exhausted"
    )))
}

fn system_identity_occupied(id: u32) -> bool {
    [
        ("/etc/passwd", 2),
        ("/etc/group", 2),
        ("/etc/subuid", 1),
        ("/etc/subgid", 1),
    ]
    .into_iter()
    .any(|(path, field)| {
        fs::read_to_string(path).is_ok_and(|contents| {
            contents.lines().any(|line| {
                line.split(':')
                    .nth(field)
                    .and_then(|value| value.parse::<u32>().ok())
                    == Some(id)
            })
        })
    })
}

#[derive(Debug)]
struct ManagedService {
    record: ServiceRecord,
    child: Option<Child>,
    pidfd: Option<PidFd>,
    process_group: Option<u32>,
    forking_pending: bool,
    forking_parent_exited: bool,
    adopted_pid: Option<u32>,
    notify_socket: Option<UnixDatagram>,
    notify_path: Option<PathBuf>,
    notify_ready: bool,
    dbus_ready: bool,
    dbus_probe: Option<Child>,
    dbus_probe_index: usize,
    dbus_probe_at: Option<Instant>,
    watchdog_deadline: Option<Instant>,
    listeners: Vec<ManagedListener>,
    activation_fd_names: Vec<String>,
    activation_fds: Vec<RawFd>,
    activation_owned_fds: Vec<RawFd>,
    stop_child: Option<Child>,
    stop_pidfd: Option<PidFd>,
    stop_pid: Option<u32>,
    reload_child: Option<Child>,
    reload_pidfd: Option<PidFd>,
    reload_pid: Option<u32>,
    reload_failed: bool,
    start_helper: Option<HelperProcess>,
    exec_condition_helper: Option<HelperProcess>,
    exec_conditions_done: bool,
    start_post_helper: Option<HelperProcess>,
    active_stop_helper: Option<HelperProcess>,
    stop_post_helper: Option<HelperProcess>,
    stop_signal: Option<i32>,
    stop_signal_sent: bool,
    start_deadline: Option<Instant>,
    stop_deadline: Option<Instant>,
    runtime_deadline: Option<Instant>,
    restart_at: Option<Instant>,
    restart_after_stop: bool,
    wants_running: bool,
    start_attempts: VecDeque<Instant>,
    cgroup_path: Option<PathBuf>,
    oom_events: Option<u64>,
    directories: Vec<ManagedDirectory>,
    credential_directory: Option<PathBuf>,
    private_tmp_paths: Option<PrivateTmpPaths>,
    trigger_started: Option<Instant>,
    trigger_last_fire: Option<Instant>,
    timer_boot_fired: bool,
    timer_persistent_pending: bool,
    timer_random_delay: Duration,
    timer_accuracy_delay: Duration,
    calendar_fires: BTreeMap<String, u64>,
    calendar_pending: BTreeMap<String, (u64, SystemTime)>,
    path_observations: Vec<PathObservation>,
    path_initial_check: bool,
}

impl ManagedService {
    fn new_with_dynamic_identity(
        mut spec: ServiceSpec,
        identity: Option<DynamicIdentity>,
    ) -> Result<Self, SupervisorError> {
        if spec.dynamic_user {
            let identity = identity.ok_or_else(|| {
                SupervisorError::new(format!(
                    "cannot allocate a dynamic identity for {}",
                    spec.name
                ))
            })?;
            spec.user = Some(identity.uid.to_string());
            spec.group = Some(identity.gid.to_string());
            if spec.supplementary_groups.is_none() {
                // Numeric identities do not go through initgroups(3). An
                // explicit empty list prevents inherited manager groups from
                // crossing the DynamicUser boundary.
                spec.supplementary_groups = Some(Vec::new());
            }
            if spec.protect_system == ProtectSystemMode::No {
                spec.protect_system = ProtectSystemMode::Strict;
            }
            if spec.protect_home == ProtectHomeMode::No {
                spec.protect_home = ProtectHomeMode::ReadOnly;
            }
            if spec.private_tmp == PrivateTmpMode::No {
                spec.private_tmp = PrivateTmpMode::Yes;
            }
        }
        let record = ServiceRecord::new(spec).map_err(|error| {
            SupervisorError::new(format!("invalid service specification: {error:?}"))
        })?;
        Ok(Self {
            record,
            child: None,
            pidfd: None,
            process_group: None,
            forking_pending: false,
            forking_parent_exited: false,
            adopted_pid: None,
            notify_socket: None,
            notify_path: None,
            notify_ready: true,
            dbus_ready: true,
            dbus_probe: None,
            dbus_probe_index: 0,
            dbus_probe_at: None,
            watchdog_deadline: None,
            listeners: Vec::new(),
            activation_fd_names: Vec::new(),
            activation_fds: Vec::new(),
            activation_owned_fds: Vec::new(),
            stop_child: None,
            stop_pidfd: None,
            stop_pid: None,
            reload_child: None,
            reload_pidfd: None,
            reload_pid: None,
            reload_failed: false,
            start_helper: None,
            exec_condition_helper: None,
            exec_conditions_done: false,
            start_post_helper: None,
            active_stop_helper: None,
            stop_post_helper: None,
            stop_signal: None,
            stop_signal_sent: false,
            start_deadline: None,
            stop_deadline: None,
            runtime_deadline: None,
            restart_at: None,
            restart_after_stop: false,
            wants_running: false,
            start_attempts: VecDeque::new(),
            cgroup_path: None,
            oom_events: None,
            directories: Vec::new(),
            credential_directory: None,
            private_tmp_paths: None,
            trigger_started: None,
            trigger_last_fire: None,
            timer_boot_fired: false,
            timer_persistent_pending: false,
            timer_random_delay: Duration::ZERO,
            timer_accuracy_delay: Duration::ZERO,
            calendar_fires: BTreeMap::new(),
            calendar_pending: BTreeMap::new(),
            path_observations: Vec::new(),
            path_initial_check: true,
        })
    }

    fn spec(&self) -> &ServiceSpec {
        self.record.spec()
    }

    fn snapshot(&self) -> ServiceSnapshot {
        ServiceSnapshot {
            name: self.spec().name.clone(),
            state: self.record.state(),
            pid: self.record.pid(),
            generation: self.record.generation(),
            restart_count: self.record.restart_count(),
            last_exit: self.record.last_exit(),
        }
    }

    fn mount_is_lost(&self) -> bool {
        self.spec().service_type == ServiceType::Mount
            && self.record.state() == ServiceState::Active
            && self
                .spec()
                .mount_where
                .as_deref()
                .is_some_and(|path| !path_is_mount_point(path))
    }

    fn device_is_lost(&self) -> bool {
        self.record.state() == ServiceState::Active
            && self
                .spec()
                .device_path
                .as_deref()
                .is_some_and(|path| !path.exists())
    }

    fn mark_mount_lost(&mut self) -> Result<(), SupervisorError> {
        if !self.mount_is_lost() {
            return Ok(());
        }
        eprintln!(
            "fractald: mount {} disappeared; marking it inactive",
            self.spec().name
        );
        self.wants_running = false;
        self.record
            .transition(Event::StopRequested)
            .map_err(|error| {
                SupervisorError::new(format!(
                    "cannot record lost mount {}: {error:?}",
                    self.spec().name
                ))
            })?;
        self.cleanup_directories();
        Ok(())
    }

    fn mark_device_lost(&mut self) -> Result<(), SupervisorError> {
        if !self.device_is_lost() {
            return Ok(());
        }
        eprintln!(
            "fractald: device unit {} disappeared; marking it inactive",
            self.spec().name
        );
        self.wants_running = false;
        self.record
            .transition(Event::StopRequested)
            .map_err(|error| {
                SupervisorError::new(format!(
                    "cannot record lost device {}: {error:?}",
                    self.spec().name
                ))
            })?;
        self.cleanup_directories();
        Ok(())
    }

    fn direct_child_pids(&self) -> Vec<u32> {
        let mut pids = Vec::new();
        if let Some(pid) = self.record.pid() {
            pids.push(pid);
        }
        if let Some(child) = self.child.as_ref() {
            pids.push(child.id());
        }
        if let Some(pid) = self.stop_pid {
            pids.push(pid);
        }
        if let Some(child) = self.stop_child.as_ref() {
            pids.push(child.id());
        }
        if let Some(pid) = self.reload_pid {
            pids.push(pid);
        }
        if let Some(child) = self.reload_child.as_ref() {
            pids.push(child.id());
        }
        if let Some(child) = self.dbus_probe.as_ref() {
            pids.push(child.id());
        }
        for helper in [
            self.start_helper.as_ref(),
            self.exec_condition_helper.as_ref(),
            self.start_post_helper.as_ref(),
            self.active_stop_helper.as_ref(),
            self.stop_post_helper.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            pids.push(helper.pid);
        }
        pids
    }

    fn ready_for_dependency(&self) -> bool {
        if self.record.state() != ServiceState::Running
            || self.forking_pending
            || self.start_post_helper.is_some()
        {
            return false;
        }
        match self.spec().service_type {
            ServiceType::Oneshot => false,
            ServiceType::Mount | ServiceType::Swap => false,
            ServiceType::Notify => self.notify_ready,
            ServiceType::Dbus => self.dbus_ready,
            _ => true,
        }
    }

    fn conditions_met(&self) -> bool {
        self.spec()
            .conditions
            .iter()
            .all(|condition| condition_matches(condition, self.spec()))
    }

    fn assertions_met(&self) -> bool {
        self.spec()
            .assertions
            .iter()
            .all(|assertion| condition_matches(assertion, self.spec()))
    }

    fn is_stopped(&self) -> bool {
        self.child.is_none()
            && self.pidfd.is_none()
            && self.process_group.is_none()
            && !self.forking_pending
            && !self.forking_parent_exited
            && self.adopted_pid.is_none()
            && self.notify_socket.is_none()
            && self.dbus_probe.is_none()
            && self.watchdog_deadline.is_none()
            && self.listeners.is_empty()
            && self.activation_fd_names.is_empty()
            && self.activation_fds.is_empty()
            && self.activation_owned_fds.is_empty()
            && self.stop_child.is_none()
            && self.stop_pidfd.is_none()
            && self.reload_child.is_none()
            && self.reload_pidfd.is_none()
            && self.start_helper.is_none()
            && self.exec_condition_helper.is_none()
            && self.start_post_helper.is_none()
            && self.active_stop_helper.is_none()
            && self.stop_post_helper.is_none()
            && matches!(
                self.record.state(),
                ServiceState::Defined
                    | ServiceState::Exited
                    | ServiceState::Skipped
                    | ServiceState::Failed
            )
            && self.restart_at.is_none()
            && self.runtime_deadline.is_none()
            && self.credential_directory.is_none()
            && self.private_tmp_paths.is_none()
    }

    fn start(&mut self, now: Instant) -> Result<(), SupervisorError> {
        self.wants_running = true;
        self.restart_after_stop = false;
        self.restart_at = None;
        if matches!(
            self.record.state(),
            ServiceState::Running | ServiceState::Starting | ServiceState::Active
        ) {
            return Ok(());
        }
        if self.record.state() == ServiceState::Stopping {
            self.restart_after_stop = true;
            return Ok(());
        }
        if self.record.state() == ServiceState::Exited
            && (self.active_stop_helper.is_some() || self.stop_post_helper.is_some())
        {
            self.restart_after_stop = true;
            return Ok(());
        }
        self.enforce_start_limit(now)?;
        self.stop_signal = None;
        self.stop_signal_sent = false;
        self.runtime_deadline = None;
        let actions = self
            .record
            .transition(Event::StartRequested)
            .map_err(|error| {
                SupervisorError::new(format!("cannot start {}: {error:?}", self.spec().name))
            })?;
        if !self.conditions_met() {
            let generation = self.record.generation();
            self.record
                .transition(Event::ConditionSkipped { generation })
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot skip {} after a false condition: {error:?}",
                        self.spec().name
                    ))
                })?;
            self.start_deadline = None;
            return Ok(());
        }
        if !self.assertions_met() {
            let generation = self.record.generation();
            self.record
                .transition(Event::StartFailed { generation })
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot fail {} after a false assertion: {error:?}",
                        self.spec().name
                    ))
                })?;
            self.start_deadline = None;
            return Ok(());
        }
        self.exec_conditions_done = false;
        let generation = self.record.generation();
        self.credential_directory = match prepare_credentials(self.spec()) {
            Ok(directory) => directory,
            Err(error) => {
                self.cleanup_directories();
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot prepare credentials for {}: {error}",
                    self.spec().name
                )));
            }
        };
        self.directories = match prepare_directories(self.spec()) {
            Ok(directories) => directories,
            Err(error) => {
                self.cleanup_directories();
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot prepare service directories for {}: {error}",
                    self.spec().name
                )));
            }
        };
        self.private_tmp_paths = match prepare_private_tmp(self.spec()) {
            Ok(paths) => paths,
            Err(error) => {
                self.cleanup_directories();
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot prepare private temporary directories for {}: {error}",
                    self.spec().name
                )));
            }
        };
        self.start_deadline = timeout_deadline(now, self.spec().start_timeout);
        self.apply_start_actions(actions, now)
    }

    fn stop(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let signal = self.spec().kill_signal;
        self.stop_with_signal(now, signal)
    }

    fn mark_job_timeout(&mut self, now: Instant) -> Result<(), SupervisorError> {
        self.wants_running = false;
        self.restart_after_stop = false;
        self.restart_at = None;
        self.runtime_deadline = None;
        self.kill_dbus_probe();
        let generation = self.record.generation();
        match self.record.state() {
            ServiceState::Defined
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed
            | ServiceState::Backoff => {
                self.record
                    .transition(Event::JobTimedOut)
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot record job timeout for {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.start_deadline = None;
                self.cleanup_directories();
            }
            ServiceState::Starting => {
                let actions = self
                    .record
                    .transition(Event::StartTimedOut { generation })
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot time out start job for {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.apply_actions(actions)?;
                self.start_deadline = None;
                self.kill_start_helper();
                self.kill_exec_condition_helper();
                self.kill_start_post_helper();
                if self.record.state() == ServiceState::Stopping && self.record.pid().is_some() {
                    self.stop_deadline = timeout_deadline(now, self.spec().stop_timeout);
                }
            }
            ServiceState::Running => {
                let actions = self
                    .record
                    .transition(Event::StartFailed { generation })
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot fail timed out job for {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.apply_actions(actions)?;
                self.start_deadline = None;
                self.kill_start_helper();
                self.kill_exec_condition_helper();
                self.kill_start_post_helper();
                if self.record.state() == ServiceState::Stopping && self.record.pid().is_some() {
                    self.stop_deadline = timeout_deadline(now, self.spec().stop_timeout);
                }
            }
            ServiceState::Active | ServiceState::Stopping => {}
        }
        Ok(())
    }

    fn stop_for_restart(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let signal = self
            .spec()
            .restart_kill_signal
            .unwrap_or(self.spec().kill_signal);
        self.stop_with_signal(now, signal)
    }

    fn stop_with_signal(&mut self, now: Instant, signal: i32) -> Result<(), SupervisorError> {
        self.wants_running = false;
        self.restart_after_stop = false;
        self.restart_at = None;
        self.runtime_deadline = None;
        self.kill_dbus_probe();
        if matches!(
            self.record.state(),
            ServiceState::Starting | ServiceState::Running
        ) {
            self.stop_signal = Some(signal);
            self.stop_signal_sent = false;
        }
        let was_active = self.record.state() == ServiceState::Active;
        let actions = self
            .record
            .transition(Event::StopRequested)
            .map_err(|error| {
                SupervisorError::new(format!("cannot stop {}: {error:?}", self.spec().name))
            })?;
        self.apply_actions(actions)?;
        if self.spec().service_type == ServiceType::Socket {
            self.close_listeners(self.spec().remove_on_stop);
            self.clear_activation_fds();
        }
        if self.record.state() == ServiceState::Stopping && self.record.pid().is_none() {
            self.kill_exec_condition_helper();
            self.kill_start_helper();
            self.kill_start_post_helper();
            self.start_deadline = None;
            self.record
                .transition(Event::StopCompleted)
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot complete stop for {}: {error:?}",
                        self.spec().name
                    ))
                })?;
        }
        if was_active && self.record.state() == ServiceState::Exited {
            if self.spec().stop.is_some() {
                self.spawn_active_stop()?;
                self.stop_deadline = timeout_deadline(now, self.spec().stop_timeout);
            } else {
                self.spawn_stop_post(None);
            }
        }
        if self.record.state() == ServiceState::Exited {
            self.reset_trigger_state();
            if self.active_stop_helper.is_none() && self.stop_post_helper.is_none() {
                self.cleanup_directories();
            }
        }
        if let (Some(pid), Some(_)) = (self.record.pid(), self.spec().stop.as_ref()) {
            if self.spawn_stop_command(pid).is_err() {
                self.send_signal(pid, self.stop_signal.unwrap_or(signal))?;
            }
        }
        if self.record.state() == ServiceState::Stopping && self.record.pid().is_some() {
            self.stop_deadline = if self.spec().stop_timeout == Duration::MAX {
                None
            } else {
                now.checked_add(self.spec().stop_timeout)
            };
        } else {
            self.stop_deadline = None;
        }
        Ok(())
    }

    fn restart(&mut self, now: Instant) -> Result<(), SupervisorError> {
        self.wants_running = true;
        self.restart_at = None;
        match self.record.state() {
            ServiceState::Running | ServiceState::Starting => {
                self.stop_for_restart(now)?;
                self.wants_running = true;
                self.restart_after_stop = true;
                if self.record.state() == ServiceState::Exited {
                    self.restart_after_stop = false;
                    self.start(now)?;
                }
                Ok(())
            }
            ServiceState::Stopping => {
                self.restart_after_stop = true;
                Ok(())
            }
            ServiceState::Backoff => {
                self.record
                    .transition(Event::StopRequested)
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot restart {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.start(now)
            }
            ServiceState::Active => {
                self.stop(now)?;
                self.start(now)
            }
            ServiceState::Defined
            | ServiceState::Exited
            | ServiceState::Skipped
            | ServiceState::Failed => self.start(now),
        }
    }

    fn reload(&mut self) -> Result<(), SupervisorError> {
        if self.record.state() != ServiceState::Running {
            return Err(SupervisorError::new(format!(
                "service {} is not running",
                self.spec().name
            )));
        }
        if self.spec().reload.is_none() {
            return Err(SupervisorError::new(format!(
                "service {} has no reload command",
                self.spec().name
            )));
        }
        if self.reload_child.is_some() || self.reload_pidfd.is_some() {
            return Ok(());
        }
        let pid = self.record.pid().ok_or_else(|| {
            SupervisorError::new(format!("service {} has no process", self.spec().name))
        })?;
        self.reload_failed = false;
        self.spawn_reload_command(pid)
    }

    fn reset_failed(&mut self) -> Result<(), SupervisorError> {
        if self.record.state() != ServiceState::Failed {
            return Ok(());
        }
        self.record.transition(Event::Reset).map_err(|error| {
            SupervisorError::new(format!("cannot reset {}: {error:?}", self.spec().name))
        })?;
        self.reset_trigger_state();
        self.start_attempts.clear();
        self.runtime_deadline = None;
        self.cleanup_directories();
        Ok(())
    }

    fn enforce_start_limit(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let interval = self.spec().start_limit_interval;
        let burst = self.spec().start_limit_burst;
        if interval.is_none() && burst.is_none() {
            return Ok(());
        }
        let interval = interval.unwrap_or_else(|| Duration::from_secs(10));
        let burst = burst.unwrap_or(5);
        if interval.is_zero() || burst == 0 {
            self.start_attempts.clear();
            return Ok(());
        }
        while self
            .start_attempts
            .front()
            .is_some_and(|attempt| now.duration_since(*attempt) >= interval)
        {
            self.start_attempts.pop_front();
        }
        if self.start_attempts.len() >= burst as usize {
            let generation = self.record.generation();
            self.record
                .transition(Event::StartLimitHit { generation })
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot record start limit for {}: {error:?}",
                        self.spec().name
                    ))
                })?;
            self.wants_running = false;
            self.restart_at = None;
            self.reset_trigger_state();
            self.cleanup_directories();
            return Err(SupervisorError::new(format!(
                "start limit hit for {} ({burst} starts in {})",
                self.spec().name,
                format!("{interval:?}")
            )));
        }
        self.start_attempts.push_back(now);
        Ok(())
    }

    fn poll(&mut self, now: Instant) -> Result<(), SupervisorError> {
        self.observe_notify(now)?;
        self.observe_dbus(now)?;
        self.check_oom(now)?;
        self.check_watchdog(now)?;
        self.check_runtime_limit(now)?;
        if let Some(exit) = self.observe_exec_condition_helper()? {
            self.handle_exec_condition_exit(exit, now)?;
        }
        if let Some(exit) = self.observe_start_helper()? {
            self.handle_start_helper_exit(exit, now)?;
        }
        if let Some(exit) = self.observe_start_post_helper()? {
            self.handle_start_post_exit(exit, now)?;
        }
        if let Some(exit) = self.observe_active_stop_helper()? {
            self.handle_active_stop_exit(exit, now)?;
        }
        if let Some(exit) = self.observe_stop_post_helper()? {
            self.handle_stop_post_exit(exit, now)?;
        }
        if let Some(exit) = self.observe_reload_exit()? {
            self.handle_reload_exit(exit);
        }
        if let Some(exit) = self.observe_stop_exit()? {
            self.handle_stop_exit(exit)?;
        }
        if let Some(exit) = self.observe_exit()? {
            if self.forking_pending && !self.forking_parent_exited {
                self.handle_forking_parent_exit(exit, now)?;
            } else {
                self.handle_exit(exit, now)?;
            }
        }
        if self.forking_pending && self.forking_parent_exited {
            self.try_adopt_forking(now)?;
        }

        if self.record.state() == ServiceState::Stopping {
            if let Some(deadline) = self.stop_deadline {
                if now >= deadline {
                    let actions = self
                        .record
                        .transition(Event::StopTimedOut {
                            generation: self.record.generation(),
                        })
                        .map_err(|error| {
                            SupervisorError::new(format!(
                                "cannot escalate {}: {error:?}",
                                self.spec().name
                            ))
                        })?;
                    self.apply_actions(actions)?;
                    self.kill_start_helper();
                    self.kill_exec_condition_helper();
                    self.kill_start_post_helper();
                    self.kill_dbus_probe();
                    self.kill_stop_command();
                    self.stop_deadline = None;
                }
            }
        }

        if self.record.state() == ServiceState::Exited && self.active_stop_helper.is_some() {
            if let Some(deadline) = self.stop_deadline {
                if now >= deadline {
                    self.kill_active_stop_helper();
                    self.stop_deadline = None;
                    self.spawn_stop_post(None);
                    if self.stop_post_helper.is_none() {
                        self.cleanup_directories();
                    }
                    self.maybe_start_after_teardown(now)?;
                }
            }
        }

        if self.record.state() == ServiceState::Starting {
            if let Some(deadline) = self.start_deadline {
                if now >= deadline {
                    let actions = self
                        .record
                        .transition(Event::StartTimedOut {
                            generation: self.record.generation(),
                        })
                        .map_err(|error| {
                            SupervisorError::new(format!(
                                "cannot time out {}: {error:?}",
                                self.spec().name
                            ))
                        })?;
                    self.apply_actions(actions)?;
                    self.start_deadline = None;
                    self.kill_start_helper();
                    self.kill_start_post_helper();
                    self.kill_dbus_probe();
                    if self.record.state() == ServiceState::Stopping && self.record.pid().is_some()
                    {
                        self.stop_deadline = timeout_deadline(now, self.spec().stop_timeout);
                    }
                }
            }
        }

        if self.wants_running
            && self.record.state() == ServiceState::Backoff
            && self.restart_at.is_some_and(|deadline| now >= deadline)
        {
            self.restart_at = None;
            if let Err(error) = self.enforce_start_limit(now) {
                eprintln!("fractald: {}", error);
            } else {
                let actions = self
                    .record
                    .transition(Event::RestartDue {
                        generation: self.record.generation(),
                    })
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot restart {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.apply_start_actions(actions, now)?;
            }
        }
        Ok(())
    }

    fn observe_notify(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let Some(socket) = self.notify_socket.as_ref() else {
            return Ok(());
        };
        let mut messages = Vec::new();
        loop {
            let mut buffer = [0_u8; 8192];
            match fractald_platform::receive_socket_credentials(socket.as_raw_fd(), &mut buffer) {
                Ok(Some((length, sender_pid))) => {
                    if self.notification_sender_allowed(sender_pid) {
                        messages.push((buffer[..length].to_vec(), sender_pid));
                    }
                }
                Ok(None) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) if matches!(error.raw_os_error(), Some(90 | 71)) => continue,
                Err(error) => {
                    return Err(SupervisorError::new(format!(
                        "cannot receive readiness notification for {}: {error}",
                        self.spec().name
                    )));
                }
            }
        }
        for (message, _sender_pid) in messages {
            let message = String::from_utf8_lossy(&message);
            let ready = message.lines().any(|line| line.trim() == "READY=1");
            let watchdog = message.lines().any(|line| line.trim() == "WATCHDOG=1");
            if ready {
                if let Some(pid) = self.record.pid() {
                    if self.record.state() == ServiceState::Starting {
                        self.record
                            .transition(Event::Ready {
                                pid,
                                generation: self.record.generation(),
                            })
                            .map_err(|error| {
                                SupervisorError::new(format!(
                                    "cannot accept readiness notification for {}: {error:?}",
                                    self.spec().name
                                ))
                            })?;
                        self.notify_ready = true;
                        self.start_deadline = None;
                        self.arm_watchdog(now);
                        self.arm_runtime_limit(now);
                        self.spawn_start_post(pid)?;
                    }
                }
            }
            if watchdog && self.record.state() == ServiceState::Running {
                self.arm_watchdog(now);
            }
        }
        Ok(())
    }

    fn notification_sender_allowed(&self, sender_pid: u32) -> bool {
        let access = effective_notify_access(self.spec());
        match access {
            NotifyAccess::None => false,
            NotifyAccess::Main => self.record.pid() == Some(sender_pid),
            NotifyAccess::Exec => {
                self.record.pid() == Some(sender_pid)
                    || self.stop_pid == Some(sender_pid)
                    || self.reload_pid == Some(sender_pid)
                    || self
                        .start_helper
                        .as_ref()
                        .is_some_and(|helper| helper.pid == sender_pid)
                    || self
                        .exec_condition_helper
                        .as_ref()
                        .is_some_and(|helper| helper.pid == sender_pid)
                    || self
                        .start_post_helper
                        .as_ref()
                        .is_some_and(|helper| helper.pid == sender_pid)
                    || self
                        .active_stop_helper
                        .as_ref()
                        .is_some_and(|helper| helper.pid == sender_pid)
                    || self
                        .stop_post_helper
                        .as_ref()
                        .is_some_and(|helper| helper.pid == sender_pid)
            }
            NotifyAccess::All => self.process_belongs_to_service(sender_pid),
        }
    }

    fn process_belongs_to_service(&self, pid: u32) -> bool {
        if let Some(path) = self.cgroup_path.as_ref() {
            if cgroup_contains_pid(path, pid) {
                return true;
            }
        }
        self.process_group
            .and_then(|group| process_group_id(pid).map(|sender_group| sender_group == group))
            .unwrap_or(false)
    }

    fn arm_watchdog(&mut self, now: Instant) {
        self.watchdog_deadline = self
            .spec()
            .watchdog
            .and_then(|watchdog| now.checked_add(watchdog));
    }

    fn check_watchdog(&mut self, now: Instant) -> Result<(), SupervisorError> {
        if self.record.state() != ServiceState::Running
            || !self
                .watchdog_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            return Ok(());
        }
        self.watchdog_deadline = None;
        let Some(pid) = self.record.pid() else {
            return Ok(());
        };
        eprintln!(
            "fractald: watchdog expired for {}, sending SIGABRT",
            self.spec().name
        );
        self.send_signal(pid, SIGABRT)
    }

    fn arm_runtime_limit(&mut self, now: Instant) {
        self.runtime_deadline = self
            .spec()
            .runtime_max
            .filter(|runtime_max| !runtime_max.is_zero())
            .and_then(|runtime_max| timeout_deadline(now, runtime_max));
    }

    fn check_runtime_limit(&mut self, now: Instant) -> Result<(), SupervisorError> {
        if self.record.state() != ServiceState::Running
            || !self
                .runtime_deadline
                .is_some_and(|deadline| now >= deadline)
        {
            return Ok(());
        }
        self.runtime_deadline = None;
        let actions = self
            .record
            .transition(Event::RuntimeTimedOut {
                generation: self.record.generation(),
            })
            .map_err(|error| {
                SupervisorError::new(format!(
                    "cannot time out runtime for {}: {error:?}",
                    self.spec().name
                ))
            })?;
        self.apply_actions(actions)?;
        if self.record.state() == ServiceState::Stopping && self.record.pid().is_some() {
            self.stop_deadline = timeout_deadline(now, self.spec().stop_timeout);
        }
        Ok(())
    }

    fn check_oom(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let Some(path) = self.cgroup_path.as_ref() else {
            return Ok(());
        };
        let Some(current) = read_cgroup_oom_events(path) else {
            return Ok(());
        };
        let Some(previous) = self.oom_events.as_mut() else {
            self.oom_events = Some(current);
            return Ok(());
        };
        if current <= *previous {
            *previous = current;
            return Ok(());
        }
        *previous = current;
        if self.record.state() != ServiceState::Running {
            return Ok(());
        }
        match self.spec().oom_policy {
            OomPolicy::Continue => {}
            OomPolicy::Stop => {
                eprintln!(
                    "fractald: out-of-memory event for {}, stopping service",
                    self.spec().name
                );
                self.stop(now)?;
            }
            OomPolicy::Kill => {
                eprintln!(
                    "fractald: out-of-memory event for {}, killing service cgroup",
                    self.spec().name
                );
                if let Err(error) = kill_cgroup(path) {
                    eprintln!(
                        "fractald: cannot kill cgroup for {}: {error}",
                        self.spec().name
                    );
                    if let Some(pid) = self.record.pid() {
                        self.send_signal(pid, SIGKILL)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn observe_dbus(&mut self, now: Instant) -> Result<(), SupervisorError> {
        if self.spec().service_type != ServiceType::Dbus
            || self.dbus_ready
            || self.record.state() != ServiceState::Starting
        {
            return Ok(());
        }
        if let Some(probe) = self.dbus_probe.as_mut() {
            match probe.try_wait() {
                Ok(Some(status)) if status.success() => {
                    self.dbus_probe = None;
                    let pid = self.record.pid().ok_or_else(|| {
                        SupervisorError::new(format!(
                            "D-Bus service {} lost its process before readiness",
                            self.spec().name
                        ))
                    })?;
                    self.record
                        .transition(Event::Ready {
                            pid,
                            generation: self.record.generation(),
                        })
                        .map_err(|error| {
                            SupervisorError::new(format!(
                                "cannot accept D-Bus readiness for {}: {error:?}",
                                self.spec().name
                            ))
                        })?;
                    self.dbus_ready = true;
                    self.dbus_probe_index = 0;
                    self.dbus_probe_at = None;
                    self.start_deadline = None;
                    self.arm_runtime_limit(now);
                    self.spawn_start_post(pid)?;
                    return Ok(());
                }
                Ok(Some(_)) => {
                    self.dbus_probe = None;
                    self.dbus_probe_index += 1;
                    if self.dbus_probe_index >= self.spec().bus_names.len() {
                        self.dbus_probe_index = 0;
                        self.dbus_probe_at = now.checked_add(Duration::from_millis(100));
                    } else {
                        self.dbus_probe_at = Some(now);
                    }
                }
                Ok(None) => return Ok(()),
                Err(error) => {
                    self.dbus_probe = None;
                    self.dbus_probe_at = now.checked_add(Duration::from_millis(100));
                    return Err(SupervisorError::new(format!(
                        "cannot poll D-Bus readiness for {}: {error}",
                        self.spec().name
                    )));
                }
            }
        }
        if self.dbus_probe.is_some() || self.dbus_probe_at.is_some_and(|deadline| now < deadline) {
            return Ok(());
        }
        let bus_name = self
            .spec()
            .bus_names
            .get(self.dbus_probe_index)
            .cloned()
            .ok_or_else(|| {
                SupervisorError::new(format!(
                    "D-Bus service {} has no bus name",
                    self.spec().name
                ))
            })?;
        match spawn_dbus_probe(&bus_name) {
            Ok(probe) => {
                self.dbus_probe = Some(probe);
                self.dbus_probe_at = None;
            }
            Err(error) => {
                self.dbus_probe_at = now.checked_add(Duration::from_secs(1));
                eprintln!(
                    "fractald: cannot probe D-Bus name {} for {}: {error}",
                    bus_name,
                    self.spec().name
                );
            }
        }
        Ok(())
    }

    fn observe_start_helper(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        let name = self.spec().name.clone();
        observe_helper(&mut self.start_helper, "start-pre", &name)
    }

    fn observe_exec_condition_helper(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        let name = self.spec().name.clone();
        observe_helper(&mut self.exec_condition_helper, "exec-condition", &name)
    }

    fn observe_start_post_helper(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        let name = self.spec().name.clone();
        observe_helper(&mut self.start_post_helper, "start-post", &name)
    }

    fn observe_stop_post_helper(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        let name = self.spec().name.clone();
        observe_helper(&mut self.stop_post_helper, "stop-post", &name)
    }

    fn observe_active_stop_helper(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        let name = self.spec().name.clone();
        observe_helper(&mut self.active_stop_helper, "stop", &name)
    }

    fn handle_start_helper_exit(
        &mut self,
        exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        self.start_helper = None;
        if self.record.state() == ServiceState::Stopping {
            self.record
                .transition(Event::StopCompleted)
                .map_err(|error| {
                    SupervisorError::new(format!("cannot complete stop: {error:?}"))
                })?;
            self.start_deadline = None;
            return Ok(());
        }
        if !is_success(exit) {
            self.start_deadline = None;
            self.mark_spawn_failed(self.record.generation(), now)?;
            return Ok(());
        }
        if self.record.state() == ServiceState::Starting {
            self.spawn_main(self.record.generation(), now)?;
        }
        Ok(())
    }

    fn handle_exec_condition_exit(
        &mut self,
        exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        self.exec_condition_helper = None;
        if self.record.state() == ServiceState::Stopping {
            self.record
                .transition(Event::StopCompleted)
                .map_err(|error| {
                    SupervisorError::new(format!("cannot complete stop: {error:?}"))
                })?;
            self.start_deadline = None;
            self.cleanup_directories();
            return Ok(());
        }
        let generation = self.record.generation();
        match exit {
            ExitKind::Exited(0) => {
                self.exec_conditions_done = true;
                if self.record.state() == ServiceState::Starting {
                    self.spawn(generation, now)?;
                }
            }
            ExitKind::Exited(1..=254) => {
                self.start_deadline = None;
                self.record
                    .transition(Event::ConditionSkipped { generation })
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot skip {} after ExecCondition: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.cleanup_directories();
            }
            ExitKind::Exited(_) | ExitKind::Signaled(_) | ExitKind::CoreDumped(_) => {
                self.start_deadline = None;
                self.mark_spawn_failed(generation, now)?;
            }
        }
        Ok(())
    }

    fn handle_start_post_exit(
        &mut self,
        exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        self.start_post_helper = None;
        if !is_success(exit) && self.record.state() == ServiceState::Running {
            let actions = self
                .record
                .transition(Event::StartFailed {
                    generation: self.record.generation(),
                })
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot record failed start-post for {}: {error:?}",
                        self.spec().name
                    ))
                })?;
            self.apply_actions(actions)?;
            self.start_deadline = None;
            self.stop_deadline =
                if self.record.state() == ServiceState::Stopping && self.record.pid().is_some() {
                    timeout_deadline(now, self.spec().stop_timeout)
                } else {
                    None
                };
        }
        Ok(())
    }

    fn handle_stop_post_exit(
        &mut self,
        _exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        self.stop_post_helper = None;
        self.stop_deadline = None;
        self.cleanup_directories();
        self.maybe_start_after_teardown(now)
    }

    fn handle_active_stop_exit(
        &mut self,
        _exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        self.active_stop_helper = None;
        self.stop_deadline = None;
        self.spawn_stop_post(None);
        if self.stop_post_helper.is_none() {
            self.cleanup_directories();
        }
        self.maybe_start_after_teardown(now)
    }

    fn maybe_start_after_teardown(&mut self, now: Instant) -> Result<(), SupervisorError> {
        if self.restart_after_stop
            && self.wants_running
            && self.record.state() == ServiceState::Exited
            && self.active_stop_helper.is_none()
            && self.stop_post_helper.is_none()
        {
            self.restart_after_stop = false;
            self.start(now)?;
        }
        Ok(())
    }

    fn spawn_script(
        &self,
        commands: &[fractald_core::CommandSpec],
        main_pid: Option<u32>,
        label: &str,
    ) -> Result<HelperProcess, SupervisorError> {
        let spec = self.spec().clone();
        let mut environment = self.effective_environment()?;
        prepend_toolbox_path(&mut environment);
        let script = shell_script(commands, main_pid, &spec, &environment).map_err(|error| {
            SupervisorError::new(format!("cannot build {label} command: {error}"))
        })?;
        let user = spec
            .user
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid User", spec.name))
            })?;
        let group = spec
            .group
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid Group", spec.name))
            })?;
        let supplementary_groups = prepare_supplementary_groups(&spec)?;
        let system_call_filter = prepare_system_call_filter(&spec)?;
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", script.as_str()])
            .envs(&environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_unset_environment(&mut command, &spec);
        command.env_remove("FRACTALD_NOTIFY_SOCKET");
        if let Some(path) = self.notify_path.as_ref() {
            command.env("FRACTALD_NOTIFY_SOCKET", path);
        }
        if let Some(watchdog) = spec.watchdog {
            command.env(
                "FRACTALD_WATCHDOG_USEC",
                watchdog.as_micros().min(u64::MAX as u128).to_string(),
            );
        }
        if let Some(pid) = main_pid {
            command.env("MAINPID", pid.to_string());
        }
        if let Some(directory) = spec.working_directory.as_ref() {
            command.current_dir(directory);
        }
        let no_new_privileges = spec.no_new_privileges;
        let umask = spec.umask;
        let nice = spec.nice;
        let oom_score_adjust = spec.oom_score_adjust;
        let nofile = spec.nofile;
        let memlock = spec.memlock;
        let nproc = spec.nproc;
        let memory_deny_write_execute = spec.memory_deny_write_execute;
        let restrict_realtime = spec.restrict_realtime;
        let restrict_suid_sgid = spec.restrict_suid_sgid;
        let restrict_namespaces = spec.restrict_namespaces;
        let protect_control_groups = spec.protect_control_groups;
        let protect_kernel_modules = spec.protect_kernel_modules;
        let protect_kernel_tunables = spec.protect_kernel_tunables;
        let protect_kernel_logs = spec.protect_kernel_logs;
        let protect_clock = spec.protect_clock;
        let protect_hostname = spec.protect_hostname;
        let lock_personality = spec.lock_personality;
        let private_tmp = spec.private_tmp;
        let private_devices = spec.private_devices;
        let private_users = spec.private_users;
        let private_mounts = spec.private_mounts;
        let private_ipc = spec.private_ipc;
        let private_tmp_paths = self.private_tmp_paths.clone();
        let private_network = spec.private_network;
        let capability_bounding_set = spec.capability_bounding_set;
        let ambient_capabilities = spec.ambient_capabilities;
        let keep_capabilities = ambient_capabilities.is_some()
            && (user.is_some()
                || group.is_some()
                || supplementary_groups
                    .as_ref()
                    .map_or(false, |groups| !groups.is_empty()));
        let restrict_address_families = spec.restrict_address_families;
        let protect_system = spec.protect_system;
        let protect_home = spec.protect_home;
        let protect_proc = spec.protect_proc;
        let proc_subset = spec.proc_subset;
        let read_write_paths = spec.read_write_paths.clone();
        let read_only_paths = spec.read_only_paths.clone();
        let inaccessible_paths = spec.inaccessible_paths.clone();
        let managed_paths = self
            .directories
            .iter()
            .filter(|directory| directory.kind != DirectoryKind::Configuration)
            .map(|directory| directory.path.clone())
            .collect::<Vec<_>>();
        let ignore_sigpipe = spec.ignore_sigpipe;
        let parent_pid = std::process::id();
        unsafe {
            command.pre_exec(move || {
                fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
                fractald_platform::set_process_group()?;
                if keep_capabilities {
                    fractald_platform::set_keep_capabilities()?;
                }
                if private_users != PrivateUsersMode::No {
                    fractald_platform::enter_private_users(
                        private_users_mode_number(private_users),
                        user.as_deref(),
                        group.as_deref(),
                        supplementary_groups.as_deref(),
                    )?;
                }
                apply_process_settings(
                    umask,
                    nice,
                    oom_score_adjust,
                    nofile,
                    memlock,
                    nproc,
                    memory_deny_write_execute,
                    restrict_realtime,
                    protect_control_groups,
                    protect_kernel_modules,
                    protect_kernel_tunables,
                    protect_kernel_logs,
                    protect_clock,
                    protect_hostname,
                    lock_personality,
                    private_tmp,
                    private_devices,
                    private_mounts,
                    private_ipc,
                    private_tmp_paths.as_ref(),
                    private_network,
                    capability_bounding_set,
                    restrict_address_families,
                    protect_system,
                    protect_home,
                    protect_proc,
                    proc_subset,
                    &read_write_paths,
                    &read_only_paths,
                    &inaccessible_paths,
                    &managed_paths,
                    ignore_sigpipe,
                )?;
                fractald_platform::set_identity(
                    user.as_deref(),
                    group.as_deref(),
                    supplementary_groups.as_deref(),
                )?;
                if let Some(allowed) = capability_bounding_set {
                    fractald_platform::apply_capability_bounding_set(allowed)?;
                }
                if let Some(allowed) = ambient_capabilities {
                    fractald_platform::apply_ambient_capabilities(allowed)?;
                }
                if protect_clock {
                    fractald_platform::protect_clock()?;
                }
                if protect_hostname {
                    fractald_platform::install_hostname_filter()?;
                }
                if lock_personality {
                    fractald_platform::lock_personality()?;
                }
                if let Some(allowed) = restrict_address_families {
                    fractald_platform::restrict_address_families(allowed)?;
                }
                if no_new_privileges {
                    fractald_platform::set_no_new_privileges()?;
                }
                if restrict_suid_sgid {
                    fractald_platform::restrict_suid_sgid()?;
                }
                if let Some(allowed) = restrict_namespaces {
                    fractald_platform::restrict_namespaces(allowed)?;
                }
                install_prepared_system_call_filter(system_call_filter.as_ref())?;
                Ok(())
            });
        }
        let mut child = command
            .spawn()
            .map_err(|error| SupervisorError::new(format!("cannot execute {label}: {error}")))?;
        let pid = child.id();
        let pidfd = match PidFd::open(pid) {
            Ok(pidfd) => Some(pidfd),
            Err(error) if pidfd_unavailable(&error) => None,
            Err(error) => {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                return Err(SupervisorError::new(format!(
                    "cannot open {label} pidfd for {pid}: {error}"
                )));
            }
        };
        Ok(HelperProcess { child, pidfd, pid })
    }

    fn observe_exit(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        if let Some(pid) = self.adopted_pid {
            return Ok((!process_exists(pid)).then_some(ExitKind::Exited(1)));
        }
        if let Some(pidfd) = self.pidfd.as_ref() {
            return pidfd.try_wait().map_err(|error| {
                SupervisorError::new(format!("cannot observe {}: {error}", self.spec().name))
            });
        }
        let Some(child) = self.child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait().map_err(|error| {
            SupervisorError::new(format!("cannot poll {}: {error}", self.spec().name))
        })?;
        Ok(status.map(|status| match status.code() {
            Some(code) => ExitKind::Exited(code),
            None => match status.signal() {
                Some(signal) => ExitKind::Signaled(signal),
                None => ExitKind::Exited(1),
            },
        }))
    }

    fn observe_stop_exit(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        if let Some(pidfd) = self.stop_pidfd.as_ref() {
            return pidfd.try_wait().map_err(|error| {
                SupervisorError::new(format!(
                    "cannot observe stop command for {}: {error}",
                    self.spec().name
                ))
            });
        }
        let Some(child) = self.stop_child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait().map_err(|error| {
            SupervisorError::new(format!(
                "cannot poll stop command for {}: {error}",
                self.spec().name
            ))
        })?;
        Ok(status.map(|status| match status.code() {
            Some(code) => ExitKind::Exited(code),
            None => match status.signal() {
                Some(signal) => ExitKind::Signaled(signal),
                None => ExitKind::Exited(1),
            },
        }))
    }

    fn handle_stop_exit(&mut self, _exit: ExitKind) -> Result<(), SupervisorError> {
        self.stop_child = None;
        self.stop_pidfd = None;
        self.stop_pid = None;
        if let Some(pid) = self.record.pid() {
            let signal = self.stop_signal.unwrap_or(self.spec().kill_signal);
            self.send_signal(pid, signal)?;
        }
        Ok(())
    }

    fn handle_exit(&mut self, exit: ExitKind, now: Instant) -> Result<(), SupervisorError> {
        let pid = self.record.pid().ok_or_else(|| {
            SupervisorError::new(format!(
                "{} exited without a recorded PID",
                self.spec().name
            ))
        })?;
        let generation = self.record.generation();
        self.kill_stop_command();
        self.kill_reload_command();
        self.pidfd = None;
        self.child = None;
        self.process_group = None;
        self.forking_pending = false;
        self.forking_parent_exited = false;
        self.adopted_pid = None;
        self.notify_socket = None;
        self.kill_dbus_probe();
        self.dbus_ready = true;
        self.dbus_probe_index = 0;
        self.watchdog_deadline = None;
        self.runtime_deadline = None;
        if let Some(path) = self.notify_path.take() {
            let _ = fs::remove_file(path);
        }
        self.notify_ready = true;
        if let Some(path) = self.cgroup_path.take() {
            cleanup_cgroup(&path);
        }
        self.oom_events = None;
        self.start_deadline = None;
        self.stop_deadline = None;
        let mut reason = match exit {
            ExitKind::Exited(code) => ExitReason::Exited(code),
            ExitKind::Signaled(signal) => ExitReason::Signaled(signal),
            ExitKind::CoreDumped(signal) => ExitReason::CoreDumped(signal),
        };
        if self.spec().main_ignore_failure && !reason.is_success() {
            reason = ExitReason::Exited(0);
        }
        let actions = self
            .record
            .transition(Event::Exited {
                pid,
                generation,
                reason,
            })
            .map_err(|error| {
                SupervisorError::new(format!(
                    "cannot commit {} exit: {error:?}",
                    self.spec().name
                ))
            })?;
        self.apply_actions(actions)?;
        if self.record.state() != ServiceState::Active {
            self.spawn_stop_post(Some(pid));
            if self.stop_post_helper.is_none() {
                self.cleanup_directories();
            }
        }

        if self.record.state() == ServiceState::Backoff && self.wants_running {
            self.restart_at = now.checked_add(self.spec().restart_backoff).or(Some(now));
        } else if self.restart_after_stop && self.wants_running {
            self.restart_after_stop = false;
            if let Err(error) = self.enforce_start_limit(now) {
                eprintln!("fractald: {}", error);
            } else {
                let actions = self
                    .record
                    .transition(Event::StartRequested)
                    .map_err(|error| {
                        SupervisorError::new(format!(
                            "cannot restart {}: {error:?}",
                            self.spec().name
                        ))
                    })?;
                self.apply_start_actions(actions, now)?;
            }
        }
        Ok(())
    }

    fn apply_start_actions(
        &mut self,
        actions: Vec<Action>,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        for action in actions {
            match action {
                Action::Spawn { generation } => self.spawn(generation, now)?,
                other => self.apply_actions(vec![other])?,
            }
        }
        Ok(())
    }

    fn apply_actions(&mut self, actions: Vec<Action>) -> Result<(), SupervisorError> {
        for action in actions {
            match action {
                Action::SendSignal { pid, signal } => self.send_signal(pid, signal)?,
                Action::ScheduleRestart { .. } | Action::Spawn { .. } => {}
            }
        }
        Ok(())
    }

    fn spawn(&mut self, generation: u64, now: Instant) -> Result<(), SupervisorError> {
        let spec = self.spec().clone();
        if spec.service_type == ServiceType::Socket {
            return self.spawn_socket(generation, now);
        }
        if matches!(spec.service_type, ServiceType::Timer | ServiceType::Path) {
            return self.spawn_trigger(generation, now);
        }
        if !spec.exec_conditions.is_empty() && !self.exec_conditions_done {
            let helper = self
                .spawn_script(&spec.exec_conditions, None, "exec-condition")
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot execute ExecCondition for {}: {error}",
                        spec.name
                    ))
                });
            match helper {
                Ok(helper) => {
                    self.exec_condition_helper = Some(helper);
                    return Ok(());
                }
                Err(error) => {
                    self.mark_spawn_failed(generation, now)?;
                    return Err(error);
                }
            }
        }
        if !spec.start_pre.is_empty() {
            let helper = self
                .spawn_script(&spec.start_pre, None, "start-pre")
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot execute start-pre for {}: {error}",
                        spec.name
                    ))
                });
            match helper {
                Ok(helper) => {
                    self.start_helper = Some(helper);
                    return Ok(());
                }
                Err(error) => {
                    self.mark_spawn_failed(generation, now)?;
                    return Err(error);
                }
            }
        }
        self.spawn_main(generation, now)
    }

    fn spawn_trigger(&mut self, generation: u64, now: Instant) -> Result<(), SupervisorError> {
        if let Err(error) = self.record.transition(Event::Activated { generation }) {
            return Err(SupervisorError::new(format!(
                "cannot activate trigger {}: {error:?}",
                self.spec().name
            )));
        }
        self.start_deadline = None;
        self.trigger_started = Some(now);
        self.trigger_last_fire = None;
        self.timer_boot_fired = false;
        self.timer_persistent_pending = match self.spec().trigger.as_ref() {
            Some(TriggerSpec::Timer {
                persistent: true, ..
            }) => timer_has_missed_schedule(self.spec(), SystemTime::now()),
            _ => false,
        };
        self.timer_random_delay = match self.spec().trigger.as_ref() {
            Some(TriggerSpec::Timer {
                randomized_delay: Some(max),
                ..
            }) => timer_random_delay(self.spec(), *max),
            _ => Duration::ZERO,
        };
        self.timer_accuracy_delay = match self.spec().trigger.as_ref() {
            Some(TriggerSpec::Timer {
                accuracy: Some(max),
                ..
            }) => timer_random_delay(self.spec(), *max),
            _ => Duration::ZERO,
        };
        self.calendar_fires.clear();
        self.calendar_pending.clear();
        self.path_initial_check = true;
        self.path_observations = match self.spec().trigger.clone() {
            Some(TriggerSpec::Path { watches, .. }) => watches
                .iter()
                .map(|watch| observe_path_watch(watch, self.spec()))
                .collect(),
            _ => Vec::new(),
        };
        Ok(())
    }

    fn trigger_due(&mut self, now: Instant) -> Option<String> {
        if self.record.state() != ServiceState::Active {
            return None;
        }
        let trigger = self.spec().trigger.clone()?;
        match trigger {
            TriggerSpec::Timer {
                service,
                on_boot,
                on_unit_active,
                on_unit_inactive,
                on_calendar,
                ..
            } => {
                let started = self.trigger_started.unwrap_or(now);
                let schedule_delay = self
                    .timer_random_delay
                    .saturating_add(self.timer_accuracy_delay);
                if self.timer_persistent_pending && now.duration_since(started) >= schedule_delay {
                    self.timer_persistent_pending = false;
                    return Some(self.fire_timer(service, now));
                }
                if let Some(delay) = on_boot {
                    let due = delay.saturating_add(schedule_delay);
                    if !self.timer_boot_fired && now.duration_since(started) >= due {
                        self.timer_boot_fired = true;
                        return Some(self.fire_timer(service, now));
                    }
                }
                for delay in [on_unit_active, on_unit_inactive].into_iter().flatten() {
                    let reference = self.trigger_last_fire.unwrap_or(started);
                    let due = delay.saturating_add(schedule_delay);
                    if now.duration_since(reference) >= due {
                        return Some(self.fire_timer(service.clone(), now));
                    }
                }
                let wall_clock = SystemTime::now();
                for expression in on_calendar {
                    if let Some((bucket, due)) = self.calendar_pending.get(&expression).copied() {
                        if wall_clock >= due {
                            self.calendar_pending.remove(&expression);
                            self.calendar_fires.insert(expression, bucket);
                            return Some(self.fire_timer(service.clone(), now));
                        }
                        continue;
                    }
                    let Some(bucket) = calendar_match(&expression, wall_clock) else {
                        continue;
                    };
                    if self.calendar_fires.get(&expression).copied() == Some(bucket) {
                        continue;
                    }
                    if !self.timer_random_delay.is_zero() {
                        let due = wall_clock.checked_add(schedule_delay).unwrap_or(wall_clock);
                        self.calendar_pending.insert(expression, (bucket, due));
                        continue;
                    }
                    self.calendar_fires.insert(expression, bucket);
                    return Some(self.fire_timer(service.clone(), now));
                }
                None
            }
            TriggerSpec::Path { service, watches } => {
                let current = watches
                    .iter()
                    .map(|watch| observe_path_watch(watch, self.spec()))
                    .collect::<Vec<_>>();
                let initial = self.path_initial_check;
                let due = watches.iter().enumerate().any(|(index, watch)| {
                    let current = &current[index];
                    if initial {
                        return match watch {
                            PathWatch::Exists(_) | PathWatch::ExistsGlob(_) => current.exists,
                            PathWatch::DirectoryNotEmpty(_) => current.directory_nonempty,
                            PathWatch::Changed(_) | PathWatch::Modified(_) => false,
                        };
                    }
                    let previous = self.path_observations.get(index);
                    match (watch, previous) {
                        (PathWatch::Exists(_), Some(previous))
                        | (PathWatch::ExistsGlob(_), Some(previous)) => {
                            current.exists && !previous.exists
                        }
                        (PathWatch::DirectoryNotEmpty(_), Some(previous)) => {
                            current.directory_nonempty && !previous.directory_nonempty
                        }
                        (PathWatch::Changed(_), Some(previous))
                        | (PathWatch::Modified(_), Some(previous)) => current != previous,
                        (_, None) => true,
                    }
                });
                self.path_initial_check = false;
                self.path_observations = current;
                due.then_some(service)
            }
        }
    }

    fn reset_trigger_state(&mut self) {
        self.trigger_started = None;
        self.trigger_last_fire = None;
        self.timer_boot_fired = false;
        self.timer_persistent_pending = false;
        self.timer_random_delay = Duration::ZERO;
        self.timer_accuracy_delay = Duration::ZERO;
        self.calendar_fires.clear();
        self.calendar_pending.clear();
        self.path_observations.clear();
        self.path_initial_check = true;
    }

    fn fire_timer(&mut self, service: String, now: Instant) -> String {
        self.trigger_last_fire = Some(now);
        if matches!(
            self.spec().trigger,
            Some(TriggerSpec::Timer {
                persistent: true,
                ..
            })
        ) {
            if let Err(error) = record_timer_fire(self.spec()) {
                eprintln!(
                    "fractald: cannot persist timer state for {}: {error}",
                    self.spec().name
                );
            }
        }
        service
    }

    fn cleanup_directories(&mut self) {
        let directories = std::mem::take(&mut self.directories);
        for directory in directories.into_iter().rev() {
            if directory.cleanup {
                let _ = fs::remove_dir(&directory.path);
            }
        }
        if let Some(path) = self.credential_directory.take() {
            let _ = fs::remove_dir_all(path);
        }
        if let Some(paths) = self.private_tmp_paths.take() {
            paths.cleanup();
        }
    }

    fn spawn_socket(&mut self, generation: u64, now: Instant) -> Result<(), SupervisorError> {
        let listeners = match open_listeners(
            &self.spec().listeners,
            self.spec().socket_mode,
            self.spec().socket_user.as_deref(),
            self.spec().socket_group.as_deref(),
        ) {
            Ok(listeners) => listeners,
            Err(error) => {
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot open socket listeners for {}: {error}",
                    self.spec().name
                )));
            }
        };
        if let Err(error) = self.record.transition(Event::Activated { generation }) {
            for listener in &listeners {
                listener.remove_path();
            }
            return Err(SupervisorError::new(format!(
                "cannot activate socket {}: {error:?}",
                self.spec().name
            )));
        }
        self.listeners = listeners;
        self.start_deadline = None;
        Ok(())
    }

    fn set_activation_fds(&mut self, fds: Vec<RawFd>, names: Vec<String>) {
        if self.child.is_none() && self.record.state() != ServiceState::Running {
            self.clear_activation_fds();
            self.activation_fd_names = names;
            self.activation_fds = fds;
        }
    }

    fn set_activation_connection(&mut self, fd: RawFd) {
        if self.child.is_none() && self.record.state() != ServiceState::Running {
            self.clear_activation_fds();
            self.activation_fds = vec![fd];
            self.activation_owned_fds = vec![fd];
        } else {
            let _ = fractald_platform::close_fd(fd);
        }
    }

    fn clear_activation_fds(&mut self) {
        for fd in self.activation_owned_fds.drain(..) {
            let _ = fractald_platform::close_fd(fd);
        }
        self.activation_fd_names.clear();
        self.activation_fds.clear();
    }

    fn listener_fds(&self) -> Vec<RawFd> {
        self.listeners.iter().map(ManagedListener::raw_fd).collect()
    }

    fn close_listeners(&mut self, remove_on_stop: bool) {
        for listener in &mut self.listeners {
            if remove_on_stop {
                listener.remove_path();
            } else {
                listener.disarm_path_removal();
            }
        }
        self.listeners.clear();
    }

    fn effective_environment(&self) -> Result<BTreeMap<OsString, OsString>, SupervisorError> {
        let spec = self.spec();
        let mut environment = BTreeMap::new();
        for entry in &spec.environment_files {
            let path = PathBuf::from(expand_specifiers(entry.path.as_os_str(), spec));
            let source = match fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) if entry.optional && error.kind() == std::io::ErrorKind::NotFound => {
                    continue;
                }
                Err(error) => {
                    return Err(SupervisorError::new(format!(
                        "cannot read EnvironmentFile {} for {}: {error}",
                        path.to_string_lossy(),
                        spec.name
                    )));
                }
            };
            parse_environment_file(&source, &path, &mut environment)?;
        }
        for (name, kind) in [
            ("CONFIGURATION_DIRECTORY", DirectoryKind::Configuration),
            ("RUNTIME_DIRECTORY", DirectoryKind::Runtime),
            ("STATE_DIRECTORY", DirectoryKind::State),
            ("CACHE_DIRECTORY", DirectoryKind::Cache),
            ("LOGS_DIRECTORY", DirectoryKind::Logs),
        ] {
            let paths = self
                .directories
                .iter()
                .filter(|directory| directory.kind == kind)
                .map(|directory| directory.path.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            if !paths.is_empty() {
                environment.insert(OsString::from(name), OsString::from(paths.join(" ")));
            }
        }
        environment.extend(spec.environment.clone());
        for variable in &spec.unset_environment {
            environment.insert(variable.clone(), OsString::new());
        }
        if let Some(path) = self.credential_directory.as_ref() {
            environment.insert(
                OsString::from("CREDENTIALS_DIRECTORY"),
                path.as_os_str().to_owned(),
            );
        }
        Ok(environment)
    }

    fn spawn_main(&mut self, generation: u64, now: Instant) -> Result<(), SupervisorError> {
        let spec = self.spec().clone();
        let environment = match self.effective_environment() {
            Ok(environment) => environment,
            Err(error) => {
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let notify_access = effective_notify_access(&spec);
        let notify = if spec.service_type == ServiceType::Notify
            || spec.watchdog.is_some()
            || notify_access != NotifyAccess::None
        {
            match prepare_notify_socket(&spec, generation) {
                Ok(notify) => Some(notify),
                Err(error) => {
                    self.mark_spawn_failed(generation, now)?;
                    return Err(SupervisorError::new(format!(
                        "cannot prepare readiness socket for {}: {error}",
                        spec.name
                    )));
                }
            }
        } else {
            None
        };
        let cgroup_path = match prepare_cgroup(&spec) {
            Ok(path) => path,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot prepare cgroup for {}: {error}",
                    spec.name
                )));
            }
        };
        let activation_fd = self.activation_fds.first().copied();
        let stdout =
            match service_output(&spec, &spec.stdout, "stdout", &environment, activation_fd) {
                Ok(stdout) => stdout,
                Err(error) => {
                    if let Some((_, path)) = notify.as_ref() {
                        let _ = fs::remove_file(path);
                    }
                    if let Some(path) = cgroup_path.as_ref() {
                        cleanup_cgroup(path);
                    }
                    self.mark_spawn_failed(generation, now)?;
                    return Err(error);
                }
            };
        let stderr =
            match service_output(&spec, &spec.stderr, "stderr", &environment, activation_fd) {
                Ok(stderr) => stderr,
                Err(error) => {
                    if let Some((_, path)) = notify.as_ref() {
                        let _ = fs::remove_file(path);
                    }
                    if let Some(path) = cgroup_path.as_ref() {
                        cleanup_cgroup(path);
                    }
                    self.mark_spawn_failed(generation, now)?;
                    return Err(error);
                }
            };
        let stdin = match service_input(&spec, &spec.standard_input, &environment) {
            Ok(stdin) => stdin,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let program = expand_command_argument(
            spec.program.as_os_str(),
            &spec,
            &environment,
            None,
            spec.main_expand_environment,
        );
        let activation_fds = std::mem::take(&mut self.activation_fds);
        let activation_owned_fds = std::mem::take(&mut self.activation_owned_fds);
        let activation_fd_names = std::mem::take(&mut self.activation_fd_names);
        let activation_launcher = if activation_fds.is_empty() {
            None
        } else {
            activation_launcher_path()
        };
        let mut command = if let Some(launcher) = activation_launcher.as_ref() {
            let mut command = Command::new(launcher);
            command.arg("--").arg(&program);
            command
        } else {
            toolbox_command(
                Path::new(&program),
                spec.service_type,
                spec.mount_filesystem.as_deref(),
            )
        };
        command
            .args(spec.args.iter().map(|argument| {
                expand_command_argument(
                    argument,
                    &spec,
                    &environment,
                    None,
                    spec.main_expand_environment,
                )
            }))
            .envs(&environment)
            .stdin(stdin)
            .stdout(stdout)
            .stderr(stderr);
        apply_unset_environment(&mut command, &spec);
        command
            .env_remove("LISTEN_FDS")
            .env_remove("LISTEN_PID")
            .env_remove("LISTEN_PIDFDID")
            .env_remove("LISTEN_FDNAMES")
            .env_remove("FRACTALD_LAUNCH_ARG0");
        if !activation_fds.is_empty() {
            command.env("LISTEN_FDS", activation_fds.len().to_string());
        }
        if !activation_fd_names.is_empty() {
            command.env("LISTEN_FDNAMES", activation_fd_names.join(":"));
        }
        if let Some((_, path)) = notify.as_ref() {
            command.env("FRACTALD_NOTIFY_SOCKET", path);
        }
        if let Some(watchdog) = spec.watchdog {
            command.env(
                "FRACTALD_WATCHDOG_USEC",
                watchdog.as_micros().min(u64::MAX as u128).to_string(),
            );
        }
        if let Some(argv0) = spec.main_argv0.as_ref() {
            let argv0 = expand_command_argument(
                argv0,
                &spec,
                &environment,
                None,
                spec.main_expand_environment,
            );
            if activation_launcher.is_some() {
                command.env("FRACTALD_LAUNCH_ARG0", argv0);
            } else {
                command.arg0(argv0);
            }
        }
        if let Some(directory) = spec.working_directory.as_ref() {
            command.current_dir(expand_command_argument(
                directory.as_os_str(),
                &spec,
                &environment,
                None,
                true,
            ));
        }
        let user = spec
            .user
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid User", spec.name))
            });
        let user = match user {
            Ok(user) => user,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let group = spec
            .group
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid Group", spec.name))
            });
        let group = match group {
            Ok(group) => group,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let supplementary_groups = match prepare_supplementary_groups(&spec) {
            Ok(groups) => groups,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let system_call_filter = match prepare_system_call_filter(&spec) {
            Ok(filter) => filter,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(error);
            }
        };
        let no_new_privileges = spec.no_new_privileges;
        let umask = spec.umask;
        let nice = spec.nice;
        let oom_score_adjust = spec.oom_score_adjust;
        let nofile = spec.nofile;
        let memlock = spec.memlock;
        let nproc = spec.nproc;
        let memory_deny_write_execute = spec.memory_deny_write_execute;
        let restrict_realtime = spec.restrict_realtime;
        let protect_control_groups = spec.protect_control_groups;
        let protect_kernel_modules = spec.protect_kernel_modules;
        let protect_kernel_tunables = spec.protect_kernel_tunables;
        let protect_kernel_logs = spec.protect_kernel_logs;
        let protect_clock = spec.protect_clock;
        let protect_hostname = spec.protect_hostname;
        let lock_personality = spec.lock_personality;
        let private_tmp = spec.private_tmp;
        let private_devices = spec.private_devices;
        let private_users = spec.private_users;
        let private_mounts = spec.private_mounts;
        let private_ipc = spec.private_ipc;
        let private_tmp_paths = self.private_tmp_paths.clone();
        let private_network = spec.private_network;
        let capability_bounding_set = spec.capability_bounding_set;
        let ambient_capabilities = spec.ambient_capabilities;
        let keep_capabilities = ambient_capabilities.is_some()
            && (user.is_some()
                || group.is_some()
                || supplementary_groups
                    .as_ref()
                    .map_or(false, |groups| !groups.is_empty()));
        let restrict_address_families = spec.restrict_address_families;
        let protect_system = spec.protect_system;
        let protect_home = spec.protect_home;
        let protect_proc = spec.protect_proc;
        let proc_subset = spec.proc_subset;
        let read_write_paths = spec.read_write_paths.clone();
        let read_only_paths = spec.read_only_paths.clone();
        let inaccessible_paths = spec.inaccessible_paths.clone();
        let managed_paths = self
            .directories
            .iter()
            .filter(|directory| directory.kind != DirectoryKind::Configuration)
            .map(|directory| directory.path.clone())
            .collect::<Vec<_>>();
        let restrict_suid_sgid = spec.restrict_suid_sgid;
        let restrict_namespaces = spec.restrict_namespaces;
        let standard_input_socket = matches!(&spec.standard_input, InputMode::Socket);
        let ignore_sigpipe = spec.ignore_sigpipe;
        let parent_pid = std::process::id();
        unsafe {
            command.pre_exec(move || {
                fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
                fractald_platform::set_process_group()?;
                if keep_capabilities {
                    fractald_platform::set_keep_capabilities()?;
                }
                if private_users != PrivateUsersMode::No {
                    fractald_platform::enter_private_users(
                        private_users_mode_number(private_users),
                        user.as_deref(),
                        group.as_deref(),
                        supplementary_groups.as_deref(),
                    )?;
                }
                apply_process_settings(
                    umask,
                    nice,
                    oom_score_adjust,
                    nofile,
                    memlock,
                    nproc,
                    memory_deny_write_execute,
                    restrict_realtime,
                    protect_control_groups,
                    protect_kernel_modules,
                    protect_kernel_tunables,
                    protect_kernel_logs,
                    protect_clock,
                    protect_hostname,
                    lock_personality,
                    private_tmp,
                    private_devices,
                    private_mounts,
                    private_ipc,
                    private_tmp_paths.as_ref(),
                    private_network,
                    capability_bounding_set,
                    restrict_address_families,
                    protect_system,
                    protect_home,
                    protect_proc,
                    proc_subset,
                    &read_write_paths,
                    &read_only_paths,
                    &inaccessible_paths,
                    &managed_paths,
                    ignore_sigpipe,
                )?;
                fractald_platform::set_identity(
                    user.as_deref(),
                    group.as_deref(),
                    supplementary_groups.as_deref(),
                )?;
                if let Some(allowed) = capability_bounding_set {
                    fractald_platform::apply_capability_bounding_set(allowed)?;
                }
                if let Some(allowed) = ambient_capabilities {
                    fractald_platform::apply_ambient_capabilities(allowed)?;
                }
                if protect_clock {
                    fractald_platform::protect_clock()?;
                }
                if protect_hostname {
                    fractald_platform::install_hostname_filter()?;
                }
                if lock_personality {
                    fractald_platform::lock_personality()?;
                }
                if let Some(allowed) = restrict_address_families {
                    fractald_platform::restrict_address_families(allowed)?;
                }
                if no_new_privileges {
                    fractald_platform::set_no_new_privileges()?;
                }
                if standard_input_socket && activation_fds.is_empty() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "StandardInput=socket requires an activated socket",
                    ));
                }
                if !activation_fds.is_empty() {
                    fractald_platform::prepare_activation_fds(&activation_fds)?;
                    if standard_input_socket {
                        fractald_platform::set_activation_stdin()?;
                    }
                }
                if restrict_suid_sgid {
                    fractald_platform::restrict_suid_sgid()?;
                }
                if let Some(allowed) = restrict_namespaces {
                    fractald_platform::restrict_namespaces(allowed)?;
                }
                install_prepared_system_call_filter(system_call_filter.as_ref())?;
                Ok(())
            });
        }
        let spawn_result = command.spawn();
        close_activation_fds(&activation_owned_fds);
        let mut child = match spawn_result {
            Ok(child) => child,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot execute {}: {error}",
                    spec.program.display()
                )));
            }
        };
        let pid = child.id();
        let journal_stdout = matches!(spec.stdout, OutputMode::Journal);
        let journal_stderr = matches!(spec.stderr, OutputMode::Journal);
        if journal_stdout {
            let Some(stdout) = child.stdout.take() else {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "journal stdout pipe was not created for {}",
                    spec.name
                )));
            };
            if let Err(error) = spawn_journal_forwarder(stdout, &spec.name, "stdout", pid) {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot forward journal stdout for {}: {error}",
                    spec.name
                )));
            }
        }
        if journal_stderr {
            let Some(stderr) = child.stderr.take() else {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "journal stderr pipe was not created for {}",
                    spec.name
                )));
            };
            if let Err(error) = spawn_journal_forwarder(stderr, &spec.name, "stderr", pid) {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot forward journal stderr for {}: {error}",
                    spec.name
                )));
            }
        }
        if let Some(path) = cgroup_path.as_ref() {
            if let Err(error) = attach_to_cgroup(path, pid) {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = child.kill();
                let _ = child.wait();
                cleanup_cgroup(path);
                if let Some((_, notify_path)) = notify.as_ref() {
                    let _ = fs::remove_file(notify_path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot attach {pid} to cgroup {}: {error}",
                    path.display()
                )));
            }
        }
        let oom_events = cgroup_path
            .as_ref()
            .and_then(|path| read_cgroup_oom_events(path));
        let pidfd = match PidFd::open(pid) {
            Ok(pidfd) => Some(pidfd),
            Err(error) if pidfd_unavailable(&error) => None,
            Err(error) => {
                if let Some((_, path)) = notify.as_ref() {
                    let _ = fs::remove_file(path);
                }
                let _ = child.kill();
                let _ = child.wait();
                if let Some(path) = cgroup_path.as_ref() {
                    cleanup_cgroup(path);
                }
                self.mark_spawn_failed(generation, now)?;
                return Err(SupervisorError::new(format!(
                    "cannot open pidfd for {pid}: {error}"
                )));
            }
        };
        if let Err(error) = self.record.transition(Event::Spawned { pid, generation }) {
            if let Some(pidfd) = pidfd.as_ref() {
                let _ = pidfd.send_signal(SIGKILL);
            } else {
                let _ = child.kill();
            }
            let _ = child.wait();
            if let Some(path) = cgroup_path.as_ref() {
                cleanup_cgroup(path);
            }
            if let Some((_, path)) = notify.as_ref() {
                let _ = fs::remove_file(path);
            }
            return Err(SupervisorError::new(format!(
                "cannot register {pid}: {error:?}"
            )));
        }
        self.child = Some(child);
        self.pidfd = pidfd;
        self.process_group = Some(pid);
        self.forking_pending = spec.service_type == ServiceType::Forking;
        self.forking_parent_exited = false;
        self.notify_ready = spec.service_type != ServiceType::Notify;
        self.dbus_ready = spec.service_type != ServiceType::Dbus;
        self.dbus_probe_index = 0;
        self.dbus_probe_at = None;
        self.watchdog_deadline = None;
        if self.record.state() == ServiceState::Running
            && !matches!(spec.service_type, ServiceType::Forking | ServiceType::Dbus)
        {
            self.arm_runtime_limit(now);
        } else {
            self.runtime_deadline = None;
        }
        if let Some((socket, path)) = notify {
            self.notify_socket = Some(socket);
            self.notify_path = Some(path);
        }
        self.cgroup_path = cgroup_path;
        self.oom_events = oom_events;
        self.start_deadline = self
            .start_deadline
            .or_else(|| timeout_deadline(now, spec.start_timeout));
        if !spec.start_post.is_empty()
            && !matches!(spec.service_type, ServiceType::Forking | ServiceType::Dbus)
        {
            self.spawn_start_post(pid)?;
        }
        Ok(())
    }

    fn spawn_start_post(&mut self, pid: u32) -> Result<(), SupervisorError> {
        let commands = self.spec().start_post.clone();
        if commands.is_empty() || self.start_post_helper.is_some() {
            return Ok(());
        }
        match self.spawn_script(&commands, Some(pid), "start-post") {
            Ok(helper) => {
                self.start_post_helper = Some(helper);
                Ok(())
            }
            Err(error) => {
                let _ = self.send_signal(pid, self.spec().kill_signal);
                Err(SupervisorError::new(format!(
                    "cannot execute start-post for {}: {error}",
                    self.spec().name
                )))
            }
        }
    }

    fn handle_forking_parent_exit(
        &mut self,
        exit: ExitKind,
        now: Instant,
    ) -> Result<(), SupervisorError> {
        if !is_success(exit) {
            self.forking_pending = false;
            self.forking_parent_exited = false;
            return self.handle_exit(exit, now);
        }
        self.child = None;
        self.pidfd = None;
        self.forking_parent_exited = true;
        self.try_adopt_forking(now)
    }

    fn try_adopt_forking(&mut self, now: Instant) -> Result<(), SupervisorError> {
        let Some(parent_pid) = self.process_group else {
            self.forking_pending = false;
            self.forking_parent_exited = false;
            return Err(SupervisorError::new(format!(
                "forking service {} has no launcher process group",
                self.spec().name
            )));
        };
        let candidate = forking_process_candidate(self.spec(), parent_pid);
        let Some(pid) = candidate else {
            if self.start_deadline.is_some_and(|deadline| now >= deadline) {
                self.forking_pending = false;
                self.forking_parent_exited = false;
                return self.handle_exit(ExitKind::Exited(1), now);
            }
            return Ok(());
        };
        if pid == parent_pid {
            return Ok(());
        }
        let pidfd = match PidFd::open(pid) {
            Ok(pidfd) => Some(pidfd),
            Err(error) if pidfd_unavailable(&error) => None,
            Err(error) if error.raw_os_error() == Some(3) => return Ok(()),
            Err(error) => {
                return Err(SupervisorError::new(format!(
                    "cannot open forked process {pid} for {}: {error}",
                    self.spec().name
                )));
            }
        };
        self.record
            .transition(Event::Adopted {
                pid,
                generation: self.record.generation(),
            })
            .map_err(|error| {
                SupervisorError::new(format!(
                    "cannot adopt process {pid} for {}: {error:?}",
                    self.spec().name
                ))
            })?;
        if let Some(path) = self.cgroup_path.as_ref() {
            if let Err(error) = attach_to_cgroup(path, pid) {
                if let Some(pidfd) = pidfd.as_ref() {
                    let _ = pidfd.send_signal(SIGKILL);
                }
                return Err(SupervisorError::new(format!(
                    "cannot attach forked process {pid} to cgroup {}: {error}",
                    path.display()
                )));
            }
        }
        self.pidfd = pidfd;
        self.forking_pending = false;
        self.forking_parent_exited = false;
        self.adopted_pid = Some(pid);
        self.start_deadline = None;
        self.arm_runtime_limit(now);
        self.spawn_start_post(pid)?;
        Ok(())
    }

    fn mark_spawn_failed(&mut self, generation: u64, now: Instant) -> Result<(), SupervisorError> {
        self.kill_exec_condition_helper();
        self.clear_activation_fds();
        self.cleanup_directories();
        let actions = self
            .record
            .transition(Event::SpawnFailed { generation })
            .map_err(|error| {
                SupervisorError::new(format!("cannot record spawn failure: {error:?}"))
            })?;
        for action in actions {
            match action {
                Action::ScheduleRestart { after, .. } => {
                    self.restart_at = now.checked_add(after).or(Some(now));
                }
                Action::SendSignal { pid, signal } => self.send_signal(pid, signal)?,
                Action::Spawn { .. } => {}
            }
        }
        Ok(())
    }

    fn spawn_stop_command(&mut self, pid: u32) -> Result<(), SupervisorError> {
        let spec = self.spec().clone();
        let environment = self.effective_environment()?;
        let stop = spec
            .stop
            .clone()
            .ok_or_else(|| SupervisorError::new("service has no stop command"))?;
        let args = stop
            .args
            .iter()
            .map(|argument| {
                expand_command_argument(
                    argument,
                    &spec,
                    &environment,
                    Some(pid),
                    stop.expand_environment,
                )
            })
            .collect::<Vec<_>>();
        let program = expand_command_argument(
            stop.program.as_os_str(),
            &spec,
            &environment,
            Some(pid),
            stop.expand_environment,
        );
        let mut command = toolbox_command(
            Path::new(&program),
            spec.service_type,
            spec.mount_filesystem.as_deref(),
        );
        command
            .args(args)
            .envs(&environment)
            .env("MAINPID", pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_unset_environment(&mut command, &spec);
        if let Some(directory) = spec.working_directory.as_ref() {
            command.current_dir(expand_command_argument(
                directory.as_os_str(),
                &spec,
                &environment,
                Some(pid),
                true,
            ));
        }
        if let Some(argv0) = stop.argv0.as_ref() {
            command.arg0(expand_command_argument(
                argv0,
                &spec,
                &environment,
                Some(pid),
                stop.expand_environment,
            ));
        }
        let user = spec
            .user
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid User", spec.name))
            })?;
        let group = spec
            .group
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid Group", spec.name))
            })?;
        let supplementary_groups = prepare_supplementary_groups(&spec)?;
        let system_call_filter = prepare_system_call_filter(&spec)?;
        let no_new_privileges = spec.no_new_privileges;
        let umask = spec.umask;
        let nice = spec.nice;
        let oom_score_adjust = spec.oom_score_adjust;
        let nofile = spec.nofile;
        let memlock = spec.memlock;
        let nproc = spec.nproc;
        let memory_deny_write_execute = spec.memory_deny_write_execute;
        let restrict_realtime = spec.restrict_realtime;
        let restrict_suid_sgid = spec.restrict_suid_sgid;
        let restrict_namespaces = spec.restrict_namespaces;
        let protect_control_groups = spec.protect_control_groups;
        let protect_kernel_modules = spec.protect_kernel_modules;
        let protect_kernel_tunables = spec.protect_kernel_tunables;
        let protect_kernel_logs = spec.protect_kernel_logs;
        let protect_clock = spec.protect_clock;
        let protect_hostname = spec.protect_hostname;
        let lock_personality = spec.lock_personality;
        let private_tmp = spec.private_tmp;
        let private_devices = spec.private_devices;
        let private_users = spec.private_users;
        let private_mounts = spec.private_mounts;
        let private_ipc = spec.private_ipc;
        let private_tmp_paths = self.private_tmp_paths.clone();
        let private_network = spec.private_network;
        let capability_bounding_set = spec.capability_bounding_set;
        let ambient_capabilities = spec.ambient_capabilities;
        let keep_capabilities = ambient_capabilities.is_some()
            && (user.is_some()
                || group.is_some()
                || supplementary_groups
                    .as_ref()
                    .map_or(false, |groups| !groups.is_empty()));
        let restrict_address_families = spec.restrict_address_families;
        let protect_system = spec.protect_system;
        let protect_home = spec.protect_home;
        let protect_proc = spec.protect_proc;
        let proc_subset = spec.proc_subset;
        let read_write_paths = spec.read_write_paths.clone();
        let read_only_paths = spec.read_only_paths.clone();
        let inaccessible_paths = spec.inaccessible_paths.clone();
        let managed_paths = self
            .directories
            .iter()
            .filter(|directory| directory.kind != DirectoryKind::Configuration)
            .map(|directory| directory.path.clone())
            .collect::<Vec<_>>();
        let ignore_sigpipe = spec.ignore_sigpipe;
        let parent_pid = std::process::id();
        unsafe {
            command.pre_exec(move || {
                fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
                fractald_platform::set_process_group()?;
                if keep_capabilities {
                    fractald_platform::set_keep_capabilities()?;
                }
                if private_users != PrivateUsersMode::No {
                    fractald_platform::enter_private_users(
                        private_users_mode_number(private_users),
                        user.as_deref(),
                        group.as_deref(),
                        supplementary_groups.as_deref(),
                    )?;
                }
                apply_process_settings(
                    umask,
                    nice,
                    oom_score_adjust,
                    nofile,
                    memlock,
                    nproc,
                    memory_deny_write_execute,
                    restrict_realtime,
                    protect_control_groups,
                    protect_kernel_modules,
                    protect_kernel_tunables,
                    protect_kernel_logs,
                    protect_clock,
                    protect_hostname,
                    lock_personality,
                    private_tmp,
                    private_devices,
                    private_mounts,
                    private_ipc,
                    private_tmp_paths.as_ref(),
                    private_network,
                    capability_bounding_set,
                    restrict_address_families,
                    protect_system,
                    protect_home,
                    protect_proc,
                    proc_subset,
                    &read_write_paths,
                    &read_only_paths,
                    &inaccessible_paths,
                    &managed_paths,
                    ignore_sigpipe,
                )?;
                fractald_platform::set_identity(
                    user.as_deref(),
                    group.as_deref(),
                    supplementary_groups.as_deref(),
                )?;
                if let Some(allowed) = capability_bounding_set {
                    fractald_platform::apply_capability_bounding_set(allowed)?;
                }
                if let Some(allowed) = ambient_capabilities {
                    fractald_platform::apply_ambient_capabilities(allowed)?;
                }
                if protect_clock {
                    fractald_platform::protect_clock()?;
                }
                if protect_hostname {
                    fractald_platform::install_hostname_filter()?;
                }
                if lock_personality {
                    fractald_platform::lock_personality()?;
                }
                if let Some(allowed) = restrict_address_families {
                    fractald_platform::restrict_address_families(allowed)?;
                }
                if no_new_privileges {
                    fractald_platform::set_no_new_privileges()?;
                }
                if restrict_suid_sgid {
                    fractald_platform::restrict_suid_sgid()?;
                }
                if let Some(allowed) = restrict_namespaces {
                    fractald_platform::restrict_namespaces(allowed)?;
                }
                install_prepared_system_call_filter(system_call_filter.as_ref())?;
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|error| {
            SupervisorError::new(format!("cannot execute stop command: {error}"))
        })?;
        let stop_pid = child.id();
        let stop_pidfd = match PidFd::open(stop_pid) {
            Ok(pidfd) => Some(pidfd),
            Err(error) if pidfd_unavailable(&error) => None,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SupervisorError::new(format!(
                    "cannot open stop command pidfd for {stop_pid}: {error}"
                )));
            }
        };
        self.stop_child = Some(child);
        self.stop_pidfd = stop_pidfd;
        self.stop_pid = Some(stop_pid);
        Ok(())
    }

    fn spawn_reload_command(&mut self, pid: u32) -> Result<(), SupervisorError> {
        let spec = self.spec().clone();
        let environment = self.effective_environment()?;
        let reload = spec
            .reload
            .clone()
            .ok_or_else(|| SupervisorError::new("service has no reload command"))?;
        let args = reload
            .args
            .iter()
            .map(|argument| {
                expand_command_argument(
                    argument,
                    &spec,
                    &environment,
                    Some(pid),
                    reload.expand_environment,
                )
            })
            .collect::<Vec<_>>();
        let program = expand_command_argument(
            reload.program.as_os_str(),
            &spec,
            &environment,
            Some(pid),
            reload.expand_environment,
        );
        let mut command = toolbox_command(
            Path::new(&program),
            spec.service_type,
            spec.mount_filesystem.as_deref(),
        );
        command
            .args(args)
            .envs(&environment)
            .env("MAINPID", pid.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        apply_unset_environment(&mut command, &spec);
        if let Some(directory) = spec.working_directory.as_ref() {
            command.current_dir(expand_command_argument(
                directory.as_os_str(),
                &spec,
                &environment,
                Some(pid),
                true,
            ));
        }
        if let Some(argv0) = reload.argv0.as_ref() {
            command.arg0(expand_command_argument(
                argv0,
                &spec,
                &environment,
                Some(pid),
                reload.expand_environment,
            ));
        }
        let user = spec
            .user
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid User", spec.name))
            })?;
        let group = spec
            .group
            .as_deref()
            .map(CString::new)
            .transpose()
            .map_err(|_| {
                SupervisorError::new(format!("service {} has an invalid Group", spec.name))
            })?;
        let supplementary_groups = prepare_supplementary_groups(&spec)?;
        let system_call_filter = prepare_system_call_filter(&spec)?;
        let no_new_privileges = spec.no_new_privileges;
        let umask = spec.umask;
        let nice = spec.nice;
        let oom_score_adjust = spec.oom_score_adjust;
        let nofile = spec.nofile;
        let memlock = spec.memlock;
        let nproc = spec.nproc;
        let memory_deny_write_execute = spec.memory_deny_write_execute;
        let restrict_realtime = spec.restrict_realtime;
        let restrict_suid_sgid = spec.restrict_suid_sgid;
        let restrict_namespaces = spec.restrict_namespaces;
        let protect_control_groups = spec.protect_control_groups;
        let protect_kernel_modules = spec.protect_kernel_modules;
        let protect_kernel_tunables = spec.protect_kernel_tunables;
        let protect_kernel_logs = spec.protect_kernel_logs;
        let protect_clock = spec.protect_clock;
        let protect_hostname = spec.protect_hostname;
        let lock_personality = spec.lock_personality;
        let private_tmp = spec.private_tmp;
        let private_devices = spec.private_devices;
        let private_users = spec.private_users;
        let private_mounts = spec.private_mounts;
        let private_ipc = spec.private_ipc;
        let private_tmp_paths = self.private_tmp_paths.clone();
        let private_network = spec.private_network;
        let capability_bounding_set = spec.capability_bounding_set;
        let ambient_capabilities = spec.ambient_capabilities;
        let keep_capabilities = ambient_capabilities.is_some()
            && (user.is_some()
                || group.is_some()
                || supplementary_groups
                    .as_ref()
                    .map_or(false, |groups| !groups.is_empty()));
        let restrict_address_families = spec.restrict_address_families;
        let protect_system = spec.protect_system;
        let protect_home = spec.protect_home;
        let protect_proc = spec.protect_proc;
        let proc_subset = spec.proc_subset;
        let read_write_paths = spec.read_write_paths.clone();
        let read_only_paths = spec.read_only_paths.clone();
        let inaccessible_paths = spec.inaccessible_paths.clone();
        let managed_paths = self
            .directories
            .iter()
            .filter(|directory| directory.kind != DirectoryKind::Configuration)
            .map(|directory| directory.path.clone())
            .collect::<Vec<_>>();
        let ignore_sigpipe = spec.ignore_sigpipe;
        let parent_pid = std::process::id();
        unsafe {
            command.pre_exec(move || {
                fractald_platform::set_parent_death_signal(SIGKILL, parent_pid)?;
                fractald_platform::set_process_group()?;
                if keep_capabilities {
                    fractald_platform::set_keep_capabilities()?;
                }
                if private_users != PrivateUsersMode::No {
                    fractald_platform::enter_private_users(
                        private_users_mode_number(private_users),
                        user.as_deref(),
                        group.as_deref(),
                        supplementary_groups.as_deref(),
                    )?;
                }
                apply_process_settings(
                    umask,
                    nice,
                    oom_score_adjust,
                    nofile,
                    memlock,
                    nproc,
                    memory_deny_write_execute,
                    restrict_realtime,
                    protect_control_groups,
                    protect_kernel_modules,
                    protect_kernel_tunables,
                    protect_kernel_logs,
                    protect_clock,
                    protect_hostname,
                    lock_personality,
                    private_tmp,
                    private_devices,
                    private_mounts,
                    private_ipc,
                    private_tmp_paths.as_ref(),
                    private_network,
                    capability_bounding_set,
                    restrict_address_families,
                    protect_system,
                    protect_home,
                    protect_proc,
                    proc_subset,
                    &read_write_paths,
                    &read_only_paths,
                    &inaccessible_paths,
                    &managed_paths,
                    ignore_sigpipe,
                )?;
                fractald_platform::set_identity(
                    user.as_deref(),
                    group.as_deref(),
                    supplementary_groups.as_deref(),
                )?;
                if let Some(allowed) = capability_bounding_set {
                    fractald_platform::apply_capability_bounding_set(allowed)?;
                }
                if let Some(allowed) = ambient_capabilities {
                    fractald_platform::apply_ambient_capabilities(allowed)?;
                }
                if protect_clock {
                    fractald_platform::protect_clock()?;
                }
                if protect_hostname {
                    fractald_platform::install_hostname_filter()?;
                }
                if lock_personality {
                    fractald_platform::lock_personality()?;
                }
                if let Some(allowed) = restrict_address_families {
                    fractald_platform::restrict_address_families(allowed)?;
                }
                if no_new_privileges {
                    fractald_platform::set_no_new_privileges()?;
                }
                if restrict_suid_sgid {
                    fractald_platform::restrict_suid_sgid()?;
                }
                if let Some(allowed) = restrict_namespaces {
                    fractald_platform::restrict_namespaces(allowed)?;
                }
                install_prepared_system_call_filter(system_call_filter.as_ref())?;
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|error| {
            SupervisorError::new(format!("cannot execute reload command: {error}"))
        })?;
        let reload_pid = child.id();
        let reload_pidfd = match PidFd::open(reload_pid) {
            Ok(pidfd) => Some(pidfd),
            Err(error) if pidfd_unavailable(&error) => None,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(SupervisorError::new(format!(
                    "cannot open reload command pidfd for {reload_pid}: {error}"
                )));
            }
        };
        self.reload_child = Some(child);
        self.reload_pidfd = reload_pidfd;
        self.reload_pid = Some(reload_pid);
        Ok(())
    }

    fn observe_reload_exit(&mut self) -> Result<Option<ExitKind>, SupervisorError> {
        if let Some(pidfd) = self.reload_pidfd.as_ref() {
            return pidfd.try_wait().map_err(|error| {
                SupervisorError::new(format!(
                    "cannot observe reload command for {}: {error}",
                    self.spec().name
                ))
            });
        }
        let Some(child) = self.reload_child.as_mut() else {
            return Ok(None);
        };
        let status = child.try_wait().map_err(|error| {
            SupervisorError::new(format!(
                "cannot poll reload command for {}: {error}",
                self.spec().name
            ))
        })?;
        Ok(status.map(|status| match status.code() {
            Some(code) => ExitKind::Exited(code),
            None => match status.signal() {
                Some(signal) => ExitKind::Signaled(signal),
                None => ExitKind::Exited(1),
            },
        }))
    }

    fn handle_reload_exit(&mut self, exit: ExitKind) {
        self.reload_failed = !is_success(exit);
        self.reload_child = None;
        self.reload_pidfd = None;
        self.reload_pid = None;
    }

    fn spawn_stop_post(&mut self, pid: Option<u32>) {
        let commands = self.spec().stop_post.clone();
        if commands.is_empty() {
            return;
        }
        self.kill_stop_post_helper();
        if let Ok(helper) = self.spawn_script(&commands, pid, "stop-post") {
            self.stop_post_helper = Some(helper);
        }
    }

    fn spawn_active_stop(&mut self) -> Result<(), SupervisorError> {
        let command = self
            .spec()
            .stop
            .clone()
            .ok_or_else(|| SupervisorError::new("service has no stop command"))?;
        let helper = self.spawn_script(std::slice::from_ref(&command), None, "stop")?;
        self.active_stop_helper = Some(helper);
        Ok(())
    }

    fn kill_start_helper(&mut self) {
        if let Some(mut helper) = self.start_helper.take() {
            helper.kill();
        }
    }

    fn kill_dbus_probe(&mut self) {
        if let Some(mut probe) = self.dbus_probe.take() {
            let _ = probe.kill();
            let _ = probe.wait();
        }
        self.dbus_probe_at = None;
    }

    fn kill_exec_condition_helper(&mut self) {
        if let Some(mut helper) = self.exec_condition_helper.take() {
            helper.kill();
        }
    }

    fn kill_start_post_helper(&mut self) {
        if let Some(mut helper) = self.start_post_helper.take() {
            helper.kill();
        }
    }

    fn kill_active_stop_helper(&mut self) {
        if let Some(mut helper) = self.active_stop_helper.take() {
            helper.kill();
        }
    }

    fn kill_stop_post_helper(&mut self) {
        if let Some(mut helper) = self.stop_post_helper.take() {
            helper.kill();
        }
    }

    fn kill_stop_command(&mut self) {
        if let Some(pid) = self.stop_pid {
            let _ = fractald_platform::signal_process_group(pid, SIGKILL);
        }
        if let Some(pidfd) = self.stop_pidfd.as_ref() {
            let _ = pidfd.send_signal(SIGKILL);
        }
        if let Some(child) = self.stop_child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stop_pid = None;
        self.stop_pidfd = None;
        self.stop_child = None;
    }

    fn kill_reload_command(&mut self) {
        let was_pending =
            self.reload_pid.is_some() || self.reload_pidfd.is_some() || self.reload_child.is_some();
        if let Some(pid) = self.reload_pid {
            let _ = fractald_platform::signal_process_group(pid, SIGKILL);
        }
        if let Some(pidfd) = self.reload_pidfd.as_ref() {
            let _ = pidfd.send_signal(SIGKILL);
        }
        if let Some(child) = self.reload_child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.reload_pid = None;
        self.reload_pidfd = None;
        self.reload_child = None;
        if was_pending {
            self.reload_failed = true;
        }
    }

    fn reload_state(&self) -> &'static str {
        if self.reload_pid.is_some() || self.reload_pidfd.is_some() || self.reload_child.is_some() {
            "pending"
        } else if self.reload_failed {
            "failed"
        } else {
            "done"
        }
    }

    fn send_signal(&mut self, pid: u32, signal: i32) -> Result<(), SupervisorError> {
        let effective_signal = if self.record.state() == ServiceState::Stopping
            && self.stop_signal.is_some()
            && signal == self.spec().kill_signal
        {
            self.stop_signal.unwrap_or(signal)
        } else {
            signal
        };
        let initial_stop_signal = self.record.state() == ServiceState::Stopping
            && self.stop_signal == Some(effective_signal)
            && !self.stop_signal_sent;
        self.send_signal_raw(pid, effective_signal)?;
        if initial_stop_signal {
            self.stop_signal_sent = true;
            if self.spec().send_sighup && effective_signal != SIGHUP {
                self.send_signal_raw(pid, SIGHUP)?;
            }
        }
        Ok(())
    }

    fn send_signal_raw(&mut self, pid: u32, signal: i32) -> Result<(), SupervisorError> {
        if self.record.pid() != Some(pid) {
            return Err(SupervisorError::new(format!(
                "service {} has no process {pid}",
                self.spec().name
            )));
        }
        match self.spec().kill_mode {
            KillMode::None => Ok(()),
            KillMode::Process => self.send_process_signal(pid, signal),
            KillMode::Mixed if signal != SIGKILL => self.send_process_signal(pid, signal),
            KillMode::ControlGroup | KillMode::Mixed => self.send_group_signal(pid, signal),
        }
    }

    fn send_process_signal(&mut self, pid: u32, signal: i32) -> Result<(), SupervisorError> {
        match fractald_platform::signal_process(pid, signal) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(3) => Ok(()),
            Err(process_error) => {
                if let Some(pidfd) = self.pidfd.as_ref() {
                    match pidfd.send_signal(signal) {
                        Ok(()) => Ok(()),
                        Err(error) if error.raw_os_error() == Some(3) => Ok(()),
                        Err(pidfd_error) => Err(SupervisorError::new(format!(
                            "cannot signal {}: process={process_error}; pidfd={pidfd_error}",
                            self.spec().name
                        ))),
                    }
                } else {
                    Err(SupervisorError::new(format!(
                        "cannot send signal {signal} to {}: {process_error}",
                        self.spec().name
                    )))
                }
            }
        }
    }

    fn send_group_signal(&mut self, pid: u32, signal: i32) -> Result<(), SupervisorError> {
        let process_group = self.process_group.unwrap_or(pid);
        match fractald_platform::signal_process_group(process_group, signal) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(3) => Ok(()),
            Err(group_error) => {
                if let Some(pidfd) = self.pidfd.as_ref() {
                    match pidfd.send_signal(signal) {
                        Ok(()) => Ok(()),
                        Err(error) if error.raw_os_error() == Some(3) => Ok(()),
                        Err(pidfd_error) => Err(SupervisorError::new(format!(
                            "cannot signal {}: group={group_error}; process={pidfd_error}",
                            self.spec().name
                        ))),
                    }
                } else if signal == SIGKILL {
                    if let Some(child) = self.child.as_mut() {
                        child.kill().map_err(|error| {
                            SupervisorError::new(format!(
                                "cannot kill {}: {error}",
                                self.spec().name
                            ))
                        })
                    } else {
                        Ok(())
                    }
                } else {
                    Err(SupervisorError::new(format!(
                        "cannot send signal {signal} to {}: {group_error}",
                        self.spec().name
                    )))
                }
            }
        }
    }
}

#[derive(Debug)]
struct HelperProcess {
    child: Child,
    pidfd: Option<PidFd>,
    pid: u32,
}

impl HelperProcess {
    fn try_wait(&mut self) -> std::io::Result<Option<ExitKind>> {
        if let Some(pidfd) = self.pidfd.as_ref() {
            return pidfd.try_wait();
        }
        self.child.try_wait().map(|status| {
            status.map(|status| match status.code() {
                Some(code) => ExitKind::Exited(code),
                None => match status.signal() {
                    Some(signal) => ExitKind::Signaled(signal),
                    None => ExitKind::Exited(1),
                },
            })
        })
    }

    fn kill(&mut self) {
        let _ = fractald_platform::signal_process_group(self.pid, SIGKILL);
        if let Some(pidfd) = self.pidfd.as_ref() {
            let _ = pidfd.send_signal(SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn observe_helper(
    helper: &mut Option<HelperProcess>,
    label: &str,
    service: &str,
) -> Result<Option<ExitKind>, SupervisorError> {
    Ok(helper
        .as_mut()
        .map(HelperProcess::try_wait)
        .transpose()
        .map_err(|error| {
            SupervisorError::new(format!("cannot observe {label} for {service}: {error}"))
        })?
        .flatten())
}

fn is_success(exit: ExitKind) -> bool {
    matches!(exit, ExitKind::Exited(0))
}

fn service_was_up(state: ServiceState) -> bool {
    matches!(
        state,
        ServiceState::Starting | ServiceState::Running | ServiceState::Active
    )
}

fn service_is_down(state: ServiceState) -> bool {
    !service_was_up(state)
}

fn manager_action_priority(action: ManagerAction) -> u8 {
    match action {
        ManagerAction::None => 0,
        ManagerAction::Exit => 1,
        ManagerAction::Halt | ManagerAction::HaltForce => 2,
        ManagerAction::Reboot
        | ManagerAction::RebootForce
        | ManagerAction::Kexec
        | ManagerAction::KexecForce
        | ManagerAction::SoftReboot
        | ManagerAction::SoftRebootForce => 3,
        ManagerAction::Poweroff | ManagerAction::PoweroffForce => 4,
    }
}

fn timeout_deadline(now: Instant, timeout: Duration) -> Option<Instant> {
    (timeout != Duration::MAX)
        .then(|| now.checked_add(timeout))
        .flatten()
}

fn shell_script(
    commands: &[fractald_core::CommandSpec],
    main_pid: Option<u32>,
    spec: &ServiceSpec,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<String, &'static str> {
    if commands.is_empty() {
        return Err("command list is empty");
    }
    let mut script = String::from("set -e\n");
    for command in commands {
        let program = expand_command_argument(
            command.program.as_os_str(),
            spec,
            environment,
            main_pid,
            command.expand_environment,
        );
        script.push_str(&shell_quote(&program));
        for argument in &command.args {
            let argument = expand_command_argument(
                argument,
                spec,
                environment,
                main_pid,
                command.expand_environment,
            );
            script.push(' ');
            script.push_str(&shell_quote(&argument));
        }
        if command.ignore_failure {
            script.push_str(" || true");
        }
        script.push('\n');
    }
    Ok(script)
}

fn prepare_supplementary_groups(
    spec: &ServiceSpec,
) -> Result<Option<Vec<CString>>, SupervisorError> {
    spec.supplementary_groups
        .as_ref()
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    CString::new(group.as_str()).map_err(|_| {
                        SupervisorError::new(format!(
                            "service {} has an invalid SupplementaryGroups value",
                            spec.name
                        ))
                    })
                })
                .collect()
        })
        .transpose()
}

fn apply_unset_environment(command: &mut Command, spec: &ServiceSpec) {
    for variable in &spec.unset_environment {
        command.env_remove(variable);
    }
}

fn shell_quote(value: &std::ffi::OsStr) -> String {
    let value = value.to_string_lossy();
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn expand_environment(argument: &OsString, environment: &BTreeMap<OsString, OsString>) -> OsString {
    let input = argument.to_string_lossy();
    let characters: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '$' {
            output.push(characters[index]);
            index += 1;
            continue;
        }
        if characters.get(index + 1) == Some(&'$') {
            output.push('$');
            index += 2;
            continue;
        }
        let (name_start, name_end, braced) = if characters.get(index + 1) == Some(&'{') {
            let start = index + 2;
            let mut end = start;
            while end < characters.len() && characters[end] != '}' {
                end += 1;
            }
            if end == characters.len() {
                output.push('$');
                index += 1;
                continue;
            }
            (start, end, true)
        } else {
            let start = index + 1;
            if !characters
                .get(start)
                .is_some_and(|character| character.is_ascii_alphabetic() || *character == '_')
            {
                output.push('$');
                index += 1;
                continue;
            }
            let mut end = start + 1;
            while end < characters.len()
                && (characters[end].is_ascii_alphanumeric() || characters[end] == '_')
            {
                end += 1;
            }
            (start, end, false)
        };
        let name: String = characters[name_start..name_end].iter().collect();
        let name_os = OsString::from(&name);
        if let Some(value) = environment.get(&name_os) {
            output.push_str(&value.to_string_lossy());
        } else if let Some(value) = env::var_os(&name) {
            output.push_str(&value.to_string_lossy());
        }
        index = if braced { name_end + 1 } else { name_end };
    }
    OsString::from(output)
}

fn expand_command_argument(
    argument: &OsStr,
    spec: &ServiceSpec,
    environment: &BTreeMap<OsString, OsString>,
    main_pid: Option<u32>,
    expand_environment_variables: bool,
) -> OsString {
    let argument = argument.to_os_string();
    let argument = main_pid
        .map(|pid| expand_main_pid(&argument, pid))
        .unwrap_or(argument);
    let argument = expand_specifiers(&argument, spec);
    if expand_environment_variables {
        expand_environment(&argument, environment)
    } else {
        argument
    }
}

fn expand_specifiers(argument: &OsStr, spec: &ServiceSpec) -> OsString {
    let input = argument.to_string_lossy();
    let mut output = String::with_capacity(input.len());
    let mut characters = input.chars();
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
        if let Some(value) = service_specifier(specifier, spec) {
            output.push_str(&value);
        } else {
            output.push('%');
            output.push(specifier);
        }
    }
    OsString::from(output)
}

fn service_specifier(specifier: char, spec: &ServiceSpec) -> Option<String> {
    let name = &spec.name;
    let service_stem = name.strip_suffix(".svc").unwrap_or(name);
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
    let user = spec
        .user
        .clone()
        .or_else(|| env::var("USER").ok())
        .unwrap_or_else(|| fractald_platform::effective_uid().to_string());
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/root"));
    let value = match specifier {
        'n' => name.clone(),
        'N' => service_stem.to_owned(),
        'p' | 'P' => prefix.to_owned(),
        'i' | 'I' => instance.unwrap_or_default().to_owned(),
        'f' => instance.unwrap_or_default().to_owned(),
        't' => runtime.to_string_lossy().into_owned(),
        'S' => state.to_string_lossy().into_owned(),
        'd' => credential_directory_path(spec)
            .ok()?
            .to_string_lossy()
            .into_owned(),
        'u' => user,
        'U' => fractald_platform::effective_uid().to_string(),
        'h' => home.to_string_lossy().into_owned(),
        'v' => fs::read_to_string("/proc/sys/kernel/osrelease")
            .ok()
            .map(|value| value.trim().to_owned())?,
        'b' => fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .map(|value| value.trim().to_owned())?,
        'm' => fs::read_to_string("/etc/machine-id")
            .ok()
            .map(|value| value.trim().to_owned())?,
        'H' => fs::read_to_string("/etc/hostname")
            .ok()
            .map(|value| value.trim().to_owned())?,
        _ => return None,
    };
    Some(value)
}

fn parse_environment_file(
    source: &str,
    path: &std::path::Path,
    destination: &mut BTreeMap<OsString, OsString>,
) -> Result<(), SupervisorError> {
    for (index, line) in source.lines().enumerate() {
        let line_number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let (key, value) = line.split_once('=').ok_or_else(|| {
            SupervisorError::new(format!(
                "invalid EnvironmentFile {} line {}: missing '='",
                path.display(),
                line_number
            ))
        })?;
        if key.is_empty()
            || !key.chars().enumerate().all(|(position, character)| {
                (position == 0 && (character.is_ascii_alphabetic() || character == '_'))
                    || (position > 0 && (character.is_ascii_alphanumeric() || character == '_'))
            })
        {
            return Err(SupervisorError::new(format!(
                "invalid EnvironmentFile {} line {}: invalid variable name",
                path.display(),
                line_number
            )));
        }
        let value = parse_environment_value(value.trim()).map_err(|message| {
            SupervisorError::new(format!(
                "invalid EnvironmentFile {} line {}: {message}",
                path.display(),
                line_number
            ))
        })?;
        destination.insert(OsString::from(key), OsString::from(value));
    }
    Ok(())
}

fn parse_environment_value(value: &str) -> Result<String, &'static str> {
    if value.is_empty() {
        return Ok(String::new());
    }
    let mut characters = value.chars().peekable();
    let quote = matches!(characters.peek(), Some('\'' | '"')).then(|| characters.next().unwrap());
    let mut output = String::new();
    let mut escaped = false;
    while let Some(character) = characters.next() {
        if escaped {
            output.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }
        if quote == Some('"') && character == '\\' {
            escaped = true;
            continue;
        }
        if quote.is_none() && character == '\\' {
            escaped = true;
            continue;
        }
        if quote.is_some_and(|quote| character == quote) {
            if characters.any(|remaining| !remaining.is_whitespace()) {
                return Err("trailing characters after quoted value");
            }
            return Ok(output);
        }
        output.push(character);
    }
    if escaped {
        return Err("value ends with an escape");
    }
    if quote.is_some() {
        return Err("unterminated quoted value");
    }
    Ok(output.trim_end().to_owned())
}

fn pidfd_unavailable(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(22 | 38 | 95))
}

fn close_activation_fds(fds: &[RawFd]) {
    for &fd in fds {
        let _ = fractald_platform::close_fd(fd);
    }
}

fn activation_launcher_path() -> Option<PathBuf> {
    if let Some(configured) = env::var_os("FRACTALD_ACTIVATION_LAUNCHER") {
        let path = PathBuf::from(configured);
        if path.is_file() {
            return Some(path);
        }
    }
    let executable = env::current_exe().ok()?;
    let mut directory = executable.parent();
    for _ in 0..=1 {
        let Some(current) = directory else {
            break;
        };
        let candidate = current.join("fractald-launch");
        if candidate.is_file() {
            return Some(candidate);
        }
        directory = current.parent();
    }
    None
}

fn resource_limit_values(limit: LimitRange) -> (u64, u64) {
    let soft = match limit.soft {
        LimitValue::Max => u64::MAX,
        LimitValue::Value(value) => value,
    };
    let hard = match limit.hard {
        LimitValue::Max => u64::MAX,
        LimitValue::Value(value) => value,
    };
    (soft, hard)
}

struct PreparedSystemCallFilter {
    names: Vec<CString>,
    actions: Vec<i32>,
    default_allow: bool,
    default_errno: Option<i32>,
    architecture: i32,
}

fn prepare_system_call_filter(
    spec: &ServiceSpec,
) -> Result<Option<PreparedSystemCallFilter>, SupervisorError> {
    let Some(filter) = spec.system_call_filter.as_ref() else {
        return Ok(None);
    };
    let names = filter
        .rules
        .iter()
        .map(|rule| {
            CString::new(rule.name.as_str()).map_err(|_| {
                SupervisorError::new(format!(
                    "service {} has an invalid system call name",
                    spec.name
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let actions = filter
        .rules
        .iter()
        .map(|rule| match rule.action {
            fractald_core::SystemCallRuleAction::Allow => -1,
            fractald_core::SystemCallRuleAction::Deny => 0,
            fractald_core::SystemCallRuleAction::DenyWithErrno(errno) => errno,
        })
        .collect();
    let architecture = match spec.system_call_architectures {
        None | Some(SystemCallArchitectures::All) => 0,
        Some(SystemCallArchitectures::Native) => 1,
        Some(SystemCallArchitectures::X86_64) => 2,
    };
    Ok(Some(PreparedSystemCallFilter {
        names,
        actions,
        default_allow: filter.default_allow,
        default_errno: spec.system_call_error_number,
        architecture,
    }))
}

fn install_prepared_system_call_filter(
    filter: Option<&PreparedSystemCallFilter>,
) -> std::io::Result<()> {
    let Some(filter) = filter else {
        return Ok(());
    };
    fractald_platform::install_system_call_filter(
        &filter.names,
        &filter.actions,
        filter.default_allow,
        filter.default_errno,
        filter.architecture,
    )
}

fn private_users_mode_number(mode: PrivateUsersMode) -> i32 {
    match mode {
        PrivateUsersMode::No => 0,
        PrivateUsersMode::SelfMapping => 1,
        PrivateUsersMode::Identity => 2,
        PrivateUsersMode::Full => 3,
    }
}

fn apply_process_settings(
    umask: Option<u32>,
    nice: Option<i32>,
    oom_score_adjust: Option<i32>,
    nofile: Option<LimitRange>,
    memlock: Option<LimitRange>,
    nproc: Option<LimitRange>,
    memory_deny_write_execute: bool,
    restrict_realtime: bool,
    protect_control_groups: bool,
    protect_kernel_modules: bool,
    protect_kernel_tunables: bool,
    protect_kernel_logs: bool,
    _protect_clock: bool,
    protect_hostname: bool,
    _lock_personality: bool,
    private_tmp: PrivateTmpMode,
    private_devices: bool,
    private_mounts: bool,
    private_ipc: bool,
    private_tmp_paths: Option<&PrivateTmpPaths>,
    private_network: bool,
    capability_bounding_set: Option<u64>,
    _restrict_address_families: Option<u64>,
    protect_system: ProtectSystemMode,
    protect_home: ProtectHomeMode,
    protect_proc: ProtectProcMode,
    proc_subset: ProcSubsetMode,
    read_write_paths: &[PathBuf],
    read_only_paths: &[PathBuf],
    inaccessible_paths: &[PathBuf],
    managed_paths: &[PathBuf],
    ignore_sigpipe: bool,
) -> std::io::Result<()> {
    if memory_deny_write_execute {
        fractald_platform::set_memory_deny_write_execute()?;
    }
    if restrict_realtime {
        fractald_platform::restrict_realtime()?;
    }
    if protect_hostname {
        fractald_platform::enter_private_uts_namespace()?;
    }
    if private_devices {
        fractald_platform::enter_private_devices()?;
    }
    match private_tmp {
        PrivateTmpMode::No => {}
        PrivateTmpMode::Yes => {
            let paths = private_tmp_paths.ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "PrivateTmp=yes has no prepared directories",
                )
            })?;
            fractald_platform::enter_private_tmp(1, Some(&paths.tmp), Some(&paths.var_tmp))?;
        }
        PrivateTmpMode::Disconnected => {
            fractald_platform::enter_private_tmp(2, None, None)?;
        }
    }
    let system_mode = match protect_system {
        ProtectSystemMode::No => 0,
        ProtectSystemMode::Yes => 1,
        ProtectSystemMode::Full => 2,
        ProtectSystemMode::Strict => 3,
    };
    let home_mode = match protect_home {
        ProtectHomeMode::No => 0,
        ProtectHomeMode::Yes => 1,
        ProtectHomeMode::ReadOnly => 2,
        ProtectHomeMode::Tmpfs => 3,
    };
    let proc_mode = match protect_proc {
        ProtectProcMode::Default => 0,
        ProtectProcMode::NoAccess => 1,
        ProtectProcMode::Invisible => 2,
        ProtectProcMode::Ptraceable => 4,
    };
    let subset_mode = match proc_subset {
        ProcSubsetMode::All => 0,
        ProcSubsetMode::Pid => 1,
    };
    let filesystem_isolation = system_mode != 0
        || home_mode != 0
        || proc_mode != 0
        || subset_mode != 0
        || private_mounts
        || protect_control_groups
        || protect_kernel_modules
        || protect_kernel_tunables
        || protect_kernel_logs
        || protect_hostname
        || !read_only_paths.is_empty()
        || !inaccessible_paths.is_empty();
    if filesystem_isolation {
        fractald_platform::apply_filesystem_protection(
            system_mode,
            home_mode,
            proc_mode,
            subset_mode,
        )?;
        if protect_system == ProtectSystemMode::Strict {
            for path in managed_paths {
                fractald_platform::make_path_writable(path)?;
            }
        }
        for path in read_write_paths {
            fractald_platform::make_path_writable(path)?;
        }
        if private_tmp != PrivateTmpMode::No {
            fractald_platform::make_path_writable(Path::new("/tmp"))?;
            fractald_platform::make_path_writable(Path::new("/var/tmp"))?;
        }
        for path in read_only_paths {
            fractald_platform::make_path_read_only(path)?;
        }
        if protect_control_groups {
            fractald_platform::make_path_read_only(Path::new("/sys/fs/cgroup"))?;
        }
        if protect_kernel_modules {
            for path in [
                "/sys/module",
                "/lib/modules",
                "/usr/lib/modules",
                "/lib/kernel",
                "/usr/lib/kernel",
            ] {
                fractald_platform::make_path_read_only(Path::new(path))?;
            }
            fractald_platform::make_path_inaccessible(Path::new("/sys/module"))?;
        }
        if protect_kernel_tunables {
            for path in [
                "/proc/sys",
                "/sys",
                "/proc/sysrq-trigger",
                "/proc/latency_stats",
                "/proc/acpi",
                "/proc/timer_stats",
                "/proc/fs",
                "/proc/irq",
            ] {
                fractald_platform::make_path_read_only(Path::new(path))?;
            }
            for path in ["/proc/kallsyms", "/proc/kcore"] {
                fractald_platform::make_path_inaccessible(Path::new(path))?;
            }
        }
        for path in inaccessible_paths {
            fractald_platform::make_path_inaccessible(path)?;
        }
        if protect_kernel_logs {
            for path in [
                "/dev/kmsg",
                "/proc/kmsg",
                "/sys/kernel/debug",
                "/sys/kernel/tracing",
            ] {
                fractald_platform::make_path_inaccessible(Path::new(path))?;
            }
        }
        if protect_hostname {
            for path in ["/etc/hostname", "/etc/machine-info"] {
                fractald_platform::make_path_inaccessible(Path::new(path))?;
            }
        }
    }
    if private_ipc {
        fractald_platform::enter_private_ipc()?;
    }
    if private_network {
        fractald_platform::enter_private_network()?;
    }
    if let Some(allowed) = capability_bounding_set {
        fractald_platform::drop_capability_bounding_set(allowed)?;
    }
    if let Some(mask) = umask {
        fractald_platform::set_umask(mask)?;
    }
    if let Some(value) = nice {
        fractald_platform::set_nice(value)?;
    }
    if let Some(value) = oom_score_adjust {
        fractald_platform::set_oom_score_adjust(value)?;
    }
    if let Some(limit) = nofile {
        let (soft, hard) = resource_limit_values(limit);
        fractald_platform::set_nofile_limit(soft, hard)?;
    }
    if let Some(limit) = memlock {
        let (soft, hard) = resource_limit_values(limit);
        fractald_platform::set_memlock_limit(soft, hard)?;
    }
    if let Some(limit) = nproc {
        let (soft, hard) = resource_limit_values(limit);
        fractald_platform::set_nproc_limit(soft, hard)?;
    }
    fractald_platform::set_signal_disposition(SIGPIPE, ignore_sigpipe)?;
    Ok(())
}

fn service_output(
    spec: &ServiceSpec,
    mode: &OutputMode,
    stream: &str,
    environment: &BTreeMap<OsString, OsString>,
    activation_fd: Option<RawFd>,
) -> Result<Stdio, SupervisorError> {
    match mode {
        OutputMode::Null => Ok(Stdio::null()),
        OutputMode::Inherit => Ok(Stdio::inherit()),
        OutputMode::Tty => open_tty(spec, environment, stream),
        OutputMode::Socket => {
            let key = if stream == "stdout" {
                "StandardOutput"
            } else {
                "StandardError"
            };
            let fd = activation_fd.ok_or_else(|| {
                SupervisorError::new(format!(
                    "{key}=socket requires an activated connection for {}",
                    spec.name
                ))
            })?;
            let duplicate = fractald_platform::duplicate_fd(fd).map_err(|error| {
                SupervisorError::new(format!(
                    "cannot duplicate activated socket for {} {stream}: {error}",
                    spec.name
                ))
            })?;
            Ok(Stdio::from(unsafe { fs::File::from_raw_fd(duplicate) }))
        }
        OutputMode::Journal => {
            let path = journal_log_path(spec, stream);
            open_log_file(&path, true)
                .map(|_| Stdio::piped())
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot open FractalD {stream} log for {}: {error}",
                        spec.name
                    ))
                })
                .map(|_| Stdio::piped())
        }
        OutputMode::File { path, append } => {
            let path = PathBuf::from(expand_command_argument(
                path.as_os_str(),
                spec,
                environment,
                None,
                true,
            ));
            open_log_file(&path, *append)
                .map(Stdio::from)
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot open {stream} output for {} at {}: {error}",
                        spec.name,
                        path.display()
                    ))
                })
        }
    }
}

fn service_input(
    spec: &ServiceSpec,
    mode: &InputMode,
    environment: &BTreeMap<OsString, OsString>,
) -> Result<Stdio, SupervisorError> {
    match mode {
        InputMode::Null | InputMode::Socket => Ok(Stdio::null()),
        InputMode::Inherit => Ok(Stdio::inherit()),
        InputMode::Tty => open_tty(spec, environment, "stdin"),
        InputMode::File(path) => {
            let path = PathBuf::from(expand_command_argument(
                path.as_os_str(),
                spec,
                environment,
                None,
                true,
            ));
            OpenOptions::new()
                .read(true)
                .open(&path)
                .map(Stdio::from)
                .map_err(|error| {
                    SupervisorError::new(format!(
                        "cannot open stdin for {} at {}: {error}",
                        spec.name,
                        path.display()
                    ))
                })
        }
    }
}

fn open_tty(
    spec: &ServiceSpec,
    environment: &BTreeMap<OsString, OsString>,
    stream: &str,
) -> Result<Stdio, SupervisorError> {
    let path = spec
        .tty_path
        .as_ref()
        .map(|path| {
            PathBuf::from(expand_command_argument(
                path.as_os_str(),
                spec,
                environment,
                None,
                true,
            ))
        })
        .unwrap_or_else(|| PathBuf::from("/dev/tty"));
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map(Stdio::from)
        .map_err(|error| {
            SupervisorError::new(format!(
                "cannot open TTY for {} {stream} at {}: {error}",
                spec.name,
                path.display()
            ))
        })
}

fn manager_log_directory() -> PathBuf {
    let directory = if let Some(path) = env::var_os("FRACTALD_LOG_DIR") {
        PathBuf::from(path)
    } else if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
        PathBuf::from(path).join("logs")
    } else if fractald_platform::is_root() {
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
    };
    directory
}

fn journal_log_path(spec: &ServiceSpec, stream: &str) -> PathBuf {
    journal_log_path_for_unit(&spec.name, stream)
}

fn journal_log_path_for_unit(unit: &str, stream: &str) -> PathBuf {
    manager_log_directory().join(format!("{unit}.{stream}.log"))
}

fn spawn_journal_forwarder<R>(reader: R, unit: &str, stream: &str, pid: u32) -> std::io::Result<()>
where
    R: Read + Send + 'static,
{
    let unit = unit.to_owned();
    let stream = stream.to_owned();
    let log_path = journal_log_path_for_unit(&unit, &stream);
    let name = format!("fractald-journal-{stream}");
    std::thread::Builder::new().name(name).spawn(move || {
        let mut log = match open_log_file(&log_path, true) {
            Ok(file) => file,
            Err(error) => {
                eprintln!("fractald: cannot open journal fallback for {unit} {stream}: {error}");
                return;
            }
        };
        let mut native = NativeJournal::connect();
        let mut input = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            match input.read_until(b'\n', &mut line) {
                Ok(0) => break,
                Ok(_) => {
                    if let Err(error) = log.write_all(&line) {
                        eprintln!(
                            "fractald: cannot write journal fallback for {unit} {stream}: {error}"
                        );
                        break;
                    }
                    let mut message = line.as_slice();
                    if message.last() == Some(&b'\n') {
                        message = &message[..message.len() - 1];
                    }
                    if message.last() == Some(&b'\r') {
                        message = &message[..message.len() - 1];
                    }
                    if let Some(socket) = native.as_ref() {
                        if socket.send(&unit, &stream, pid, message).is_err() {
                            native = None;
                        }
                    }
                }
                Err(error) => {
                    eprintln!("fractald: cannot read journal output for {unit} {stream}: {error}");
                    break;
                }
            }
        }
        let _ = log.flush();
    })?;
    Ok(())
}

fn open_log_file(path: &Path, append: bool) -> std::io::Result<fs::File> {
    if let Some(parent) = path.parent() {
        let was_present = parent.exists();
        create_dir_all_following_symlinks(parent)?;
        if !was_present {
            let mut permissions = fs::metadata(parent)?.permissions();
            permissions.set_mode(0o700);
            fs::set_permissions(parent, permissions)?;
        }
    }
    if append {
        let max_bytes = env::var("FRACTALD_LOG_MAX_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(16 * 1024 * 1024)
            .clamp(4 * 1024, 1 << 40);
        if fs::metadata(path).is_ok_and(|metadata| metadata.len() >= max_bytes) {
            let backup = PathBuf::from(format!("{}.1", path.display()));
            match fs::remove_file(&backup) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::rename(path, backup)?;
        }
    }
    let mut options = OpenOptions::new();
    options.create(true).write(true).mode(0o600);
    if append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    options.open(path)
}

fn create_dir_all_following_symlinks(path: &Path) -> std::io::Result<()> {
    fn create(path: &Path, symlink_depth: u8) -> std::io::Result<()> {
        if symlink_depth >= 40 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!("too many directory symlinks while creating {}", path.display()),
            ));
        }

        let mut current = if path.is_absolute() {
            PathBuf::from("/")
        } else {
            PathBuf::new()
        };
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            if component == std::path::Component::RootDir
                || component == std::path::Component::CurDir
            {
                continue;
            }
            current.push(component.as_os_str());
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_dir() => {}
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    let target = fs::read_link(&current)?;
                    let base = current.parent().unwrap_or_else(|| Path::new(""));
                    let target = if target.is_absolute() {
                        target
                    } else {
                        base.join(target)
                    };
                    let mut remainder = PathBuf::new();
                    for remaining in components {
                        remainder.push(remaining.as_os_str());
                    }
                    return create(&target.join(remainder), symlink_depth + 1);
                }
                Ok(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        format!("{} exists and is not a directory", current.display()),
                    ));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    match fs::create_dir(&current) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                            return create(path, symlink_depth + 1);
                        }
                        Err(error) => return Err(error),
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    create(path, 0)
}

fn credential_root() -> std::io::Result<PathBuf> {
    let configured = env::var_os("FRACTALD_CREDENTIALS_ROOT");
    let root = configured.map(PathBuf::from).unwrap_or_else(|| {
        if fractald_platform::is_root() {
            PathBuf::from("/run/credentials")
        } else {
            env::var_os("XDG_RUNTIME_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "/tmp/fractald-runtime-{}",
                        fractald_platform::effective_uid()
                    ))
                })
                .join("fractald/credentials")
        }
    });
    if root.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "credential root is empty",
        ));
    }
    Ok(root)
}

fn credential_directory_path(spec: &ServiceSpec) -> std::io::Result<PathBuf> {
    Ok(credential_root()?.join(&spec.name))
}

fn prepare_credentials(spec: &ServiceSpec) -> std::io::Result<Option<PathBuf>> {
    let import_dirs = credential_import_dirs();
    prepare_credentials_with_import_dirs(spec, &import_dirs)
}

fn prepare_credentials_with_import_dirs(
    spec: &ServiceSpec,
    import_dirs: &[PathBuf],
) -> std::io::Result<Option<PathBuf>> {
    let credential_values = materialize_credential_values(spec, import_dirs)?;
    if credential_values.is_empty() {
        return Ok(None);
    }
    let user = spec
        .user
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "User contains NUL"))?;
    let group = spec
        .group
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Group contains NUL"))?;
    let root = credential_root()?;
    match fs::symlink_metadata(&root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("credential root {} is not a directory", root.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(&root)?;
        }
        Err(error) => return Err(error),
    }
    let directory = root.join(&spec.name);
    match fs::symlink_metadata(&directory) {
        Ok(metadata) if metadata.file_type().is_dir() => {
            let marker = directory.join(CREDENTIAL_OWNER_MARKER);
            let owned = matches!(
                fs::symlink_metadata(&marker),
                Ok(marker_metadata)
                    if marker_metadata.file_type().is_file()
                        && fs::read(&marker).ok().as_deref() == Some(b"fractald\n")
            );
            if !owned {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "credential directory {} is not managed by FractalD",
                        directory.display()
                    ),
                ));
            }
            fs::remove_dir_all(&directory)?;
            fs::create_dir(&directory)?;
        }
        Ok(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("credential path {} is not a directory", directory.display()),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&directory)?;
        }
        Err(error) => return Err(error),
    }
    let result = (|| {
        let mut directory_permissions = fs::metadata(&directory)?.permissions();
        directory_permissions.set_mode(0o711);
        fs::set_permissions(&directory, directory_permissions)?;

        let marker_path = directory.join(CREDENTIAL_OWNER_MARKER);
        let mut marker = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&marker_path)?;
        marker.write_all(b"fractald\n")?;
        marker.sync_all()?;

        for (name, value) in credential_values {
            let destination = directory.join(&name);
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o400)
                .open(&destination)?;
            file.write_all(&value)?;
            file.sync_all()?;
            let mut permissions = file.metadata()?.permissions();
            permissions.set_mode(0o400);
            fs::set_permissions(&destination, permissions)?;
            if user.is_some() || group.is_some() {
                let path_c = CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "credential path contains NUL",
                    )
                })?;
                fractald_platform::chown_path(&path_c, user.as_deref(), group.as_deref())?;
            }
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&directory);
        return Err(error);
    }
    Ok(Some(directory))
}

fn credential_import_dirs() -> Vec<PathBuf> {
    if let Some(configured) = env::var_os("FRACTALD_CREDENTIAL_STORE_PATH") {
        return env::split_paths(&configured)
            .filter(|path| !path.as_os_str().is_empty())
            .collect();
    }
    [
        "/run/credentials/@system",
        "/run/fractald/credentials",
        "/etc/credstore",
        "/run/credstore",
        "/usr/lib/credstore",
    ]
    .into_iter()
    .map(PathBuf::from)
    .collect()
}

fn expand_credential_import(
    import: &CredentialImportSpec,
    spec: &ServiceSpec,
) -> CredentialImportSpec {
    CredentialImportSpec {
        pattern: expand_specifiers(OsStr::new(&import.pattern), spec)
            .to_string_lossy()
            .into_owned(),
        rename: import.rename.as_ref().map(|rename| {
            expand_specifiers(OsStr::new(rename), spec)
                .to_string_lossy()
                .into_owned()
        }),
    }
}

fn materialize_credential_values(
    spec: &ServiceSpec,
    import_dirs: &[PathBuf],
) -> std::io::Result<BTreeMap<String, Vec<u8>>> {
    let mut values = BTreeMap::new();
    let mut total_bytes = 0_usize;

    for credential in &spec.credentials {
        let source = match &credential.source {
            CredentialSource::File(source) => {
                Some(PathBuf::from(expand_specifiers(source.as_os_str(), spec)))
            }
            CredentialSource::Store(store_name) => {
                let store_name = expand_specifiers(OsStr::new(store_name), spec);
                let store_name = store_name.to_string_lossy();
                if !safe_credential_name(&store_name) {
                    eprintln!(
                        "fractald: skipping credential {} with invalid store name {}",
                        credential.name, store_name
                    );
                    None
                } else {
                    credential_store_entry(&store_name, import_dirs)
                }
            }
            CredentialSource::Value(_) => None,
        };
        let Some(source) = source else {
            continue;
        };
        let value = fs::read(&source).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "cannot load credential {} from {}: {error}",
                    credential.name,
                    source.display()
                ),
            )
        })?;
        add_credential_value(
            &mut values,
            &mut total_bytes,
            credential.name.clone(),
            value,
            true,
        )?;
    }

    for import in &spec.credential_imports {
        let import = expand_credential_import(import, spec);
        for (source_name, source) in credential_import_candidates(&import, import_dirs) {
            let destination = imported_credential_name(&import, &source_name);
            if !safe_credential_name(&destination) {
                eprintln!(
                    "fractald: skipping imported credential {} with invalid destination name {}",
                    source_name, destination
                );
                continue;
            }
            if values.contains_key(&destination) {
                continue;
            }
            let value = match fs::read(&source) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!(
                        "fractald: cannot import credential {} from {}: {error}",
                        source_name,
                        source.display()
                    );
                    continue;
                }
            };
            let _ = add_credential_value(&mut values, &mut total_bytes, destination, value, false)?;
        }
    }

    for credential in spec
        .credentials
        .iter()
        .filter(|credential| matches!(&credential.source, CredentialSource::Value(_)))
    {
        let CredentialSource::Value(value) = &credential.source else {
            unreachable!();
        };
        let _ = add_credential_value(
            &mut values,
            &mut total_bytes,
            credential.name.clone(),
            value.clone(),
            true,
        )?;
    }

    Ok(values)
}

fn add_credential_value(
    values: &mut BTreeMap<String, Vec<u8>>,
    total_bytes: &mut usize,
    name: String,
    value: Vec<u8>,
    fatal_on_limit: bool,
) -> std::io::Result<bool> {
    if values.contains_key(&name) {
        return Ok(false);
    }
    let exceeds_limit = value.len() > MAX_CREDENTIAL_BYTES
        || total_bytes
            .checked_add(value.len())
            .is_none_or(|total| total > MAX_CREDENTIAL_BYTES);
    if exceeds_limit {
        let error = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "credential data for {name} exceeds the {} byte per-unit limit",
                MAX_CREDENTIAL_BYTES
            ),
        );
        if fatal_on_limit {
            return Err(error);
        }
        eprintln!("fractald: skipping imported credential {name}: {error}");
        return Ok(false);
    }
    *total_bytes += value.len();
    values.insert(name, value);
    Ok(true)
}

fn credential_import_candidates(
    import: &CredentialImportSpec,
    import_dirs: &[PathBuf],
) -> BTreeMap<String, PathBuf> {
    let mut candidates = BTreeMap::new();
    if let Some(prefix) = import.pattern.strip_suffix('*') {
        for directory in import_dirs {
            let entries = match fs::read_dir(directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    eprintln!(
                        "fractald: cannot inspect credential store {}: {error}",
                        directory.display()
                    );
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    continue;
                };
                let file_name = entry.file_name();
                let Some(name) = file_name.to_str() else {
                    continue;
                };
                if !name.starts_with(prefix) || !safe_credential_name(name) {
                    continue;
                }
                let path = entry.path();
                if is_credential_store_file(&path) {
                    candidates.entry(name.to_owned()).or_insert(path);
                }
            }
        }
    } else {
        for directory in import_dirs {
            let path = directory.join(&import.pattern);
            if is_credential_store_file(&path) {
                candidates.insert(import.pattern.clone(), path);
                break;
            }
        }
    }
    candidates
}

fn credential_store_entry(name: &str, import_dirs: &[PathBuf]) -> Option<PathBuf> {
    import_dirs.iter().find_map(|directory| {
        let path = directory.join(name);
        is_credential_store_file(&path).then_some(path)
    })
}

fn imported_credential_name(import: &CredentialImportSpec, source_name: &str) -> String {
    if let Some(prefix) = import.pattern.strip_suffix('*') {
        let rename = import.rename.as_deref().unwrap_or(prefix);
        format!(
            "{rename}{}",
            source_name.strip_prefix(prefix).unwrap_or(source_name)
        )
    } else {
        import.rename.as_deref().unwrap_or(source_name).to_owned()
    }
}

fn is_credential_store_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn safe_credential_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value != CREDENTIAL_OWNER_MARKER
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn prepare_directories(spec: &ServiceSpec) -> std::io::Result<Vec<ManagedDirectory>> {
    let user = spec
        .user
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "User contains NUL"))?;
    let group = spec
        .group
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Group contains NUL"))?;
    let mut managed = Vec::new();
    for directory in &spec.directories {
        let root = directory_root(directory.kind)?;
        let relative = expand_specifiers(directory.path.as_os_str(), spec);
        let relative = PathBuf::from(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            cleanup_prepared_directories(&managed);
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "directory path {} is not a safe relative path",
                    directory.path.display()
                ),
            ));
        }
        let path = root.join(relative);
        let created = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => false,
            Ok(_) => {
                cleanup_prepared_directories(&managed);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("directory path {} is not a directory", path.display()),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir_all(&path) {
                    cleanup_prepared_directories(&managed);
                    return Err(error);
                }
                true
            }
            Err(error) => {
                cleanup_prepared_directories(&managed);
                return Err(error);
            }
        };
        let result = (|| {
            let mut permissions = fs::metadata(&path)?.permissions();
            permissions.set_mode(directory.mode);
            fs::set_permissions(&path, permissions)?;
            if user.is_some() || group.is_some() {
                let path_c = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "directory path contains NUL",
                    )
                })?;
                fractald_platform::chown_path(&path_c, user.as_deref(), group.as_deref())?;
            }
            Ok::<(), std::io::Error>(())
        })();
        if let Err(error) = result {
            if created {
                let _ = fs::remove_dir(&path);
            }
            cleanup_prepared_directories(&managed);
            return Err(error);
        }
        managed.push(ManagedDirectory {
            path,
            kind: directory.kind,
            cleanup: directory.kind == DirectoryKind::Runtime && !directory.preserve && created,
        });
    }
    Ok(managed)
}

fn cleanup_prepared_directories(directories: &[ManagedDirectory]) {
    for directory in directories.iter().rev() {
        if directory.cleanup {
            let _ = fs::remove_dir(&directory.path);
        }
    }
}

fn prepare_private_tmp(spec: &ServiceSpec) -> std::io::Result<Option<PrivateTmpPaths>> {
    if spec.private_tmp != PrivateTmpMode::Yes {
        return Ok(None);
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.hash(&mut hasher);
    let name_hash = hasher.finish();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    for attempt in 0..32_u32 {
        let suffix = format!(
            "fractald-private-{}-{:x}-{}-{}",
            std::process::id(),
            name_hash,
            timestamp,
            attempt
        );
        let tmp = PathBuf::from("/tmp").join(format!("{suffix}-tmp"));
        let var_tmp = PathBuf::from("/var/tmp").join(format!("{suffix}-var-tmp"));
        match fs::create_dir(&tmp) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
        if let Err(error) = fs::create_dir(&var_tmp) {
            let _ = fs::remove_dir(&tmp);
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                continue;
            }
            return Err(error);
        }
        let result = (|| {
            for path in [&tmp, &var_tmp] {
                let mut permissions = fs::metadata(path)?.permissions();
                permissions.set_mode(0o1777);
                fs::set_permissions(path, permissions)?;
            }
            Ok::<(), std::io::Error>(())
        })();
        if let Err(error) = result {
            let _ = fs::remove_dir(&tmp);
            let _ = fs::remove_dir(&var_tmp);
            return Err(error);
        }
        return Ok(Some(PrivateTmpPaths { tmp, var_tmp }));
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate unique private temporary directories",
    ))
}

fn directory_root(kind: DirectoryKind) -> std::io::Result<PathBuf> {
    let configured = match kind {
        DirectoryKind::Configuration => env::var_os("FRACTALD_CONFIGURATION_DIRECTORY_ROOT"),
        DirectoryKind::Runtime => env::var_os("FRACTALD_RUNTIME_DIRECTORY_ROOT")
            .or_else(|| env::var_os("FRACTALD_RUNTIME_DIR")),
        DirectoryKind::State => env::var_os("FRACTALD_STATE_DIRECTORY_ROOT")
            .or_else(|| env::var_os("FRACTALD_STATE_DIR")),
        DirectoryKind::Cache => env::var_os("FRACTALD_CACHE_DIRECTORY_ROOT"),
        DirectoryKind::Logs => env::var_os("FRACTALD_LOGS_DIRECTORY_ROOT"),
    };
    if let Some(path) = configured.map(PathBuf::from) {
        if path.as_os_str().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "directory root is empty",
            ));
        }
        return Ok(path);
    }
    if fractald_platform::is_root() {
        return Ok(match kind {
            DirectoryKind::Configuration => PathBuf::from("/etc"),
            DirectoryKind::Runtime => PathBuf::from("/run"),
            DirectoryKind::State => PathBuf::from("/var/lib"),
            DirectoryKind::Cache => PathBuf::from("/var/cache"),
            DirectoryKind::Logs => PathBuf::from("/var/log"),
        });
    }
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    Ok(match kind {
        DirectoryKind::Configuration => env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config")),
        DirectoryKind::Runtime => env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!(
                    "/tmp/fractald-runtime-{}",
                    fractald_platform::effective_uid()
                ))
            }),
        DirectoryKind::State => env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state")),
        DirectoryKind::Cache => env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache")),
        DirectoryKind::Logs => env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("log"),
    })
}

fn prepare_cgroup(spec: &ServiceSpec) -> std::io::Result<Option<PathBuf>> {
    let configured_root = env::var_os("FRACTALD_CGROUP_ROOT").map(PathBuf::from);
    if configured_root.is_none()
        && spec.resources.is_empty()
        && spec.cgroup_slice.is_none()
        && spec.oom_policy == OomPolicy::Continue
        && spec.device_policy == DevicePolicy::Auto
        && spec.device_allow.is_empty()
        && spec.delegate == DelegateMode::No
    {
        return Ok(None);
    }
    let root = configured_root.unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup"));
    if root.as_os_str().is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "FRACTALD_CGROUP_ROOT is empty",
        ));
    }
    let path = service_cgroup_path(&root.join("fractald"), spec)?;
    fs::create_dir_all(&path)?;
    let result = (|| {
        let controllers = requested_cgroup_controllers(spec);
        enable_cgroup_controllers(&path, &root, &controllers)?;
        if let Some(value) = spec.resources.memory_max {
            write_cgroup_limit(&path.join("memory.max"), value)?;
        }
        if let Some(value) = spec.resources.memory_high {
            write_cgroup_limit(&path.join("memory.high"), value)?;
        }
        if let Some(value) = spec.resources.memory_min {
            write_cgroup_limit(&path.join("memory.min"), value)?;
        }
        if let Some(value) = spec.resources.memory_low {
            write_cgroup_limit(&path.join("memory.low"), value)?;
        }
        if let Some(value) = spec.resources.memory_swap_max {
            write_cgroup_limit(&path.join("memory.swap.max"), value)?;
        }
        if let Some(value) = spec.resources.cpu_weight {
            fs::write(path.join("cpu.weight"), value.to_string())?;
        }
        if let Some(value) = spec.resources.cpu_quota {
            write_cgroup_cpu_quota(&path, value)?;
        }
        if let Some(value) = spec.resources.io_weight {
            fs::write(path.join("io.weight"), format!("default {value}"))?;
        }
        if let Some(value) = spec.resources.tasks_max {
            write_cgroup_limit(&path.join("pids.max"), value)?;
        }
        if let Some((default_allow, rules)) = device_filter_configuration(spec)? {
            fractald_platform::attach_device_filter(&path, default_allow, &rules)?;
        }
        let delegated = delegated_cgroup_controllers(spec);
        if !delegated.is_empty() || spec.delegate != DelegateMode::No {
            enable_delegated_cgroup_controllers(&path, &root, &delegated)?;
            delegate_cgroup_to_service(&path, spec)?;
        }
        Ok::<(), std::io::Error>(())
    })();
    if let Err(error) = result {
        cleanup_cgroup(&path);
        return Err(error);
    }
    Ok(Some(path))
}

fn device_filter_configuration(
    spec: &ServiceSpec,
) -> std::io::Result<Option<(bool, Vec<DeviceFilterRule>)>> {
    let has_explicit_rules = !spec.device_allow.is_empty();
    if spec.device_policy == DevicePolicy::Auto && !has_explicit_rules {
        return Ok(None);
    }

    let mut rules = if spec.device_policy == DevicePolicy::Closed {
        closed_device_rules()
    } else {
        Vec::new()
    };
    rules.extend(resolve_device_allow_rules(&spec.device_allow)?);
    Ok(Some((false, rules)))
}

fn closed_device_rules() -> Vec<DeviceFilterRule> {
    [3_u32, 5, 7, 8, 9]
        .into_iter()
        .map(|minor| DeviceFilterRule {
            device_type: 2,
            major: 1,
            minor,
            access: 7,
        })
        .collect()
}

fn resolve_device_allow_rules(
    directives: &[DeviceAccessRule],
) -> std::io::Result<Vec<DeviceFilterRule>> {
    let mut rules = Vec::new();
    for directive in directives {
        if let Some(group) = directive.device.strip_prefix("char-") {
            rules.extend(resolve_device_group_rules(2, group, directive.access)?);
        } else if let Some(group) = directive.device.strip_prefix("block-") {
            rules.extend(resolve_device_group_rules(1, group, directive.access)?);
        } else if directive.device.starts_with("/dev/") {
            let path = Path::new(&directive.device);
            let metadata = match fs::metadata(path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    eprintln!(
                        "fractald: DeviceAllow path {} is not present; no rule added",
                        path.display()
                    );
                    continue;
                }
                Err(error) => {
                    return Err(std::io::Error::new(
                        error.kind(),
                        format!(
                            "cannot inspect DeviceAllow path {}: {error}",
                            path.display()
                        ),
                    ));
                }
            };
            let file_type = metadata.file_type();
            let device_type = if file_type.is_char_device() {
                2
            } else if file_type.is_block_device() {
                1
            } else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("DeviceAllow path {} is not a device node", path.display()),
                ));
            };
            rules.push(DeviceFilterRule {
                device_type,
                major: linux_device_major(metadata.rdev()),
                minor: linux_device_minor(metadata.rdev()),
                access: u32::from(directive.access),
            });
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsupported DeviceAllow specifier {}", directive.device),
            ));
        }
    }
    Ok(rules)
}

fn resolve_device_group_rules(
    device_type: u32,
    pattern: &str,
    access: u8,
) -> std::io::Result<Vec<DeviceFilterRule>> {
    let devices = fs::read_to_string("/proc/devices")?;
    let mut section = None;
    let mut rules = Vec::new();
    for line in devices.lines() {
        let trimmed = line.trim();
        if trimmed == "Character devices:" {
            section = Some(2_u32);
            continue;
        }
        if trimmed == "Block devices:" {
            section = Some(1_u32);
            continue;
        }
        if section != Some(device_type) {
            continue;
        }
        let mut fields = trimmed.split_whitespace();
        let Some(major) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        if wildcard_match(pattern, name) {
            rules.push(DeviceFilterRule {
                device_type,
                major,
                minor: u32::MAX,
                access: u32::from(access),
            });
        }
    }
    Ok(rules)
}

fn linux_device_major(device: u64) -> u32 {
    (((device >> 8) & 0x0fff) | ((device >> 32) & 0xfffff000)) as u32
}

fn linux_device_minor(device: u64) -> u32 {
    ((device & 0x00ff) | ((device >> 12) & 0xffffff00)) as u32
}

fn requested_cgroup_controllers(spec: &ServiceSpec) -> BTreeSet<&'static str> {
    let mut controllers = BTreeSet::new();
    if spec.resources.memory_max.is_some()
        || spec.resources.memory_high.is_some()
        || spec.resources.memory_min.is_some()
        || spec.resources.memory_low.is_some()
        || spec.resources.memory_swap_max.is_some()
        || spec.oom_policy != OomPolicy::Continue
    {
        controllers.insert("memory");
    }
    if spec.resources.cpu_weight.is_some() || spec.resources.cpu_quota.is_some() {
        controllers.insert("cpu");
    }
    if spec.resources.io_weight.is_some() {
        controllers.insert("io");
    }
    if spec.resources.tasks_max.is_some() {
        controllers.insert("pids");
    }
    controllers
}

const DELEGATABLE_CGROUP_CONTROLLERS: &[&str] = &[
    "cpu", "cpuset", "io", "memory", "pids", "hugetlb", "rdma", "misc", "dmem",
];

fn delegated_cgroup_controllers(spec: &ServiceSpec) -> BTreeSet<&'static str> {
    match &spec.delegate {
        DelegateMode::No => BTreeSet::new(),
        DelegateMode::All => DELEGATABLE_CGROUP_CONTROLLERS.iter().copied().collect(),
        DelegateMode::Controllers(controllers) => controllers
            .iter()
            .filter_map(|controller| {
                DELEGATABLE_CGROUP_CONTROLLERS
                    .iter()
                    .copied()
                    .find(|candidate| *candidate == controller)
            })
            .collect(),
    }
}

fn enable_delegated_cgroup_controllers(
    path: &std::path::Path,
    cgroup_root: &std::path::Path,
    controllers: &BTreeSet<&str>,
) -> std::io::Result<()> {
    let mut enabled = BTreeSet::new();
    for controller in controllers {
        let requested = [*controller].into_iter().collect();
        match enable_cgroup_controllers(path, cgroup_root, &requested) {
            Ok(()) => {
                enabled.insert(*controller);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                eprintln!(
                    "fractald: delegated cgroup controller {controller} is unavailable; continuing"
                );
            }
            Err(error) => return Err(error),
        }
    }
    if enabled.is_empty() {
        return Ok(());
    }
    let subtree_control = path.join("cgroup.subtree_control");
    let current = fs::read_to_string(&subtree_control)?;
    let missing = enabled
        .iter()
        .filter(|controller| {
            !current
                .split_whitespace()
                .any(|value| value == **controller)
        })
        .map(|controller| format!("+{controller}"))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        fs::write(subtree_control, missing.join(" "))?;
    }
    Ok(())
}

fn delegate_cgroup_to_service(path: &std::path::Path, spec: &ServiceSpec) -> std::io::Result<()> {
    if spec.user.is_none() && spec.group.is_none() {
        return Ok(());
    }
    let user = spec
        .user
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Delegate User contains NUL",
            )
        })?;
    let group = spec
        .group
        .as_deref()
        .map(CString::new)
        .transpose()
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Delegate Group contains NUL",
            )
        })?;
    for relative in [
        PathBuf::new(),
        PathBuf::from("cgroup.procs"),
        PathBuf::from("cgroup.threads"),
        PathBuf::from("cgroup.subtree_control"),
    ] {
        let target = path.join(relative);
        if !target.exists() {
            continue;
        }
        let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "cgroup path contains NUL")
        })?;
        fractald_platform::chown_path(&target, user.as_deref(), group.as_deref())?;
    }
    Ok(())
}

fn enable_cgroup_controllers(
    path: &std::path::Path,
    cgroup_root: &std::path::Path,
    controllers: &BTreeSet<&str>,
) -> std::io::Result<()> {
    if controllers.is_empty() {
        return Ok(());
    }
    let mut ancestors = Vec::new();
    let mut current = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("cgroup path {} has no parent", path.display()),
        )
    })?;
    loop {
        if !current.starts_with(cgroup_root) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "cgroup path {} escapes root {}",
                    path.display(),
                    cgroup_root.display()
                ),
            ));
        }
        ancestors.push(current.to_owned());
        if current == cgroup_root {
            break;
        }
        current = current.parent().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("cgroup path {} is outside root", path.display()),
            )
        })?;
    }
    ancestors.reverse();

    for ancestor in ancestors {
        let available = fs::read_to_string(ancestor.join("cgroup.controllers"))?;
        let enabled_path = ancestor.join("cgroup.subtree_control");
        let enabled = fs::read_to_string(&enabled_path)?;
        let mut missing = Vec::new();
        for controller in controllers {
            if !available
                .split_whitespace()
                .any(|value| value == *controller)
                && !enabled.split_whitespace().any(|value| value == *controller)
            {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    format!(
                        "cgroup controller {controller} is unavailable at {}",
                        ancestor.display()
                    ),
                ));
            }
            if !enabled.split_whitespace().any(|value| value == *controller) {
                missing.push(format!("+{controller}"));
            }
        }
        if !missing.is_empty() {
            fs::write(enabled_path, missing.join(" "))?;
        }
    }
    Ok(())
}

fn write_cgroup_limit(path: &std::path::Path, value: LimitValue) -> std::io::Result<()> {
    let value = match value {
        LimitValue::Max => "max".to_owned(),
        LimitValue::Value(value) => value.to_string(),
    };
    fs::write(path, value)
}

fn write_cgroup_cpu_quota(path: &std::path::Path, value: CpuQuota) -> std::io::Result<()> {
    fs::write(
        path.join("cpu.max"),
        format!("{} {}", value.quota_usec, value.period_usec),
    )
}

fn attach_to_cgroup(path: &std::path::Path, pid: u32) -> std::io::Result<()> {
    fs::write(path.join("cgroup.procs"), pid.to_string())
}

fn read_cgroup_oom_events(path: &std::path::Path) -> Option<u64> {
    fs::read_to_string(path.join("memory.events"))
        .ok()?
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(char::is_whitespace)?;
            (key == "oom")
                .then(|| value.trim().parse::<u64>().ok())
                .flatten()
        })
}

fn kill_cgroup(path: &std::path::Path) -> std::io::Result<()> {
    let kill_path = path.join("cgroup.kill");
    if kill_path.exists() {
        return fs::write(kill_path, b"1");
    }
    let processes = fs::read_to_string(path.join("cgroup.procs"))?;
    for pid in processes
        .lines()
        .filter_map(|value| value.trim().parse::<u32>().ok())
    {
        if pid > 1 && pid != std::process::id() {
            let _ = fractald_platform::signal_process_group(pid, SIGKILL);
            let _ = fractald_platform::signal_process(pid, SIGKILL);
        }
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            kill_cgroup(&entry.path())?;
        }
    }
    Ok(())
}

fn cleanup_cgroup(path: &std::path::Path) {
    cleanup_cgroup_descendants(path);
    let mut current = path.to_owned();
    loop {
        let _ = fs::remove_dir(&current);
        let Some(parent) = current.parent() else {
            break;
        };
        if parent.file_name().is_some_and(|name| name == "fractald") {
            break;
        }
        current = parent.to_owned();
    }
}

fn cleanup_cgroup_descendants(path: &std::path::Path) {
    let Ok(entries) = fs::read_dir(path) else {
        let _ = fractald_platform::detach_device_filter(path);
        let _ = fs::remove_dir(path);
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|file_type| file_type.is_dir()) {
            cleanup_cgroup_descendants(&entry.path());
        }
    }
    let _ = fractald_platform::detach_device_filter(path);
    let _ = fs::remove_dir(path);
}

fn cgroup_contains_pid(path: &std::path::Path, pid: u32) -> bool {
    if fs::read_to_string(path.join("cgroup.procs")).is_ok_and(|processes| {
        processes
            .lines()
            .any(|value| value.trim().parse::<u32>().ok() == Some(pid))
    }) {
        return true;
    }
    fs::read_dir(path).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                && cgroup_contains_pid(&entry.path(), pid)
        })
    })
}

fn reconcile_cgroup_tree(
    path: &std::path::Path,
    reconciled: &mut usize,
) -> Result<(), SupervisorError> {
    let mut children = Vec::new();
    let entries = fs::read_dir(path).map_err(|error| {
        SupervisorError::new(format!(
            "cannot enumerate stale cgroups in {}: {error}",
            path.display()
        ))
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            SupervisorError::new(format!(
                "cannot enumerate stale cgroups in {}: {error}",
                path.display()
            ))
        })?;
        let child = entry.path();
        let metadata = fs::symlink_metadata(&child).map_err(|error| {
            SupervisorError::new(format!(
                "cannot inspect stale cgroup {}: {error}",
                child.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || metadata.is_dir() {
            if metadata.is_dir() {
                children.push(child);
            }
        }
    }
    if children.is_empty() {
        reconcile_cgroup(path)?;
        *reconciled += 1;
        return Ok(());
    }
    for child in children {
        reconcile_cgroup_tree(&child, reconciled)?;
    }
    cleanup_cgroup(path);
    Ok(())
}

fn service_cgroup_path(
    managed_root: &std::path::Path,
    spec: &ServiceSpec,
) -> std::io::Result<PathBuf> {
    let mut path = managed_root.to_owned();
    if let Some(slice) = spec.cgroup_slice.as_ref() {
        let slice = expand_specifiers(OsStr::new(slice), spec)
            .to_string_lossy()
            .into_owned();
        if slice == "-.slice" {
            // The root slice is represented by the FractalD cgroup itself.
        } else if !slice.ends_with(".slice") || slice.len() <= ".slice".len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid expanded Slice={slice}"),
            ));
        } else {
            let stem = &slice[..slice.len() - ".slice".len()];
            let mut current = String::new();
            for (index, component) in stem.split('-').enumerate() {
                if component.is_empty()
                    || component.contains('/')
                    || component
                        .chars()
                        .any(|character| character.is_whitespace() || character.is_control())
                {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!("invalid expanded Slice={slice}"),
                    ));
                }
                if index != 0 {
                    current.push('-');
                }
                current.push_str(component);
                path.push(format!("{current}.slice"));
            }
        }
    }
    path.push(&spec.name);
    Ok(path)
}

fn reconcile_cgroup(path: &std::path::Path) -> Result<(), SupervisorError> {
    let kill_path = path.join("cgroup.kill");
    if kill_path.exists() {
        fs::write(&kill_path, b"1").map_err(|error| {
            SupervisorError::new(format!(
                "cannot terminate stale cgroup {}: {error}",
                path.display()
            ))
        })?;
    } else if let Ok(contents) = fs::read_to_string(path.join("cgroup.procs")) {
        for pid in contents
            .lines()
            .filter_map(|line| line.trim().parse::<u32>().ok())
        {
            if pid > 1 && pid != std::process::id() {
                let _ = fractald_platform::signal_process_group(pid, SIGKILL);
                let _ = fractald_platform::signal_process(pid, SIGKILL);
            }
        }
    }
    cleanup_cgroup(path);
    Ok(())
}

fn expand_main_pid(argument: &OsString, pid: u32) -> OsString {
    let value = argument.to_string_lossy();
    let pid = pid.to_string();
    OsString::from(value.replace("${MAINPID}", &pid).replace("$MAINPID", &pid))
}

fn forking_process_candidate(spec: &ServiceSpec, process_group: u32) -> Option<u32> {
    if let Some(path) = spec.pid_file.as_ref() {
        let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
        if let Ok(value) = fs::read_to_string(path) {
            if let Some(pid) = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
            {
                if pid != 0 && process_exists(pid) {
                    return Some(pid);
                }
            }
        }
    }

    let mut candidates = Vec::new();
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == process_group {
            continue;
        }
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat,
            Err(_) => continue,
        };
        let Some((_, fields)) = stat.rsplit_once(") ") else {
            continue;
        };
        let mut fields = fields.split_whitespace();
        let Some(state) = fields.next() else {
            continue;
        };
        let _parent = fields.next();
        let Some(group) = fields.next().and_then(|value| value.parse::<u32>().ok()) else {
            continue;
        };
        if group == process_group && state != "Z" {
            candidates.push(pid);
        }
    }
    candidates.sort_unstable();
    candidates.into_iter().next()
}

fn process_exists(pid: u32) -> bool {
    fs::metadata(format!("/proc/{pid}"))
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn process_group_id(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(") ")?;
    let mut fields = fields.split_whitespace();
    fields.next()?;
    fields.next()?;
    fields.next()?.parse().ok()
}

fn observe_path_watch(watch: &PathWatch, spec: &ServiceSpec) -> PathObservation {
    let path = match watch {
        PathWatch::Changed(path)
        | PathWatch::Modified(path)
        | PathWatch::Exists(path)
        | PathWatch::DirectoryNotEmpty(path) => {
            PathBuf::from(expand_specifiers(path.as_os_str(), spec))
        }
        PathWatch::ExistsGlob(pattern) => {
            let pattern = PathBuf::from(expand_specifiers(pattern.as_os_str(), spec));
            return PathObservation {
                exists: path_exists_glob(&pattern),
                is_directory: false,
                length: 0,
                modified: None,
                directory_nonempty: false,
            };
        }
    };
    let Ok(metadata) = fs::metadata(&path) else {
        return PathObservation {
            exists: false,
            is_directory: false,
            length: 0,
            modified: None,
            directory_nonempty: false,
        };
    };
    let is_directory = metadata.is_dir();
    let directory_nonempty = is_directory
        && fs::read_dir(&path)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
    PathObservation {
        exists: true,
        is_directory,
        length: metadata.len(),
        modified: metadata.modified().ok(),
        directory_nonempty,
    }
}

#[derive(Clone, Copy, Debug)]
struct CivilTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    weekday: u32,
}

fn calendar_match(expression: &str, now: SystemTime) -> Option<u64> {
    let seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let time = CivilTime {
        year: 0,
        month: 0,
        day: 0,
        hour: (day_seconds / 3_600) as u32,
        minute: ((day_seconds % 3_600) / 60) as u32,
        second: (day_seconds % 60) as u32,
        weekday: ((days + 4).rem_euclid(7)) as u32,
    };
    let (year, month, day) = civil_date(days);
    let time = CivilTime {
        year,
        month,
        day,
        ..time
    };
    let expression = expression.trim();
    let alias = expression.to_ascii_lowercase();
    match alias.as_str() {
        "minutely" => return (time.second == 0).then_some(seconds),
        "hourly" => return (time.minute == 0 && time.second == 0).then_some(seconds),
        "daily" | "midnight" => {
            return (time.hour == 0 && time.minute == 0 && time.second == 0).then_some(seconds);
        }
        "weekly" => {
            return (time.weekday == 0 && time.hour == 0 && time.minute == 0 && time.second == 0)
                .then_some(seconds);
        }
        "monthly" => {
            return (time.day == 1 && time.hour == 0 && time.minute == 0 && time.second == 0)
                .then_some(seconds);
        }
        "quarterly" => {
            return (matches!(time.month, 1 | 4 | 7 | 10)
                && time.day == 1
                && time.hour == 0
                && time.minute == 0
                && time.second == 0)
                .then_some(seconds);
        }
        "yearly" | "annually" => {
            return (time.month == 1
                && time.day == 1
                && time.hour == 0
                && time.minute == 0
                && time.second == 0)
                .then_some(seconds);
        }
        _ => {}
    }

    let tokens = expression.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    let mut weekday = None;
    let mut date = None;
    let mut clock = None;
    for token in tokens {
        if token.eq_ignore_ascii_case("utc") || token.eq_ignore_ascii_case("local") {
            continue;
        }
        if token.contains(':') {
            if clock.replace(token).is_some() {
                return None;
            }
        } else if looks_like_date(token) {
            if date.replace(token).is_some() {
                return None;
            }
        } else if looks_like_weekday(token) {
            if weekday.replace(token).is_some() {
                return None;
            }
        } else {
            return None;
        }
    }

    if let Some(weekday) = weekday.as_ref() {
        if !weekday_matches(weekday, time.weekday) {
            return None;
        }
    }
    if let Some(date) = date.as_ref() {
        let components = date.split('-').collect::<Vec<_>>();
        if components.len() != 3
            || !matches_component(components[0], time.year as u32)
            || !matches_component(components[1], time.month)
            || !matches_component(components[2], time.day)
        {
            return None;
        }
    }
    if let Some(clock) = clock {
        let components = clock.split(':').collect::<Vec<_>>();
        if !(2..=3).contains(&components.len()) {
            return None;
        }
        let second = components
            .get(2)
            .map_or("0", |value| value.split('.').next().unwrap_or(value));
        if !matches_component(components[0], time.hour)
            || !matches_component(components[1], time.minute)
            || !matches_component(second, time.second)
        {
            return None;
        }
    } else if weekday.is_some() || date.is_some() {
        if time.hour != 0 || time.minute != 0 || time.second != 0 {
            return None;
        }
    } else {
        return None;
    }
    Some(seconds)
}

fn timer_state_directory() -> PathBuf {
    if let Some(path) = env::var_os("FRACTALD_TIMER_STATE_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = env::var_os("FRACTALD_STATE_DIR") {
        return PathBuf::from(path).join("timers");
    }
    if fractald_platform::is_root() {
        return PathBuf::from("/var/lib/fractald/timers");
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return PathBuf::from(path).join("fractald/timers");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".local/state/fractald/timers");
    }
    PathBuf::from(format!(
        "/tmp/fractald-timers-{}",
        fractald_platform::effective_uid()
    ))
}

fn timer_state_path(spec: &ServiceSpec) -> PathBuf {
    timer_state_directory().join(format!("{}.last", spec.name))
}

fn timer_last_fire(spec: &ServiceSpec) -> Option<SystemTime> {
    let value = fs::read_to_string(timer_state_path(spec)).ok()?;
    let seconds = value.trim().parse::<u64>().ok()?;
    UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

fn record_timer_fire(spec: &ServiceSpec) -> std::io::Result<()> {
    let path = timer_state_path(spec);
    let directory = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "timer state path has no parent directory",
        )
    })?;
    fs::create_dir_all(directory)?;
    let temporary = directory.join(format!(".{}.{}.tmp", spec.name, std::process::id()));
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "system clock is before the Unix epoch",
            )
        })?
        .as_secs()
        .to_string();
    if let Err(error) = fs::write(&temporary, seconds) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

fn timer_random_delay(spec: &ServiceSpec, maximum: Duration) -> Duration {
    if maximum.is_zero() {
        return Duration::ZERO;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    let span = maximum.as_nanos().saturating_add(1);
    let offset = u128::from(hasher.finish()) % span;
    Duration::new(
        (offset / 1_000_000_000) as u64,
        (offset % 1_000_000_000) as u32,
    )
}

fn timer_has_missed_schedule(spec: &ServiceSpec, now: SystemTime) -> bool {
    let Some(last) = timer_last_fire(spec) else {
        return false;
    };
    let Some(TriggerSpec::Timer {
        on_unit_active,
        on_unit_inactive,
        on_calendar,
        ..
    }) = spec.trigger.as_ref()
    else {
        return false;
    };
    if on_calendar
        .iter()
        .any(|expression| calendar_missed_since(expression, last, now))
    {
        return true;
    }
    [*on_unit_active, *on_unit_inactive]
        .into_iter()
        .flatten()
        .any(|delay| {
            now.duration_since(last)
                .is_ok_and(|elapsed| elapsed >= delay)
        })
}

fn calendar_missed_since(expression: &str, last: SystemTime, now: SystemTime) -> bool {
    let Ok(elapsed) = now.duration_since(last) else {
        return false;
    };
    if elapsed.is_zero() {
        return false;
    }
    let Some(now_seconds) = now
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
    else {
        return false;
    };
    let Some(last_seconds) = last
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|value| value.as_secs())
    else {
        return false;
    };
    let second_precision = expression
        .split_whitespace()
        .filter(|token| token.contains(':'))
        .any(|token| {
            token
                .split(':')
                .nth(2)
                .and_then(|value| value.split('.').next())
                .and_then(|value| value.parse::<u32>().ok())
                .is_some_and(|value| value != 0)
        });
    let step = if second_precision { 1 } else { 60 };
    let mut candidate = last_seconds.saturating_add(1);
    if step == 60 {
        candidate = candidate.div_ceil(60).saturating_mul(60);
    }
    let scan_end = now_seconds.min(last_seconds.saturating_add(370 * 86_400));
    while candidate <= scan_end {
        if calendar_match(expression, UNIX_EPOCH + Duration::from_secs(candidate)).is_some() {
            return true;
        }
        candidate = candidate.saturating_add(step);
        if candidate == u64::MAX {
            break;
        }
    }
    now_seconds > last_seconds.saturating_add(370 * 86_400)
}

fn civil_date(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted / 146_097
    } else {
        (shifted - 146_096) / 146_097
    };
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_part = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_part + 2) / 5 + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    (year as i32, month as u32, day as u32)
}

fn looks_like_date(value: &str) -> bool {
    let components = value.split('-').collect::<Vec<_>>();
    components.len() == 3
        && components.iter().all(|component| {
            !component.is_empty()
                && component.chars().all(|character| {
                    character.is_ascii_digit() || matches!(character, '*' | '.' | ',' | '/')
                })
        })
}

fn looks_like_weekday(value: &str) -> bool {
    value.split([',', '.']).any(|part| {
        matches!(
            part.to_ascii_lowercase().as_str(),
            "mon"
                | "monday"
                | "tue"
                | "tuesday"
                | "wed"
                | "wednesday"
                | "thu"
                | "thursday"
                | "fri"
                | "friday"
                | "sat"
                | "saturday"
                | "sun"
                | "sunday"
        )
    })
}

fn weekday_matches(expression: &str, weekday: u32) -> bool {
    expression.split(',').any(|part| {
        let parts = part.split("..").collect::<Vec<_>>();
        match parts.as_slice() {
            [single] => weekday_number(single).is_some_and(|value| value == weekday),
            [first, last] => {
                let Some(first) = weekday_number(first) else {
                    return false;
                };
                let Some(last) = weekday_number(last) else {
                    return false;
                };
                if first <= last {
                    (first..=last).contains(&weekday)
                } else {
                    weekday >= first || weekday <= last
                }
            }
            _ => false,
        }
    })
}

fn weekday_number(value: &str) -> Option<u32> {
    match value.to_ascii_lowercase().as_str() {
        "sun" | "sunday" => Some(0),
        "mon" | "monday" => Some(1),
        "tue" | "tuesday" => Some(2),
        "wed" | "wednesday" => Some(3),
        "thu" | "thursday" => Some(4),
        "fri" | "friday" => Some(5),
        "sat" | "saturday" => Some(6),
        _ => None,
    }
}

fn matches_component(expression: &str, value: u32) -> bool {
    expression.split(',').any(|part| {
        let (base, step) = part
            .split_once('/')
            .map_or((part, 1), |(base, step)| (base, step.parse().unwrap_or(0)));
        if step == 0 {
            return false;
        }
        if base == "*" {
            return value % step == 0;
        }
        let range = base.split("..").collect::<Vec<_>>();
        match range.as_slice() {
            [single] => single
                .parse::<u32>()
                .is_ok_and(|start| value >= start && (value - start) % step == 0),
            [first, last] => {
                let Some(first) = first.parse::<u32>().ok() else {
                    return false;
                };
                let Some(last) = last.parse::<u32>().ok() else {
                    return false;
                };
                value >= first && value <= last && (value - first) % step == 0
            }
            _ => false,
        }
    })
}

fn condition_matches(condition: &Condition, spec: &ServiceSpec) -> bool {
    let (matched, negate) = match condition {
        Condition::Any(conditions) => {
            return conditions
                .iter()
                .any(|condition| condition_matches(condition, spec));
        }
        Condition::PathExists { path, negate } => (
            PathBuf::from(expand_specifiers(path.as_os_str(), spec)).exists(),
            *negate,
        ),
        Condition::PathExistsGlob { pattern, negate } => (
            path_exists_glob(&PathBuf::from(expand_specifiers(pattern.as_os_str(), spec))),
            *negate,
        ),
        Condition::DirectoryNotEmpty { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            let matched = fs::read_dir(path)
                .map(|mut entries| entries.next().is_some())
                .unwrap_or(false);
            (matched, *negate)
        }
        Condition::FileIsExecutable { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            let matched = fs::metadata(path)
                .map(|metadata| {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                    }
                    #[cfg(not(unix))]
                    {
                        metadata.is_file()
                    }
                })
                .unwrap_or(false);
            (matched, *negate)
        }
        Condition::PathIsReadWrite { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (path_is_read_write(&path), *negate)
        }
        Condition::PathIsDirectory { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (
                fs::metadata(path)
                    .map(|metadata| metadata.is_dir())
                    .unwrap_or(false),
                *negate,
            )
        }
        Condition::FileNotEmpty { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (
                fs::metadata(path)
                    .map(|metadata| metadata.is_file() && metadata.len() > 0)
                    .unwrap_or(false),
                *negate,
            )
        }
        Condition::PathIsMountPoint { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (path_is_mount_point(&path), *negate)
        }
        Condition::PathIsSymbolicLink { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (
                fs::symlink_metadata(path)
                    .map(|metadata| metadata.file_type().is_symlink())
                    .unwrap_or(false),
                *negate,
            )
        }
        Condition::KernelCommandLine { argument, negate } => {
            let argument = expand_specifiers(OsStr::new(argument), spec);
            (
                kernel_command_line_contains(&argument.to_string_lossy()),
                *negate,
            )
        }
        Condition::Virtualization { value, negate } => (virtualization_matches(value), *negate),
        Condition::Security { value, negate } => (security_enabled(value), *negate),
        Condition::ACPower {
            on_ac_power,
            negate,
        } => (ac_power_state() == *on_ac_power, *negate),
        Condition::Capability { capability, negate } => (capability_available(capability), *negate),
        Condition::KernelModuleLoaded { module, negate } => {
            let module = expand_specifiers(OsStr::new(module), spec);
            (kernel_module_loaded(&module.to_string_lossy()), *negate)
        }
        Condition::Firmware { value, negate } => (firmware_matches(value), *negate),
        Condition::FirstBoot { first_boot, negate } => (first_boot_state() == *first_boot, *negate),
        Condition::Credential { credential, negate } => {
            let credential = expand_specifiers(OsStr::new(credential), spec);
            (
                credential_available(spec, &credential.to_string_lossy()),
                *negate,
            )
        }
        Condition::ControlGroupController { controller, negate } => {
            (control_group_controller_available(controller), *negate)
        }
        Condition::Environment {
            name,
            value,
            negate,
        } => {
            let matched = match (env::var_os(name), value.as_deref()) {
                (Some(actual), Some(expected)) => actual == OsStr::new(expected),
                (Some(_), None) => true,
                (None, _) => false,
            };
            (matched, *negate)
        }
        Condition::NeedsUpdate { path, negate } => {
            let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
            (needs_update(&path), *negate)
        }
    };
    if negate { !matched } else { matched }
}

fn needs_update(path: &Path) -> bool {
    needs_update_against(path, Path::new("/usr"))
}

fn needs_update_against(path: &Path, usr: &Path) -> bool {
    let Ok(usr_modified) = fs::metadata(usr).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    let stamp = path.join(".updated");
    match fs::metadata(stamp).and_then(|metadata| metadata.modified()) {
        Ok(updated) => usr_modified > updated,
        Err(_) => true,
    }
}

const TOOLBOX_HELPERS: &[&str] = &[
    "mkdir", "mount", "mv", "rm", "rmdir", "swapon", "swapoff", "umount",
];

fn toolbox_command(
    program: &Path,
    service_type: ServiceType,
    mount_filesystem: Option<&str>,
) -> Command {
    let directory = native_toolbox_directory();
    let binary = env::var_os("FRACTALD_RUSTYBOX").map(PathBuf::from);
    let (program, argv0) = toolbox_selection(
        program,
        service_type,
        mount_filesystem,
        directory.as_deref(),
        binary.as_deref(),
    );
    let mut command = Command::new(program);
    if let Some(argv0) = argv0 {
        command.arg0(argv0);
    }
    command
}

fn toolbox_selection(
    program: &Path,
    service_type: ServiceType,
    mount_filesystem: Option<&str>,
    directory: Option<&Path>,
    binary: Option<&Path>,
) -> (PathBuf, Option<OsString>) {
    let Some(applet) = program.file_name().and_then(OsStr::to_str) else {
        return (program.to_path_buf(), None);
    };
    if !matches!(service_type, ServiceType::Mount | ServiceType::Swap)
        || !TOOLBOX_HELPERS.contains(&applet)
    {
        return (program.to_path_buf(), None);
    }
    if applet == "mount" && !kernel_mount_filesystem(mount_filesystem) {
        return (program.to_path_buf(), None);
    }
    if let Some(directory) = directory {
        let candidate = directory.join(applet);
        if toolbox_executable(&candidate) {
            return (candidate, None);
        }
    }
    if let Some(binary) = binary {
        if toolbox_executable(&binary) {
            return (binary.to_path_buf(), Some(OsString::from(applet)));
        }
    }
    (program.to_path_buf(), None)
}

fn kernel_mount_filesystem(filesystem: Option<&str>) -> bool {
    let Some(filesystem) = filesystem else {
        return false;
    };
    let filesystem = match filesystem.to_ascii_lowercase().as_str() {
        "fat" | "msdos" => "vfat".to_owned(),
        "ext" => "ext4".to_owned(),
        value => value.to_owned(),
    };
    if filesystem == "fuse"
        || filesystem == "fuseblk"
        || filesystem.starts_with("fuse.")
        || matches!(
            filesystem.as_str(),
            "9p" | "autofs"
                | "ceph"
                | "cifs"
                | "davfs"
                | "glusterfs"
                | "lustre"
                | "nfs"
                | "nfs4"
                | "smb3"
                | "smbfs"
                | "sshfs"
        )
    {
        return false;
    }
    if ["/usr/bin", "/usr/sbin", "/bin", "/sbin", "/usr/libexec"]
        .iter()
        .map(|directory| Path::new(directory).join(format!("mount.{filesystem}")))
        .any(|path| toolbox_executable(&path))
    {
        return false;
    }
    if let Ok(available) = fs::read_to_string("/proc/filesystems") {
        if available.lines().any(|line| {
            line.split_whitespace()
                .last()
                .is_some_and(|value| value.eq_ignore_ascii_case(&filesystem))
        }) {
            return true;
        }
    }
    matches!(
        filesystem.as_str(),
        "9p" | "adfs"
            | "affs"
            | "befs"
            | "bfs"
            | "bpf"
            | "binder"
            | "binfmt_misc"
            | "btrfs"
            | "cgroup"
            | "cgroup2"
            | "configfs"
            | "cramfs"
            | "debugfs"
            | "devpts"
            | "devtmpfs"
            | "efivarfs"
            | "erofs"
            | "exfat"
            | "ext"
            | "ext2"
            | "ext3"
            | "ext4"
            | "f2fs"
            | "fat"
            | "fusectl"
            | "hugetlbfs"
            | "hfs"
            | "hfsplus"
            | "isofs"
            | "iso9660"
            | "jffs2"
            | "jfs"
            | "minix"
            | "msdos"
            | "mqueue"
            | "nfsd"
            | "nilfs2"
            | "ntfs"
            | "ntfs3"
            | "overlay"
            | "proc"
            | "pstore"
            | "pipefs"
            | "qnx4"
            | "qnx6"
            | "ramfs"
            | "reiserfs"
            | "romfs"
            | "rpc_pipefs"
            | "securityfs"
            | "selinuxfs"
            | "sockfs"
            | "squashfs"
            | "sysfs"
            | "tmpfs"
            | "tracefs"
            | "udf"
            | "ubifs"
            | "ufs"
            | "vfat"
            | "xfs"
            | "zonefs"
    )
}

fn prepend_toolbox_path(environment: &mut BTreeMap<OsString, OsString>) {
    let Some(directory) = native_toolbox_directory() else {
        return;
    };
    let existing = environment
        .get(OsStr::new("PATH"))
        .cloned()
        .or_else(|| env::var_os("PATH"));
    let mut paths = vec![PathBuf::from(directory)];
    if let Some(existing) = existing {
        paths.extend(env::split_paths(&existing));
    }
    if let Ok(path) = env::join_paths(paths) {
        environment.insert(OsString::from("PATH"), path);
    }
}

fn native_toolbox_directory() -> Option<PathBuf> {
    if let Some(path) = env::var_os("FRACTALD_TOOLBOX_DIR") {
        return Some(PathBuf::from(path));
    }
    [
        "/usr/lib/fractald/toolbox",
        "/usr/libexec/fractald/toolbox",
        "/usr/local/lib/fractald/toolbox",
        "/usr/local/libexec/fractald/toolbox",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|path| path.is_dir())
}

fn toolbox_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn mount_info_for(path: &Path) -> Option<(PathBuf, bool)> {
    let path = fs::canonicalize(path).ok()?;
    let contents = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(PathBuf, bool)> = None;
    for line in contents.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields = left.split_whitespace().collect::<Vec<_>>();
        if fields.len() < 6 {
            continue;
        }
        let mount_point = decode_mountinfo_path(fields[4]);
        if !(path == mount_point || path.starts_with(&mount_point)) {
            continue;
        }
        let mount_options_read_only = fields[5].split(',').any(|option| option == "ro");
        let super_options_read_only = right
            .split_whitespace()
            .nth(2)
            .is_some_and(|options| options.split(',').any(|option| option == "ro"));
        let read_only = mount_options_read_only || super_options_read_only;
        let depth = mount_point.components().count();
        if best
            .as_ref()
            .is_none_or(|(best_point, _)| depth > best_point.components().count())
        {
            best = Some((mount_point, read_only));
        }
    }
    best
}

fn decode_mountinfo_path(value: &str) -> PathBuf {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                decoded.push(
                    ((digits[0] - b'0') << 6) | ((digits[1] - b'0') << 3) | (digits[2] - b'0'),
                );
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    PathBuf::from(String::from_utf8_lossy(&decoded).into_owned())
}

fn path_is_mount_point(path: &Path) -> bool {
    if let Some((mount_point, _)) = mount_info_for(path) {
        return fs::canonicalize(path).is_ok_and(|path| path == mount_point);
    }
    if path == Path::new("/") {
        return path.exists();
    }
    let Ok(path) = fs::canonicalize(path) else {
        return false;
    };
    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(path_metadata) = fs::metadata(&path) else {
        return false;
    };
    let Ok(parent_metadata) = fs::metadata(parent) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        path_metadata.dev() != parent_metadata.dev()
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn skipped_mount_is_available(spec: &ServiceSpec) -> bool {
    spec.conditions.iter().any(|condition| {
        let Condition::PathIsMountPoint { path, negate: true } = condition else {
            return false;
        };
        let path = PathBuf::from(expand_specifiers(path.as_os_str(), spec));
        path_is_mount_point(&path)
    })
}

fn path_is_read_write(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    if let Some((_, read_only)) = mount_info_for(path) {
        return !read_only;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o222 != 0)
            .unwrap_or(false);
    }
    #[cfg(not(unix))]
    {
        fs::metadata(path)
            .map(|metadata| !metadata.permissions().readonly())
            .unwrap_or(false)
    }
}

fn kernel_command_line_contains(argument: &str) -> bool {
    let Ok(command_line) = fs::read_to_string("/proc/cmdline") else {
        return false;
    };
    command_line.split_whitespace().any(|token| {
        token == argument
            || (!argument.contains('=')
                && token
                    .strip_prefix(argument)
                    .is_some_and(|suffix| suffix.starts_with('=')))
    })
}

fn detect_container() -> Option<String> {
    if let Ok(value) = fs::read_to_string("/run/fractald/container") {
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() {
            return Some(value);
        }
    }
    if let Ok(environment) = fs::read_to_string("/proc/1/environ") {
        for entry in environment.split('\0') {
            if let Some(value) = entry.strip_prefix("container=") {
                if !value.is_empty() {
                    return Some(value.to_ascii_lowercase());
                }
            }
        }
    }
    let cgroup = fs::read_to_string("/proc/1/cgroup").ok()?;
    let cgroup = cgroup.to_ascii_lowercase();
    let known = [
        ("docker", "docker"),
        ("podman", "podman"),
        ("libpod", "podman"),
        ("lxc", "lxc"),
        ("kubepods", "kubernetes"),
        ("containerd", "containerd"),
    ];
    known
        .iter()
        .find(|(marker, _)| cgroup.contains(marker))
        .map(|(_, value)| (*value).to_owned())
}

fn detect_virtual_machine() -> Option<String> {
    if let Ok(value) = fs::read_to_string("/sys/hypervisor/type") {
        let value = value.trim().to_ascii_lowercase();
        if !value.is_empty() {
            return Some(value);
        }
    }
    let product = fs::read_to_string("/sys/class/dmi/id/product_name")
        .or_else(|_| fs::read_to_string("/sys/devices/virtual/dmi/id/product_name"))
        .ok()?
        .to_ascii_lowercase();
    let vendors = [
        ("vmware", "vmware"),
        ("virtualbox", "oracle"),
        ("kvm", "kvm"),
        ("qemu", "qemu"),
        ("bochs", "bochs"),
        ("microsoft", "microsoft"),
        ("hyper-v", "microsoft"),
        ("xen", "xen"),
        ("google", "google"),
    ];
    vendors
        .iter()
        .find(|(marker, _)| product.contains(marker))
        .map(|(_, value)| (*value).to_owned())
}

fn virtualization_matches(value: &str) -> bool {
    let expected = value.to_ascii_lowercase();
    if expected == "container" {
        return detect_container().is_some();
    }
    if expected == "no" {
        return detect_container().is_none() && detect_virtual_machine().is_none();
    }
    detect_container().is_some_and(|detected| detected == expected)
        || detect_virtual_machine().is_some_and(|detected| detected == expected)
}

fn security_enabled(value: &str) -> bool {
    match value.to_ascii_lowercase().as_str() {
        "selinux" => Path::new("/sys/fs/selinux").exists(),
        "apparmor" => Path::new("/sys/module/apparmor").exists(),
        "smack" => Path::new("/sys/fs/smackfs").exists(),
        "tomoyo" => Path::new("/sys/kernel/security/tomoyo").exists(),
        "landlock" => fs::read_to_string("/sys/kernel/security/lsm")
            .map(|lsm| lsm.split(',').any(|entry| entry.trim() == "landlock"))
            .unwrap_or(false),
        "measured-uki" => false,
        _ => false,
    }
}

fn ac_power_state() -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/power_supply") else {
        return true;
    };
    let mut has_ac = false;
    let mut has_battery = false;
    let mut ac_online = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name.starts_with("ac") || name.starts_with("adp") || name.starts_with("mains") {
            has_ac = true;
            ac_online |= fs::read_to_string(entry.path().join("online"))
                .is_ok_and(|value| value.trim() == "1");
        } else if name.starts_with("bat") {
            has_battery = true;
        }
    }
    if has_ac { ac_online } else { !has_battery }
}

fn capability_available(value: &str) -> bool {
    let name = value.to_ascii_uppercase();
    let bit: u32 = match name.strip_prefix("CAP_").unwrap_or(&name) {
        "CHOWN" => 0,
        "DAC_OVERRIDE" => 1,
        "DAC_READ_SEARCH" => 2,
        "FOWNER" => 3,
        "FSETID" => 4,
        "KILL" => 5,
        "SETGID" => 6,
        "SETUID" => 7,
        "SETPCAP" => 8,
        "LINUX_IMMUTABLE" => 9,
        "NET_BIND_SERVICE" => 10,
        "NET_BROADCAST" => 11,
        "NET_ADMIN" => 12,
        "NET_RAW" => 13,
        "IPC_LOCK" => 14,
        "IPC_OWNER" => 15,
        "SYS_MODULE" => 16,
        "SYS_RAWIO" => 17,
        "SYS_CHROOT" => 18,
        "SYS_PTRACE" => 19,
        "SYS_PACCT" => 20,
        "SYS_ADMIN" => 21,
        "SYS_BOOT" => 22,
        "SYS_NICE" => 23,
        "SYS_RESOURCE" => 24,
        "SYS_TIME" => 25,
        "SYS_TTY_CONFIG" => 26,
        "MKNOD" => 27,
        "LEASE" => 28,
        "AUDIT_WRITE" => 29,
        "AUDIT_CONTROL" => 30,
        "SETFCAP" => 31,
        "MAC_OVERRIDE" => 32,
        "MAC_ADMIN" => 33,
        "SYSLOG" => 34,
        "WAKE_ALARM" => 35,
        "BLOCK_SUSPEND" => 36,
        "AUDIT_READ" => 37,
        "PERFMON" => 38,
        "BPF" => 39,
        "CHECKPOINT_RESTORE" => 40,
        _ => return false,
    };
    let Some(effective) = fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:").map(str::trim))
                .and_then(|value| u128::from_str_radix(value, 16).ok())
        })
    else {
        return false;
    };
    effective & (1_u128 << bit) != 0
}

fn kernel_module_loaded(module: &str) -> bool {
    let module = module.replace('-', "_");
    fs::read_to_string("/proc/modules")
        .map(|modules| {
            modules.lines().any(|line| {
                line.split_whitespace()
                    .next()
                    .is_some_and(|name| name == module)
            })
        })
        .unwrap_or(false)
}

fn firmware_matches(value: &str) -> bool {
    match value.to_ascii_lowercase().as_str() {
        "uefi" => Path::new("/sys/firmware/efi").exists(),
        "bios" => !Path::new("/sys/firmware/efi").exists(),
        _ => false,
    }
}

fn first_boot_state() -> bool {
    fs::read_to_string("/etc/machine-id")
        .map(|value| value.trim().is_empty())
        .unwrap_or(true)
}

fn credential_available(spec: &ServiceSpec, credential: &str) -> bool {
    let import_dirs = credential_import_dirs();
    spec.credentials.iter().any(|entry| {
        if entry.name != credential {
            return false;
        }
        match &entry.source {
            CredentialSource::Value(_) => true,
            CredentialSource::File(path) => {
                let path = expand_specifiers(path.as_os_str(), spec);
                Path::new(&path).is_file()
            }
            CredentialSource::Store(store_name) => {
                let store_name = expand_specifiers(OsStr::new(store_name), spec);
                store_name
                    .to_str()
                    .filter(|name| safe_credential_name(name))
                    .and_then(|name| credential_store_entry(name, &import_dirs))
                    .is_some()
            }
        }
    }) || [
        PathBuf::from("/run/credentials")
            .join(&spec.name)
            .join(credential),
        PathBuf::from("/run/fractald/credentials")
            .join(&spec.name)
            .join(credential),
    ]
    .iter()
    .any(|path| path.exists())
        || spec.credential_imports.iter().any(|import| {
            let import = expand_credential_import(import, spec);
            credential_import_candidates(&import, &import_dirs)
                .keys()
                .any(|source_name| imported_credential_name(&import, source_name) == credential)
        })
}

fn control_group_controller_available(controller: &str) -> bool {
    let controller = controller.to_ascii_lowercase();
    if controller == "v2" {
        return Path::new("/sys/fs/cgroup/cgroup.controllers").exists();
    }
    fs::read_to_string("/sys/fs/cgroup/cgroup.controllers")
        .map(|controllers| {
            controllers
                .split_whitespace()
                .any(|value| value == controller)
        })
        .unwrap_or(false)
}

fn open_listeners(
    specifications: &[fractald_core::ListenerSpec],
    mode: u32,
    user: Option<&str>,
    group: Option<&str>,
) -> std::io::Result<Vec<ManagedListener>> {
    let user = user.map(CString::new).transpose().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "SocketUser contains NUL")
    })?;
    let group = group.map(CString::new).transpose().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "SocketGroup contains NUL")
    })?;
    let mut listeners = Vec::new();
    for specification in specifications {
        let address = &specification.address;
        let listener = match specification.kind {
            fractald_core::ListenerKind::Fifo => {
                let path = PathBuf::from(address);
                let fd = fractald_platform::open_fifo(&path, mode)?;
                ManagedListener::Fifo {
                    file: unsafe { fs::File::from_raw_fd(fd) },
                    path,
                }
            }
            fractald_core::ListenerKind::Special => ManagedListener::Special {
                file: unsafe {
                    fs::File::from_raw_fd(fractald_platform::open_special(
                        Path::new(address),
                        true,
                    )?)
                },
            },
            fractald_core::ListenerKind::Netlink => {
                let (protocol, groups) = parse_netlink_address(address)?;
                ManagedListener::Netlink {
                    file: unsafe {
                        fs::File::from_raw_fd(fractald_platform::open_netlink(protocol, groups)?)
                    },
                }
            }
            fractald_core::ListenerKind::SequentialPacket => {
                let path = if address.starts_with('@') {
                    PathBuf::new()
                } else {
                    let path = PathBuf::from(address);
                    prepare_unix_socket_path(&path)?;
                    path
                };
                ManagedListener::SeqPacket {
                    file: unsafe {
                        fs::File::from_raw_fd(fractald_platform::bind_unix_seqpacket(address)?)
                    },
                    path,
                }
            }
            fractald_core::ListenerKind::Stream | fractald_core::ListenerKind::Datagram => {
                if let Some(name) = address.strip_prefix('@') {
                    if name.is_empty() {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "abstract socket name is empty",
                        ));
                    }
                    let socket_address = SocketAddr::from_abstract_name(name.as_bytes())?;
                    match specification.kind {
                        fractald_core::ListenerKind::Stream => ManagedListener::UnixStream {
                            listener: UnixListener::bind_addr(&socket_address)?,
                            path: PathBuf::new(),
                        },
                        fractald_core::ListenerKind::Datagram => ManagedListener::UnixDatagram {
                            socket: UnixDatagram::bind_addr(&socket_address)?,
                            path: PathBuf::new(),
                        },
                        _ => unreachable!(),
                    }
                } else if address.starts_with('/') || address.starts_with('.') {
                    let path = PathBuf::from(address);
                    prepare_unix_socket_path(&path)?;
                    match specification.kind {
                        fractald_core::ListenerKind::Stream => ManagedListener::UnixStream {
                            listener: UnixListener::bind(&path)?,
                            path,
                        },
                        fractald_core::ListenerKind::Datagram => ManagedListener::UnixDatagram {
                            socket: UnixDatagram::bind(&path)?,
                            path,
                        },
                        _ => unreachable!(),
                    }
                } else {
                    match specification.kind {
                        fractald_core::ListenerKind::Stream => {
                            let listener = TcpListener::bind(address)?;
                            listener.set_nonblocking(true)?;
                            ManagedListener::Tcp(listener)
                        }
                        fractald_core::ListenerKind::Datagram => {
                            let socket = UdpSocket::bind(address)?;
                            socket.set_nonblocking(true)?;
                            ManagedListener::Udp(socket)
                        }
                        _ => unreachable!(),
                    }
                }
            }
        };
        if let ManagedListener::UnixStream { listener, .. } = &listener {
            listener.set_nonblocking(true)?;
        }
        if let ManagedListener::UnixDatagram { socket, .. } = &listener {
            socket.set_nonblocking(true)?;
        }
        set_listener_mode(&listener, mode)?;
        set_listener_owner(&listener, user.as_deref(), group.as_deref())?;
        listeners.push(listener);
    }
    Ok(listeners)
}

fn set_listener_mode(listener: &ManagedListener, mode: u32) -> std::io::Result<()> {
    let Some(path) = listener_path(listener) else {
        return Ok(());
    };
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(mode);
    fs::set_permissions(path, permissions)
}

fn set_listener_owner(
    listener: &ManagedListener,
    user: Option<&std::ffi::CStr>,
    group: Option<&std::ffi::CStr>,
) -> std::io::Result<()> {
    let Some(path) = listener_path(listener) else {
        return Ok(());
    };
    if user.is_none() && group.is_none() {
        return Ok(());
    }
    let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "listener path contains NUL",
        )
    })?;
    fractald_platform::chown_path(&path, user, group)
}

fn listener_path(listener: &ManagedListener) -> Option<&Path> {
    match listener {
        ManagedListener::UnixStream { path, .. }
        | ManagedListener::UnixDatagram { path, .. }
        | ManagedListener::Fifo { path, .. }
        | ManagedListener::SeqPacket { path, .. }
            if !path.as_os_str().is_empty() =>
        {
            Some(path)
        }
        _ => None,
    }
}

fn parse_netlink_address(value: &str) -> std::io::Result<(i32, u32)> {
    let mut words = value.split_whitespace();
    let protocol = match words.next().unwrap_or_default() {
        "route" => 0,
        "usersock" => 2,
        "firewall" => 3,
        "sock_diag" => 4,
        "nflog" => 5,
        "xfrm" => 6,
        "selinux" => 7,
        "iscsi" => 8,
        "audit" => 9,
        "fib_lookup" => 10,
        "connector" => 11,
        "netfilter" => 12,
        "ip6_fw" => 13,
        "dnrtmsg" => 14,
        "kobject-uevent" => 15,
        "generic" => 16,
        "scsi_transport" => 18,
        "ecryptfs" => 19,
        "rdma" => 20,
        "crypto" => 21,
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsupported netlink protocol {other}"),
            ));
        }
    };
    let groups = words.next().unwrap_or("0").parse::<u32>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid netlink multicast group in {value}"),
        )
    })?;
    if words.next().is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("too many netlink fields in {value}"),
        ));
    }
    Ok((protocol, groups))
}

fn prepare_unix_socket_path(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => fs::remove_file(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("socket path {} is not a socket", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn path_exists_glob(pattern: &Path) -> bool {
    let components = pattern
        .components()
        .filter_map(|component| match component {
            std::path::Component::RootDir => None,
            std::path::Component::Normal(value) => Some(value.to_string_lossy().into_owned()),
            std::path::Component::CurDir => Some(".".to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if components.iter().all(|component| !has_wildcard(component)) {
        return pattern.exists();
    }
    let mut base = if pattern.is_absolute() {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    let mut first_wildcard = components.len();
    for (index, component) in components.iter().enumerate() {
        if has_wildcard(component) {
            first_wildcard = index;
            break;
        }
        base.push(component);
    }
    glob_walk(&base, &components, first_wildcard)
}

fn glob_walk(base: &Path, components: &[String], index: usize) -> bool {
    if index == components.len() {
        return base.exists();
    }
    let component = &components[index];
    if component == "**" {
        if glob_walk(base, components, index + 1) {
            return true;
        }
        let Ok(entries) = fs::read_dir(base) else {
            return false;
        };
        return entries
            .flatten()
            .any(|entry| entry.path().is_dir() && glob_walk(&entry.path(), components, index));
    }
    if has_wildcard(component) {
        let Ok(entries) = fs::read_dir(base) else {
            return false;
        };
        entries.flatten().any(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| wildcard_match(component, name))
                && glob_walk(&entry.path(), components, index + 1)
        })
    } else {
        glob_walk(&base.join(component), components, index + 1)
    }
}

fn has_wildcard(value: &str) -> bool {
    value.contains(['*', '?'])
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut table = vec![vec![false; value.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for index in 0..pattern.len() {
        for value_index in 0..=value.len() {
            if !table[index][value_index] {
                continue;
            }
            match pattern[index] {
                '*' => {
                    table[index + 1][value_index] = true;
                    if value_index < value.len() {
                        table[index][value_index + 1] = true;
                    }
                }
                '?' if value_index < value.len() => {
                    table[index + 1][value_index + 1] = true;
                }
                character if value_index < value.len() && character == value[value_index] => {
                    table[index + 1][value_index + 1] = true;
                }
                _ => {}
            }
        }
    }
    table[pattern.len()][value.len()]
}

fn effective_notify_access(spec: &ServiceSpec) -> NotifyAccess {
    match spec.notify_access {
        NotifyAccess::None
            if spec.service_type == ServiceType::Notify || spec.watchdog.is_some() =>
        {
            NotifyAccess::Main
        }
        access => access,
    }
}

fn spawn_dbus_probe(bus_name: &str) -> std::io::Result<Child> {
    let mut command = Command::new("dbus-send");
    if let Some(address) = env::var_os("DBUS_STARTER_ADDRESS") {
        command.arg(format!("--address={}", address.to_string_lossy()));
    } else if env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() && !fractald_platform::is_root() {
        command.arg("--session");
    } else {
        command.arg("--system");
    }
    command
        .args([
            "--print-reply",
            "--dest=org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus.GetNameOwner",
        ])
        .arg(format!("string:{bus_name}"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

fn prepare_notify_socket(
    spec: &ServiceSpec,
    generation: u64,
) -> std::io::Result<(UnixDatagram, PathBuf)> {
    let directory = env::var_os("FRACTALD_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(format!(
                "/tmp/fractald-notify-{}",
                fractald_platform::effective_uid()
            ))
        })
        .join("notify");
    fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&directory)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&directory, permissions)?;
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.hash(&mut hasher);
    let path = directory.join(format!(
        "{}-{}-{:x}.sock",
        std::process::id(),
        generation,
        hasher.finish()
    ));
    let _ = fs::remove_file(&path);
    let socket = UnixDatagram::bind(&path)?;
    socket.set_nonblocking(true)?;
    fractald_platform::enable_socket_credentials(socket.as_raw_fd())?;
    Ok((socket, path))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupervisorError {
    pub message: String,
}

impl SupervisorError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SupervisorError {}

impl Drop for ManagedService {
    fn drop(&mut self) {
        if let Some(pid) = self.record.pid() {
            let process_group = self.process_group.unwrap_or(pid);
            let _ = fractald_platform::signal_process_group(process_group, SIGKILL);
        }
        if let Some(pidfd) = self.pidfd.as_ref() {
            let _ = pidfd.send_signal(SIGKILL);
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.kill_start_helper();
        self.kill_exec_condition_helper();
        self.kill_start_post_helper();
        self.kill_dbus_probe();
        self.kill_active_stop_helper();
        self.kill_stop_post_helper();
        self.kill_stop_command();
        self.kill_reload_command();
        let remove_on_stop = self.spec().remove_on_stop;
        self.close_listeners(remove_on_stop);
        self.clear_activation_fds();
        if let Some(path) = self.notify_path.take() {
            let _ = fs::remove_file(path);
        }
        if let Some(path) = self.cgroup_path.take() {
            cleanup_cgroup(&path);
        }
        self.cleanup_directories();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fractald_core::{
        Condition, CredentialSource, DependencySet, DirectoryKind, DirectorySpec,
        EnvironmentFileSpec, InputMode, LimitRange, LimitValue, ListenerKind, ListenerSpec,
        ManagerAction, OutputMode, PathWatch, RestartPolicy, ServiceType, TriggerSpec,
    };
    use std::ffi::OsString;
    use std::fs;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::thread;
    use std::time::Duration;

    fn service(name: &str, command: &str, args: &[&str]) -> ServiceSpec {
        let mut spec = ServiceSpec::new(name, command);
        spec.args = args.iter().map(|arg| OsString::from(arg)).collect();
        spec.stdout = OutputMode::Null;
        spec.stderr = OutputMode::Null;
        spec
    }

    #[test]
    fn resolves_closed_device_defaults_and_proc_device_groups() {
        let defaults = closed_device_rules();
        assert!(defaults.iter().any(|rule| {
            rule.device_type == 2 && rule.major == 1 && rule.minor == 3 && rule.access == 7
        }));
        let groups = resolve_device_group_rules(2, "*", 6).expect("read proc device groups");
        assert!(!groups.is_empty());
    }

    #[test]
    fn decodes_linux_device_numbers() {
        let metadata = fs::metadata("/dev/null").expect("stat /dev/null");
        assert_eq!(linux_device_major(metadata.rdev()), 1);
        assert_eq!(linux_device_minor(metadata.rdev()), 3);
    }

    fn poll_until(supervisor: &mut Supervisor, predicate: impl Fn(&Supervisor) -> bool) {
        for _ in 0..1000 {
            supervisor.poll().expect("poll supervisor");
            if predicate(supervisor) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("supervisor condition did not become true");
    }

    #[test]
    fn starts_and_stops_a_service_by_pidfd() {
        let mut supervisor = Supervisor::new();
        supervisor
            .add(service("demo.service", "/bin/sh", &["-c", "sleep 30"]))
            .expect("add service");
        supervisor.start("demo.service").expect("start service");
        let snapshot = supervisor.snapshot("demo.service").expect("snapshot");
        assert_eq!(snapshot.state, ServiceState::Running);
        assert!(snapshot.pid.is_some());

        supervisor.stop("demo.service").expect("stop service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor.snapshot("demo.service").is_some_and(|snapshot| {
                snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
            })
        });
    }

    #[test]
    fn isolates_an_allowed_target_and_preserves_ignored_units() {
        let mut supervisor = Supervisor::new();

        let mut outside = service("outside.service", "/bin/sh", &["-c", "sleep 30"]);
        outside.refuse_manual_stop = true;
        supervisor.add(outside).expect("add outside service");

        let mut ignored = service("ignored.service", "/bin/sh", &["-c", "sleep 30"]);
        ignored.ignore_on_isolate = true;
        supervisor.add(ignored).expect("add ignored service");

        let kept = service("kept.service", "/bin/sh", &["-c", "sleep 30"]);
        supervisor.add(kept).expect("add kept service");

        let mut target = service("rescue.target", "/bin/true", &[]);
        target.service_type = ServiceType::Oneshot;
        target.remain_after_exit = true;
        target.allow_isolate = true;
        target.dependencies.wants.insert("kept.service".to_owned());
        supervisor.add(target).expect("add target");

        supervisor.start("outside.service").expect("start outside");
        supervisor.start("ignored.service").expect("start ignored");
        supervisor.isolate("rescue.target").expect("isolate target");

        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("rescue.target")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
                && supervisor
                    .snapshot("kept.service")
                    .is_some_and(|snapshot| snapshot.state == ServiceState::Running)
                && supervisor
                    .snapshot("outside.service")
                    .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
                && supervisor
                    .snapshot("ignored.service")
                    .is_some_and(|snapshot| snapshot.state == ServiceState::Running)
        });

        supervisor.stop_all().expect("stop isolated services");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn isolation_requires_an_allowed_profile_service() {
        let mut supervisor = Supervisor::new();
        let mut target = service("ordinary.profile", "/bin/true", &[]);
        target.service_type = ServiceType::Oneshot;
        target.remain_after_exit = true;
        supervisor.add(target).expect("add target");
        let error = supervisor
            .isolate("ordinary.profile")
            .expect_err("target without AllowIsolate must fail");
        assert!(error.to_string().contains("does not allow isolation"));

        let service = service("ordinary.svc", "/bin/true", &[]);
        supervisor.add(service).expect("add service");
        let error = supervisor
            .isolate("ordinary.svc")
            .expect_err("isolation without permission must fail");
        assert!(error.to_string().contains("does not allow isolation"));
    }

    #[test]
    fn assigns_dynamic_identities_and_implies_private_service_defaults() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("dynamic.service", "/bin/true", &[]);
        spec.dynamic_user = true;
        supervisor.add(spec).expect("add dynamic service");

        let resolved = supervisor
            .specification("dynamic.service")
            .expect("dynamic specification");
        let uid = resolved
            .user
            .as_deref()
            .expect("dynamic user id")
            .parse::<u32>()
            .expect("numeric dynamic user id");
        let gid = resolved
            .group
            .as_deref()
            .expect("dynamic group id")
            .parse::<u32>()
            .expect("numeric dynamic group id");
        assert!((DYNAMIC_USER_MIN..=DYNAMIC_USER_MAX).contains(&uid));
        assert_eq!(uid, gid);
        assert_eq!(resolved.protect_system, ProtectSystemMode::Strict);
        assert_eq!(resolved.protect_home, ProtectHomeMode::ReadOnly);
        assert_eq!(resolved.private_tmp, PrivateTmpMode::Yes);
        assert_eq!(resolved.supplementary_groups, Some(Vec::new()));
        let original_user = resolved.user.clone();
        let original_group = resolved.group.clone();

        let mut reloaded = ServiceSpec::new("dynamic.service", "/bin/true");
        reloaded.dynamic_user = true;
        supervisor
            .reload([reloaded])
            .expect("reload dynamic service");
        let reloaded_spec = supervisor
            .specification("dynamic.service")
            .expect("reloaded dynamic specification");
        assert_eq!(reloaded_spec.user, original_user);
        assert_eq!(reloaded_spec.group, original_group);
    }

    #[test]
    fn stops_a_stop_when_unneeded_dependency_after_its_consumer_stops() {
        let mut supervisor = Supervisor::new();
        let mut dependency = service("dependency.service", "/bin/sh", &["-c", "sleep 30"]);
        dependency.stop_when_unneeded = true;
        supervisor.add(dependency).expect("add dependency");
        let mut consumer = service("consumer.service", "/bin/sh", &["-c", "sleep 30"]);
        consumer
            .dependencies
            .requires
            .insert("dependency.service".to_owned());
        supervisor.add(consumer).expect("add consumer");

        supervisor
            .start("consumer.service")
            .expect("start consumer");
        assert_eq!(
            supervisor
                .snapshot("dependency.service")
                .expect("dependency")
                .state,
            ServiceState::Running
        );
        supervisor.stop("consumer.service").expect("stop consumer");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("consumer.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
                && supervisor
                    .snapshot("dependency.service")
                    .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
    }

    #[test]
    fn resolves_declared_service_aliases_through_lifecycle_operations() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("worker.svc", "/bin/sh", &["-c", "sleep 30"]);
        spec.aliases.insert("worker-alias.svc".to_owned());
        spec.aliases.insert("worker-short".to_owned());
        supervisor.add(spec).expect("add aliased service");

        assert_eq!(
            supervisor.resolve_name("worker-alias.svc"),
            Some("worker.svc")
        );
        assert_eq!(supervisor.resolve_name("worker-short"), Some("worker.svc"));
        assert_eq!(supervisor.resolve_name("worker-alias"), Some("worker.svc"));

        supervisor
            .start("worker-alias")
            .expect("start through alias");
        assert_eq!(
            supervisor
                .snapshot("worker-short")
                .expect("aliased snapshot")
                .state,
            ServiceState::Running
        );
        supervisor
            .stop("worker-alias.svc")
            .expect("stop through alias");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn rejects_alias_collisions() {
        let mut supervisor = Supervisor::new();
        let mut first = service("first.service", "/bin/true", &[]);
        first.aliases.insert("shared.service".to_owned());
        supervisor.add(first).expect("add first aliased service");
        let mut second = service("second.service", "/bin/true", &[]);
        second.aliases.insert("shared.service".to_owned());
        let error = supervisor
            .add(second)
            .expect_err("duplicate alias must fail");
        assert!(error.to_string().contains("service alias shared.service"));
    }

    #[test]
    fn starts_a_service_again_after_a_clean_stop() {
        let mut supervisor = Supervisor::new();
        supervisor
            .add(service(
                "restartable.service",
                "/bin/sh",
                &["-c", "sleep 30"],
            ))
            .expect("add service");

        supervisor
            .start("restartable.service")
            .expect("first start");
        supervisor.stop("restartable.service").expect("first stop");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("restartable.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });

        supervisor
            .start("restartable.service")
            .expect("second start");
        let snapshot = supervisor
            .snapshot("restartable.service")
            .expect("second snapshot");
        assert_eq!(snapshot.state, ServiceState::Running);
        assert_eq!(snapshot.generation, 2);

        supervisor.stop("restartable.service").expect("second stop");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn starting_a_service_stops_a_conflicting_service_in_both_directions() {
        let mut supervisor = Supervisor::new();
        let mut first = service("first.service", "/bin/true", &[]);
        first.service_type = ServiceType::Oneshot;
        first.remain_after_exit = true;
        supervisor.add(first).expect("add first service");

        let mut second = service("second.service", "/bin/true", &[]);
        second.service_type = ServiceType::Oneshot;
        second.remain_after_exit = true;
        second
            .dependencies
            .conflicts
            .insert("first.service".to_owned());
        supervisor.add(second).expect("add second service");

        supervisor
            .start("second.service")
            .expect("start second service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("second.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });

        supervisor
            .start("first.service")
            .expect("start first service");
        assert_eq!(
            supervisor
                .snapshot("second.service")
                .expect("second snapshot")
                .state,
            ServiceState::Exited
        );
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("first.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        supervisor.stop_all().expect("stop conflicting services");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn stopping_an_owner_stops_part_of_dependents() {
        let mut supervisor = Supervisor::new();
        let mut owner = service("owner.service", "/bin/true", &[]);
        owner.service_type = ServiceType::Oneshot;
        owner.remain_after_exit = true;
        supervisor.add(owner).expect("add owner");

        let mut dependent = service("dependent.service", "/bin/true", &[]);
        dependent.service_type = ServiceType::Oneshot;
        dependent.remain_after_exit = true;
        dependent
            .dependencies
            .part_of
            .insert("owner.service".to_owned());
        supervisor.add(dependent).expect("add dependent");

        supervisor.start("owner.service").expect("start owner");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("owner.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        supervisor
            .start("dependent.service")
            .expect("start dependent");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("dependent.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });

        supervisor.stop("owner.service").expect("stop owner");
        assert_eq!(
            supervisor
                .snapshot("dependent.service")
                .expect("dependent snapshot")
                .state,
            ServiceState::Exited
        );
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn requisite_must_be_active_without_being_started_implicitly() {
        let mut prerequisite = service("prerequisite.service", "/bin/true", &[]);
        prerequisite.service_type = ServiceType::Oneshot;
        prerequisite.remain_after_exit = true;

        let mut dependent = service("dependent.service", "/bin/true", &[]);
        dependent.service_type = ServiceType::Oneshot;
        dependent.remain_after_exit = true;
        dependent
            .dependencies
            .requisite
            .insert("prerequisite.service".to_owned());

        let mut supervisor = Supervisor::new();
        supervisor.add(prerequisite).expect("add prerequisite");
        supervisor.add(dependent).expect("add dependent");

        let error = supervisor
            .start("dependent.service")
            .expect_err("inactive requisite must reject start");
        assert!(error.to_string().contains("is not active"));
        assert_eq!(
            supervisor
                .snapshot("prerequisite.service")
                .expect("prerequisite snapshot")
                .state,
            ServiceState::Defined
        );

        supervisor
            .start("prerequisite.service")
            .expect("start prerequisite");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("prerequisite.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        supervisor
            .start("dependent.service")
            .expect("start dependent after prerequisite");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("dependent.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        supervisor.stop_all().expect("stop services");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn losing_a_bound_service_stops_its_dependents() {
        let mut supervisor = Supervisor::new();
        let owner = service("unstable.service", "/bin/sh", &["-c", "sleep 0.05; exit 1"]);
        supervisor.add(owner).expect("add unstable owner");

        let mut dependent = service("bound.service", "/bin/true", &[]);
        dependent.service_type = ServiceType::Oneshot;
        dependent.remain_after_exit = true;
        dependent
            .dependencies
            .binds_to
            .insert("unstable.service".to_owned());
        supervisor.add(dependent).expect("add bound dependent");

        supervisor
            .start("bound.service")
            .expect("start bound dependent");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("bound.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("unstable.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        assert_eq!(
            supervisor
                .snapshot("bound.service")
                .expect("bound snapshot")
                .state,
            ServiceState::Exited
        );
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn starts_on_failure_recovery_units_once() {
        let mut supervisor = Supervisor::new();
        let mut failing = service("failing.service", "/bin/sh", &["-c", "exit 1"]);
        failing
            .dependencies
            .on_failure
            .insert("recovery.service".to_owned());
        supervisor.add(failing).expect("add failing service");

        let mut recovery = service("recovery.service", "/bin/true", &[]);
        recovery.service_type = ServiceType::Oneshot;
        recovery.remain_after_exit = true;
        supervisor.add(recovery).expect("add recovery service");

        supervisor
            .start("failing.service")
            .expect("start failing service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("failing.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("recovery.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        assert_eq!(
            supervisor
                .snapshot("recovery.service")
                .expect("recovery snapshot")
                .generation,
            1
        );
        supervisor.stop_all().expect("stop recovery");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn queues_a_failure_action_after_a_service_fails() {
        let mut supervisor = Supervisor::new();
        let mut failing = service("failure-action.service", "/bin/sh", &["-c", "exit 1"]);
        failing.failure_action = ManagerAction::Exit;
        supervisor.add(failing).expect("add failure action service");

        supervisor
            .start("failure-action.service")
            .expect("start failure action service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("failure-action.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        assert_eq!(supervisor.take_manager_action(), Some(ManagerAction::Exit));
        assert_eq!(supervisor.take_manager_action(), None);
    }

    #[test]
    fn queues_a_success_action_after_a_successful_oneshot() {
        let mut supervisor = Supervisor::new();
        let mut completing = service("success-action.service", "/bin/true", &[]);
        completing.service_type = ServiceType::Oneshot;
        completing.success_action = ManagerAction::RebootForce;
        supervisor
            .add(completing)
            .expect("add success action service");

        supervisor
            .start("success-action.service")
            .expect("start success action service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("success-action.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        assert_eq!(
            supervisor.take_manager_action(),
            Some(ManagerAction::RebootForce)
        );
    }

    #[test]
    fn times_out_a_pending_start_job_and_requests_its_action() {
        let mut supervisor = Supervisor::new();
        let mut dependency = service(
            "job-timeout-dependency.service",
            "/bin/sh",
            &["-c", "sleep 30"],
        );
        dependency.service_type = ServiceType::Notify;
        supervisor.add(dependency).expect("add dependency");

        let mut root = service("job-timeout-root.target", "/bin/true", &[]);
        root.dependencies
            .requires
            .insert("job-timeout-dependency.service".to_owned());
        root.job_timeout = Some(Duration::from_millis(20));
        root.job_timeout_action = ManagerAction::Exit;
        supervisor.add(root).expect("add root");

        supervisor
            .start("job-timeout-root.target")
            .expect("start timed out job");
        thread::sleep(Duration::from_millis(30));
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("job-timeout-root.target")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        assert_eq!(supervisor.take_manager_action(), Some(ManagerAction::Exit));
        supervisor.stop_all().expect("stop timeout dependency");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn starts_on_success_recovery_units_once() {
        let mut supervisor = Supervisor::new();
        let mut completing = service("completing.service", "/bin/true", &[]);
        completing.service_type = ServiceType::Oneshot;
        completing
            .dependencies
            .on_success
            .insert("cleanup.service".to_owned());
        supervisor.add(completing).expect("add completing service");

        let mut cleanup = service("cleanup.service", "/bin/true", &[]);
        cleanup.service_type = ServiceType::Oneshot;
        cleanup.remain_after_exit = true;
        supervisor.add(cleanup).expect("add cleanup service");

        supervisor
            .start("completing.service")
            .expect("start completing service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("cleanup.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        assert_eq!(
            supervisor
                .snapshot("completing.service")
                .expect("completing snapshot")
                .state,
            ServiceState::Exited
        );
        assert_eq!(
            supervisor
                .snapshot("cleanup.service")
                .expect("cleanup snapshot")
                .generation,
            1
        );
        supervisor.stop_all().expect("stop success recovery");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn restarts_a_failed_service_with_backoff() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("failing.service", "/bin/sh", &["-c", "exit 1"]);
        spec.restart = RestartPolicy::OnFailure;
        spec.restart_limit = 1;
        spec.restart_backoff = Duration::from_millis(1);
        supervisor.add(spec).expect("add service");
        supervisor.start("failing.service").expect("start service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("failing.service")
                .is_some_and(|snapshot| snapshot.generation >= 2)
        });
        supervisor.stop("failing.service").expect("stop service");
    }

    #[test]
    fn start_limit_stops_an_automatic_restart_loop() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("rate-limited.service", "/bin/sh", &["-c", "exit 1"]);
        spec.restart = RestartPolicy::OnFailure;
        spec.restart_limit = 100;
        spec.restart_backoff = Duration::from_millis(1);
        spec.start_limit_interval = Some(Duration::from_secs(30));
        spec.start_limit_burst = Some(2);
        supervisor.add(spec).expect("add rate-limited service");

        supervisor
            .start("rate-limited.service")
            .expect("start rate-limited service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("rate-limited.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        let snapshot = supervisor
            .snapshot("rate-limited.service")
            .expect("rate-limited snapshot");
        assert_eq!(snapshot.generation, 3);
        assert_eq!(snapshot.restart_count, 0);
        assert_eq!(snapshot.last_exit, Some(ExitReason::Exited(1)));
        assert!(supervisor.is_stopped());
    }

    #[test]
    fn runtime_max_sec_terminates_a_long_running_service() {
        let mut spec = service("runtime-limited.service", "/bin/sh", &["-c", "sleep 30"]);
        spec.runtime_max = Some(Duration::from_millis(20));
        spec.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add runtime limited service");
        supervisor
            .start("runtime-limited.service")
            .expect("start runtime limited service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("runtime-limited.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Failed && snapshot.pid.is_none()
                })
        });
        assert!(supervisor.is_stopped());
    }

    #[test]
    fn starts_dependencies_before_the_requested_service() {
        let mut supervisor = Supervisor::new();
        supervisor
            .add(service("database.service", "/bin/sh", &["-c", "sleep 30"]))
            .expect("add database");
        let mut app = service("app.service", "/bin/sh", &["-c", "sleep 30"]);
        app.dependencies = DependencySet::default();
        app.dependencies
            .requires
            .insert("database.service".to_owned());
        supervisor.add(app).expect("add app");

        supervisor.start("app.service").expect("start app");
        assert_eq!(
            supervisor
                .snapshot("database.service")
                .expect("database")
                .state,
            ServiceState::Running
        );
        assert_eq!(
            supervisor.snapshot("app.service").expect("app").state,
            ServiceState::Running
        );
        supervisor.stop("app.service").expect("stop app");
        poll_until(&mut supervisor, |supervisor| {
            supervisor.snapshot("app.service").is_some_and(|snapshot| {
                snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
            })
        });
        assert_eq!(
            supervisor
                .snapshot("database.service")
                .expect("database")
                .state,
            ServiceState::Running
        );
        supervisor.stop_all().expect("stop all");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn a_failed_wanted_service_does_not_block_its_root() {
        let mut supervisor = Supervisor::new();
        supervisor
            .add(service("optional.service", "/definitely/missing", &[]))
            .expect("add optional service");
        let mut root = service("root.service", "/bin/sh", &[]);
        root.args = vec!["-c".into(), "sleep 30".into()];
        root.dependencies
            .wants
            .insert("optional.service".to_owned());
        supervisor.add(root).expect("add root service");

        supervisor
            .start("root.service")
            .expect("wanted failure is non-fatal");
        assert_eq!(
            supervisor
                .snapshot("optional.service")
                .expect("optional")
                .state,
            ServiceState::Failed
        );
        assert_eq!(
            supervisor.snapshot("root.service").expect("root").state,
            ServiceState::Running
        );
        supervisor.stop_all().expect("stop services");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn keeps_a_successful_oneshot_active_when_requested() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("prepare.service", "/bin/sh", &["-c", "exit 0"]);
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        supervisor.add(spec).expect("add oneshot");

        supervisor.start("prepare.service").expect("start oneshot");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("prepare.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Active && snapshot.pid.is_none()
                })
        });
        supervisor.stop("prepare.service").expect("stop oneshot");
        assert_eq!(
            supervisor
                .snapshot("prepare.service")
                .expect("snapshot")
                .state,
            ServiceState::Exited
        );
    }

    #[test]
    fn manual_refusal_does_not_block_dependency_start_or_manager_shutdown() {
        let mut supervisor = Supervisor::new();
        let mut gated = service("gated.service", "/bin/sh", &["-c", "sleep 30"]);
        gated.refuse_manual_start = true;
        gated.refuse_manual_stop = true;
        supervisor.add(gated).expect("add gated service");
        assert!(supervisor.start("gated.service").is_err());
        assert_eq!(
            supervisor
                .snapshot("gated.service")
                .expect("gated snapshot")
                .state,
            ServiceState::Defined
        );

        let mut target = service("target.service", "/bin/sh", &["-c", "sleep 30"]);
        target.dependencies.wants.insert("gated.service".to_owned());
        supervisor.add(target).expect("add target service");
        supervisor.start("target.service").expect("start target");
        assert_eq!(
            supervisor
                .snapshot("gated.service")
                .expect("gated snapshot")
                .state,
            ServiceState::Running
        );
        assert!(supervisor.stop("gated.service").is_err());
        supervisor
            .stop_all()
            .expect("manager shutdown bypasses refusal");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn runs_start_and_stop_hooks_without_blocking_the_supervisor() {
        let marker = format!("/tmp/fractald-hooks-{}", std::process::id());
        let mut supervisor = Supervisor::new();
        let mut spec = service("hooks.service", "/bin/sh", &["-c", "sleep 30"]);
        let mut start_pre = fractald_core::CommandSpec::new("/bin/sh");
        start_pre.args = vec!["-c".into(), format!("printf pre > {marker}").into()];
        spec.start_pre = vec![start_pre];
        let mut start_post = fractald_core::CommandSpec::new("/bin/sh");
        start_post.args = vec!["-c".into(), format!("printf post >> {marker}").into()];
        spec.start_post = vec![start_post];
        let mut stop_post = fractald_core::CommandSpec::new("/bin/sh");
        stop_post.args = vec!["-c".into(), format!("printf clean >> {marker}").into()];
        spec.stop_post = vec![stop_post];
        supervisor.add(spec).expect("add service");

        supervisor.start("hooks.service").expect("start service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("hooks.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Running
                        && fs::read_to_string(&marker).ok().as_deref() == Some("prepost")
                })
        });
        supervisor.stop("hooks.service").expect("stop service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert_eq!(
            fs::read_to_string(&marker).expect("hook marker"),
            "prepostclean"
        );
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn failed_start_post_marks_the_service_failed() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("failed-post.service", "/bin/sh", &["-c", "sleep 30"]);
        let mut start_post = fractald_core::CommandSpec::new("/bin/sh");
        start_post.args = vec!["-c".into(), "exit 1".into()];
        spec.start_post = vec![start_post];
        supervisor.add(spec).expect("add service");

        supervisor
            .start("failed-post.service")
            .expect("start service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("failed-post.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Failed && snapshot.pid.is_none()
                })
        });
    }

    #[test]
    fn waits_for_a_oneshot_requirement_before_starting_its_dependent() {
        let marker = format!("/tmp/fractald-transaction-{}", std::process::id());
        let mut supervisor = Supervisor::new();
        let mut prerequisite = service(
            "prepare.service",
            "/bin/sh",
            &["-c", &format!("sleep 0.05; printf ready > {marker}")],
        );
        prerequisite.service_type = ServiceType::Oneshot;
        prerequisite.remain_after_exit = true;
        supervisor.add(prerequisite).expect("add prerequisite");

        let mut app = service(
            "app.service",
            "/bin/sh",
            &["-c", &format!("printf app >> {marker}; sleep 30")],
        );
        app.dependencies
            .requires
            .insert("prepare.service".to_owned());
        supervisor.add(app).expect("add app");

        supervisor.start("app.service").expect("start transaction");
        assert_eq!(
            supervisor
                .snapshot("app.service")
                .expect("app snapshot")
                .state,
            ServiceState::Defined
        );
        assert_eq!(
            supervisor
                .snapshot("prepare.service")
                .expect("prerequisite snapshot")
                .state,
            ServiceState::Running
        );

        poll_until(&mut supervisor, |supervisor| {
            supervisor.snapshot("app.service").is_some_and(|snapshot| {
                snapshot.state == ServiceState::Running
                    && fs::read_to_string(&marker).ok().as_deref() == Some("readyapp")
            })
        });
        supervisor.stop_all().expect("stop transaction");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn required_failure_rolls_back_services_started_by_the_transaction() {
        let mut supervisor = Supervisor::new();
        let mut prerequisite = service("failed-prepare.service", "/bin/sh", &["-c", "exit 1"]);
        prerequisite.service_type = ServiceType::Oneshot;
        supervisor.add(prerequisite).expect("add prerequisite");
        let mut app = service("dependent.service", "/bin/sh", &["-c", "sleep 30"]);
        app.dependencies
            .requires
            .insert("failed-prepare.service".to_owned());
        supervisor.add(app).expect("add dependent");

        supervisor
            .start("dependent.service")
            .expect("start transaction");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("failed-prepare.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        assert_eq!(
            supervisor
                .snapshot("dependent.service")
                .expect("dependent snapshot")
                .state,
            ServiceState::Defined
        );
    }

    #[test]
    fn loads_environment_files_before_expanding_command_arguments() {
        let environment_path = format!("/tmp/fractald-environment-{}.conf", std::process::id());
        let marker = format!("/tmp/fractald-environment-{}", std::process::id());
        fs::write(&environment_path, "FRACTALD_TEST_VALUE=from-file\n")
            .expect("write environment file");
        let mut spec = service(
            "environment.service",
            "/bin/sh",
            &["-c", &format!("printf '$FRACTALD_TEST_VALUE' > {marker}")],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.environment_files.push(EnvironmentFileSpec {
            path: environment_path.clone().into(),
            optional: false,
        });
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add environment service");
        supervisor
            .start("environment.service")
            .expect("start service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("environment.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("from-file")
                })
        });
        let _ = fs::remove_file(environment_path);
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn removes_unset_environment_from_the_service_process() {
        let marker = format!("/tmp/fractald-unset-environment-{}", std::process::id());
        let mut spec = service(
            "unset-environment.service",
            "/bin/sh",
            &[
                "-c",
                &format!("test -z \"$FRACTALD_UNSET_VALUE\" && printf ok > {marker}"),
            ],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.main_expand_environment = false;
        spec.environment.insert(
            OsString::from("FRACTALD_UNSET_VALUE"),
            OsString::from("present"),
        );
        spec.unset_environment
            .insert(OsString::from("FRACTALD_UNSET_VALUE"));
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add unset environment service");
        supervisor
            .start("unset-environment.service")
            .expect("start unset environment service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("unset-environment.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("ok")
                })
        });
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn adopts_the_daemon_child_of_a_forking_launcher() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("forking.service", "/bin/sh", &["-c", "sleep 30 & exit 0"]);
        spec.service_type = ServiceType::Forking;
        spec.start_timeout = Duration::from_secs(2);
        supervisor.add(spec).expect("add forking service");

        supervisor
            .start("forking.service")
            .expect("start forking service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("forking.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Running && snapshot.pid.is_some()
                })
        });
        supervisor
            .stop("forking.service")
            .expect("stop forking service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("forking.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });
    }

    #[test]
    fn notify_services_wait_for_ready_before_releasing_dependents() {
        let mut supervisor = Supervisor::new();
        let mut notify = service(
            "notify.service",
            "/usr/bin/python3",
            &[
                "-c",
                "import os, socket, time; socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).sendto(b'READY=1\\nSTATUS=ready\\n', os.environ['FRACTALD_NOTIFY_SOCKET']); time.sleep(30)",
            ],
        );
        notify.service_type = ServiceType::Notify;
        notify.start_timeout = Duration::from_secs(2);
        supervisor.add(notify).expect("add notify service");
        let mut app = service("after-notify.service", "/bin/sh", &["-c", "sleep 30"]);
        app.dependencies
            .requires
            .insert("notify.service".to_owned());
        supervisor.add(app).expect("add dependent");

        supervisor
            .start("after-notify.service")
            .expect("start notify transaction");
        assert_eq!(
            supervisor
                .snapshot("notify.service")
                .expect("notify snapshot")
                .state,
            ServiceState::Starting
        );
        assert_eq!(
            supervisor
                .snapshot("after-notify.service")
                .expect("dependent snapshot")
                .state,
            ServiceState::Defined
        );
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("after-notify.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Running)
        });
        supervisor.stop_all().expect("stop notify transaction");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn notify_access_main_rejects_a_foreign_sender() {
        let mut supervisor = Supervisor::new();
        let mut notify = service("notify-access.service", "/bin/sleep", &["30"]);
        notify.service_type = ServiceType::Notify;
        notify.start_timeout = Duration::from_secs(2);
        supervisor.add(notify).expect("add notify access service");

        supervisor
            .start("notify-access.service")
            .expect("start notify access service");
        let path = supervisor
            .services
            .get("notify-access.service")
            .and_then(|service| service.notify_path.clone())
            .expect("notify socket path");
        let client = UnixDatagram::unbound().expect("foreign notify client");
        client
            .send_to(b"READY=1\n", path)
            .expect("send foreign readiness");
        supervisor.poll().expect("poll foreign notification");
        assert_eq!(
            supervisor
                .snapshot("notify-access.service")
                .expect("notify access snapshot")
                .state,
            ServiceState::Starting
        );

        supervisor
            .stop("notify-access.service")
            .expect("stop notify access service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn watchdog_environment_and_expiry_are_enforced_for_notify_services() {
        let mut notify = service(
            "watchdog.service",
            "/usr/bin/python3",
            &[
                "-c",
                "import os, socket, time; assert os.environ['FRACTALD_WATCHDOG_USEC'] == '20000'; socket.socket(socket.AF_UNIX, socket.SOCK_DGRAM).sendto(b'READY=1\\n', os.environ['FRACTALD_NOTIFY_SOCKET']); time.sleep(30)",
            ],
        );
        notify.service_type = ServiceType::Notify;
        notify.watchdog = Some(Duration::from_millis(20));
        notify.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(notify).expect("add watchdog service");
        supervisor
            .start("watchdog.service")
            .expect("start watchdog service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("watchdog.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Running)
        });
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("watchdog.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Failed)
        });
        assert!(supervisor.is_stopped());
    }

    #[test]
    fn process_kill_mode_signals_only_the_main_process() {
        let marker = format!("/tmp/fractald-kill-mode-{}", std::process::id());
        let signal_marker = format!("/tmp/fractald-kill-mode-signal-{}", std::process::id());
        let mut spec = service(
            "process-kill-mode.service",
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "(/bin/sh -c \"trap 'printf signaled > {signal_marker}' TERM; while :; do sleep 1; done\") & child=$!; printf '%s' \"$child\" > {marker}; trap 'exit 0' TERM; wait"
                ),
            ],
        );
        spec.kill_mode = KillMode::Process;
        spec.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add process kill mode service");
        supervisor
            .start("process-kill-mode.service")
            .expect("start process kill mode service");
        poll_until(&mut supervisor, |_| Path::new(&marker).exists());
        let main_pid = supervisor
            .snapshot("process-kill-mode.service")
            .and_then(|snapshot| snapshot.pid)
            .expect("process kill mode main pid");

        supervisor
            .stop("process-kill-mode.service")
            .expect("stop process kill mode service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("process-kill-mode.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });
        thread::sleep(Duration::from_millis(50));
        assert!(!Path::new(&signal_marker).exists());
        let _ = fractald_platform::signal_process_group(main_pid, SIGKILL);
        let _ = fs::remove_file(marker);
        let _ = fs::remove_file(signal_marker);
    }

    #[test]
    fn private_tmp_directories_are_allocated_and_cleaned() {
        let mut spec = service("private-tmp.service", "/bin/true", &[]);
        spec.private_tmp = PrivateTmpMode::Yes;
        let paths = prepare_private_tmp(&spec)
            .expect("prepare private temporary directories")
            .expect("private temporary directories");
        assert!(paths.tmp.is_dir());
        assert!(paths.var_tmp.is_dir());
        assert_eq!(
            fs::metadata(&paths.tmp)
                .expect("tmp metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o1777
        );
        assert_eq!(
            fs::metadata(&paths.var_tmp)
                .expect("var tmp metadata")
                .permissions()
                .mode()
                & 0o7777,
            0o1777
        );
        let tmp = paths.tmp.clone();
        let var_tmp = paths.var_tmp.clone();
        paths.cleanup();
        assert!(!tmp.exists());
        assert!(!var_tmp.exists());
    }

    #[test]
    fn cgroup_slice_paths_expand_template_instances() {
        let mut spec = service("user@1000.svc", "/bin/true", &[]);
        spec.cgroup_slice = Some("user-%i.slice".to_owned());
        assert_eq!(
            service_cgroup_path(Path::new("/tmp/fractald-cgroup"), &spec).expect("cgroup path"),
            PathBuf::from("/tmp/fractald-cgroup/user.slice/user-1000.slice/user@1000.svc")
        );

        spec.cgroup_slice = Some("-.slice".to_owned());
        assert_eq!(
            service_cgroup_path(Path::new("/tmp/fractald-cgroup"), &spec).expect("root slice path"),
            PathBuf::from("/tmp/fractald-cgroup/user@1000.svc")
        );
    }

    #[test]
    fn reads_cgroup_oom_event_counter() {
        let root = PathBuf::from(format!("/tmp/fractald-oom-events-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("oom cgroup directory");
        fs::write(root.join("memory.events"), "low 0\noom 7\nome 2\n").expect("oom events");
        assert_eq!(read_cgroup_oom_events(&root), Some(7));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn writes_cpu_quota_in_cgroup_v2_format() {
        let root = PathBuf::from(format!("/tmp/fractald-cpu-quota-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("cpu quota cgroup directory");
        write_cgroup_cpu_quota(
            &root,
            CpuQuota {
                quota_usec: 313_750,
                period_usec: 250_000,
            },
        )
        .expect("cpu quota");
        assert_eq!(
            fs::read_to_string(root.join("cpu.max")).expect("cpu.max"),
            "313750 250000"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn enables_resource_controllers_from_the_cgroup_root_downward() {
        let root = PathBuf::from(format!(
            "/tmp/fractald-cgroup-controllers-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let target = root.join("fractald/app.slice/quota.service");
        fs::create_dir_all(&target).expect("cgroup tree");
        let ancestors = vec![
            root.clone(),
            root.join("fractald"),
            root.join("fractald/app.slice"),
        ];
        for ancestor in &ancestors {
            fs::write(ancestor.join("cgroup.controllers"), "cpu memory\n")
                .expect("available controllers");
            fs::write(ancestor.join("cgroup.subtree_control"), "").expect("subtree control");
        }
        let controllers = ["cpu"].into_iter().collect();
        enable_cgroup_controllers(&target, &root, &controllers).expect("enable controllers");
        for ancestor in ancestors {
            assert_eq!(
                fs::read_to_string(ancestor.join("cgroup.subtree_control"))
                    .expect("subtree control")
                    .trim(),
                "+cpu"
            );
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn false_path_conditions_skip_a_service_without_spawning_it() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("conditional.service", "/bin/sh", &["-c", "sleep 30"]);
        spec.conditions.push(Condition::PathExists {
            path: format!(
                "/tmp/fractald-path-that-does-not-exist-{}",
                std::process::id()
            )
            .into(),
            negate: false,
        });
        supervisor.add(spec).expect("add conditional service");
        supervisor
            .start("conditional.service")
            .expect("start conditional service");
        assert_eq!(
            supervisor
                .snapshot("conditional.service")
                .expect("conditional snapshot")
                .state,
            ServiceState::Skipped
        );
        assert!(supervisor.is_stopped());
    }

    #[test]
    fn a_preexisting_mount_satisfies_a_required_parent_after_condition_skip() {
        let mut parent = service("root.mount", "/bin/true", &[]);
        parent.service_type = ServiceType::Mount;
        parent.mount_where = Some(PathBuf::from("/"));
        parent.conditions.push(Condition::PathIsMountPoint {
            path: PathBuf::from("/"),
            negate: true,
        });

        let mut child = service("child.service", "/bin/true", &[]);
        child.service_type = ServiceType::Oneshot;
        child.remain_after_exit = true;
        child.dependencies.requires.insert("root.mount".to_owned());

        let mut supervisor = Supervisor::new();
        supervisor.add(parent).expect("add parent mount");
        supervisor.add(child).expect("add child service");
        supervisor
            .start("child.service")
            .expect("start child service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("child.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        assert_eq!(
            supervisor
                .snapshot("root.mount")
                .expect("parent mount snapshot")
                .state,
            ServiceState::Skipped
        );
    }

    #[test]
    fn an_external_mount_loss_marks_a_remain_after_exit_mount_inactive() {
        let mountpoint = PathBuf::from(format!(
            "/tmp/fractald-missing-mount-{}",
            std::process::id()
        ));
        let mut mount = service("lost.mount", "/bin/true", &[]);
        mount.service_type = ServiceType::Mount;
        mount.mount_where = Some(mountpoint.clone());
        mount.remain_after_exit = true;

        let mut supervisor = Supervisor::new();
        supervisor.add(mount).expect("add mount");
        supervisor.start("lost.mount").expect("start mount");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("lost.mount")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        assert!(!mountpoint.exists());
    }

    #[test]
    fn a_device_unit_loss_stops_bound_dependents() {
        let device_path = PathBuf::from(format!("/tmp/fractald-device-{}", std::process::id()));
        let _ = fs::remove_file(&device_path);

        let mut device = service("dev-test.device", "/bin/sh", &[]);
        device.args = vec![
            "-c".into(),
            "while [ ! -e \"$1\" ]; do sleep 0.01; done".into(),
            "fractald-device-wait".into(),
            device_path.as_os_str().to_owned(),
        ];
        device.service_type = ServiceType::Oneshot;
        device.remain_after_exit = true;
        device.default_dependencies = false;
        device.device_path = Some(device_path.clone());

        let mut dependent = service("device-dependent.service", "/bin/true", &[]);
        dependent.service_type = ServiceType::Oneshot;
        dependent.remain_after_exit = true;
        dependent
            .dependencies
            .binds_to
            .insert("dev-test.device".to_owned());

        let mut supervisor = Supervisor::new();
        supervisor.add(device).expect("device unit");
        supervisor.add(dependent).expect("dependent unit");
        supervisor
            .start("device-dependent.service")
            .expect("start dependent");
        for _ in 0..5 {
            supervisor.poll().expect("poll waiting device");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_ne!(
            supervisor
                .snapshot("device-dependent.service")
                .expect("dependent snapshot")
                .state,
            ServiceState::Active
        );

        fs::write(&device_path, b"present").expect("device marker");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("device-dependent.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });

        fs::remove_file(&device_path).expect("remove device marker");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("device-dependent.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        assert_eq!(
            supervisor
                .snapshot("dev-test.device")
                .expect("device snapshot")
                .state,
            ServiceState::Exited
        );
        poll_until(&mut supervisor, Supervisor::is_stopped);
    }

    #[test]
    fn false_assertions_fail_a_service_without_spawning_it() {
        let mut supervisor = Supervisor::new();
        let mut spec = service("asserted.service", "/bin/sh", &["-c", "sleep 30"]);
        spec.assertions.push(Condition::PathExists {
            path: format!(
                "/tmp/fractald-assertion-that-does-not-exist-{}",
                std::process::id()
            )
            .into(),
            negate: false,
        });
        supervisor.add(spec).expect("add asserted service");
        supervisor
            .start("asserted.service")
            .expect("start asserted service");
        let snapshot = supervisor
            .snapshot("asserted.service")
            .expect("asserted snapshot");
        assert_eq!(snapshot.state, ServiceState::Failed);
        assert!(snapshot.pid.is_none());
        assert!(supervisor.is_stopped());
    }

    #[test]
    fn evaluates_extended_host_conditions() {
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        );
        let directory = PathBuf::from(format!("/tmp/fractald-condition-{suffix}"));
        let file = directory.join("payload");
        let link = directory.join("link");
        fs::create_dir(&directory).expect("condition directory");
        fs::write(&file, "payload").expect("condition file");
        std::os::unix::fs::symlink(&file, &link).expect("condition symlink");
        let spec = service("conditions.service", "/bin/true", &[]);

        assert!(condition_matches(
            &Condition::PathIsDirectory {
                path: directory.clone(),
                negate: false,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::FileNotEmpty {
                path: file.clone(),
                negate: false,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::PathIsSymbolicLink {
                path: link,
                negate: false,
            },
            &spec
        ));
        let path_value = env::var("PATH").expect("PATH is set");
        assert!(condition_matches(
            &Condition::Environment {
                name: "PATH".to_owned(),
                value: None,
                negate: false,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::Environment {
                name: "PATH".to_owned(),
                value: Some(path_value),
                negate: false,
            },
            &spec
        ));
        assert!(!condition_matches(
            &Condition::Environment {
                name: "FRACTALD_TEST_ABSENT".to_owned(),
                value: None,
                negate: false,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::PathIsMountPoint {
                path: PathBuf::from("/"),
                negate: false,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::Any(vec![
                Condition::FileNotEmpty {
                    path: directory.join("missing"),
                    negate: false,
                },
                Condition::FileNotEmpty {
                    path: file.clone(),
                    negate: false,
                },
            ]),
            &spec
        ));
        assert!(condition_matches(
            &Condition::KernelCommandLine {
                argument: "fractald-condition-that-is-not-present".to_owned(),
                negate: true,
            },
            &spec
        ));
        assert!(condition_matches(
            &Condition::Security {
                value: "fractald-unknown-security".to_owned(),
                negate: true,
            },
            &spec
        ));

        let update_root = PathBuf::from(format!("/tmp/fractald-needs-update-{suffix}"));
        let update_usr = update_root.join("usr");
        let update_etc = update_root.join("etc");
        fs::create_dir_all(&update_usr).expect("update usr");
        fs::create_dir_all(&update_etc).expect("update etc");
        fs::write(update_etc.join(".updated"), b"").expect("update stamp");
        assert!(!needs_update_against(&update_etc, &update_usr));
        std::thread::sleep(Duration::from_millis(20));
        fs::write(update_usr.join("changed"), b"").expect("changed usr");
        assert!(needs_update_against(&update_etc, &update_usr));
        fs::remove_dir_all(update_root).expect("remove update directory");
        fs::remove_dir_all(directory).expect("remove condition directory");
    }

    #[test]
    fn passes_socket_activation_descriptors_to_the_service() {
        let socket_path = format!("/tmp/fractald-activation-{}.sock", std::process::id());
        let marker = format!("/tmp/fractald-activation-{}", std::process::id());
        let mut socket = ServiceSpec::new("example.socket", "/bin/true");
        socket.service_type = ServiceType::Socket;
        socket.remove_on_stop = true;
        socket.socket_service = Some("example.service".to_owned());
        socket.file_descriptor_name = Some("example-listener".to_owned());
        socket.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: socket_path.clone(),
        });

        let mut service = service(
            "example.service",
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "env | grep -qx 'LISTEN_FDS=1' && env | grep -qx 'LISTEN_FDNAMES=example-listener' && test -S /proc/self/fd/3 && test -S /proc/self/fd/0 && printf activated > {marker}; sleep 30"
                ),
            ],
        );
        service.standard_input = InputMode::Socket;
        service.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(socket).expect("add socket");
        supervisor.add(service).expect("add service");
        supervisor.start("example.socket").expect("start socket");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("example.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Running
                        && fs::read_to_string(&marker).ok().as_deref() == Some("activated")
                })
        });
        supervisor.stop_all().expect("stop socket service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert!(!Path::new(&socket_path).exists());
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn batches_multiple_socket_units_for_one_service() {
        let suffix = std::process::id();
        let first_path = format!("/tmp/fractald-multi-first-{suffix}.sock");
        let second_path = format!("/tmp/fractald-multi-second-{suffix}.sock");
        let marker = format!("/tmp/fractald-multi-activation-{suffix}");
        let mut first = ServiceSpec::new("multi-first.socket", "/bin/true");
        first.service_type = ServiceType::Socket;
        first.remove_on_stop = true;
        first.socket_service = Some("multi.service".to_owned());
        first.file_descriptor_name = Some("first".to_owned());
        first.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: first_path.clone(),
        });
        first
            .dependencies
            .wants
            .insert("multi-second.socket".to_owned());
        first
            .dependencies
            .after
            .insert("multi-second.socket".to_owned());

        let mut second = ServiceSpec::new("multi-second.socket", "/bin/true");
        second.service_type = ServiceType::Socket;
        second.remove_on_stop = true;
        second.socket_service = Some("multi.service".to_owned());
        second.file_descriptor_name = Some("second".to_owned());
        second.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: second_path.clone(),
        });

        let mut service = service(
            "multi.service",
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "env | grep -qx 'LISTEN_FDS=2' && env | grep -qx 'LISTEN_FDNAMES=first:second' && test -S /proc/self/fd/3 && test -S /proc/self/fd/4 && printf activated > {marker}; sleep 30"
                ),
            ],
        );
        service.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(first).expect("add first socket");
        supervisor.add(second).expect("add second socket");
        supervisor.add(service).expect("add multi service");
        supervisor
            .start("multi-first.socket")
            .expect("start first socket");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("multi.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Running
                        && fs::read_to_string(&marker).ok().as_deref() == Some("activated")
                })
        });
        supervisor.stop_all().expect("stop multi socket service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert!(!Path::new(&first_path).exists());
        assert!(!Path::new(&second_path).exists());
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn failed_socket_activation_does_not_reenter_start_progress() {
        let socket_path = format!(
            "/tmp/fractald-failed-activation-{}.sock",
            std::process::id()
        );
        let mut socket = ServiceSpec::new("failed.socket", "/bin/true");
        socket.service_type = ServiceType::Socket;
        socket.remove_on_stop = true;
        socket.socket_service = Some("failed.service".to_owned());
        socket.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: socket_path.clone(),
        });

        let mut service = service(
            "failed.service",
            "/definitely/missing-fractald-activation",
            &[],
        );
        service.service_type = ServiceType::Oneshot;
        service.working_directory = Some(PathBuf::from("/definitely/missing-fractald-directory"));
        service.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(socket).expect("add failed socket");
        supervisor.add(service).expect("add failed service");
        supervisor
            .start("failed.socket")
            .expect("start failed socket");
        assert_eq!(
            supervisor
                .snapshot("failed.service")
                .expect("failed service snapshot")
                .state,
            ServiceState::Failed
        );

        supervisor.stop_all().expect("stop failed socket");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert!(!Path::new(&socket_path).exists());
    }

    #[test]
    fn accept_socket_activation_connects_stdio_to_the_client() {
        let socket_path = format!("/tmp/fractald-accept-{}.sock", std::process::id());
        let mut socket = ServiceSpec::new("accept.socket", "/bin/true");
        socket.service_type = ServiceType::Socket;
        socket.remove_on_stop = true;
        socket.socket_accept = true;
        socket.socket_service = Some("accept.service".to_owned());
        socket.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: socket_path.clone(),
        });

        let mut service = service("accept.service", "/bin/sh", &["-c", "cat"]);
        service.standard_input = InputMode::Socket;
        service.stdout = OutputMode::Socket;
        service.start_timeout = Duration::from_secs(2);

        let mut supervisor = Supervisor::new();
        supervisor.add(socket).expect("add accepting socket");
        supervisor.add(service).expect("add accepting service");
        supervisor
            .start("accept.socket")
            .expect("start accepting socket");

        let mut client = UnixStream::connect(&socket_path).expect("connect accepting socket");
        client.write_all(b"hello\n").expect("write client request");
        client
            .shutdown(std::net::Shutdown::Write)
            .expect("close client write side");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("accept.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });
        let mut response = String::new();
        client
            .read_to_string(&mut response)
            .expect("read client response");
        assert_eq!(response, "hello\n");
        supervisor.stop_all().expect("stop accepting socket");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert!(!Path::new(&socket_path).exists());
    }

    #[test]
    fn preserves_socket_path_when_remove_on_stop_is_disabled() {
        let socket_path = format!("/tmp/fractald-preserved-{}.sock", std::process::id());
        let _ = fs::remove_file(&socket_path);
        let mut socket = ServiceSpec::new("preserved.socket", "/bin/true");
        socket.service_type = ServiceType::Socket;
        socket.listeners.push(ListenerSpec {
            kind: ListenerKind::Stream,
            address: socket_path.clone(),
        });

        let mut supervisor = Supervisor::new();
        supervisor.add(socket).expect("add preserved socket");
        supervisor
            .start("preserved.socket")
            .expect("start preserved socket");
        assert!(Path::new(&socket_path).exists());
        supervisor
            .stop("preserved.socket")
            .expect("stop preserved socket");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert!(Path::new(&socket_path).exists());
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn binds_abstract_unix_socket_listeners() {
        let name = format!("@fractald-abstract-{}", std::process::id());
        let listeners = open_listeners(
            &[ListenerSpec {
                kind: ListenerKind::Stream,
                address: name,
            }],
            0o600,
            None,
            None,
        )
        .expect("bind abstract unix listener");
        assert_eq!(listeners.len(), 1);
    }

    #[test]
    fn opens_fifo_seqpacket_netlink_and_special_listeners() {
        let fifo_path = format!("/tmp/fractald-fifo-{}", std::process::id());
        let seqpacket_path = format!("/tmp/fractald-seqpacket-{}.sock", std::process::id());
        let listeners = open_listeners(
            &[
                ListenerSpec {
                    kind: ListenerKind::Fifo,
                    address: fifo_path.clone(),
                },
                ListenerSpec {
                    kind: ListenerKind::SequentialPacket,
                    address: seqpacket_path.clone(),
                },
                ListenerSpec {
                    kind: ListenerKind::Netlink,
                    address: "route 0".to_owned(),
                },
                ListenerSpec {
                    kind: ListenerKind::Special,
                    address: "/dev/null".to_owned(),
                },
            ],
            0o600,
            None,
            None,
        )
        .expect("open extended listeners");
        assert_eq!(listeners.len(), 4);
        drop(listeners);
        assert!(!Path::new(&fifo_path).exists());
        assert!(!Path::new(&seqpacket_path).exists());
    }

    #[test]
    fn executes_stop_command_with_main_pid() {
        let marker = format!("/tmp/fractald-stop-command-{}", std::process::id());
        let mut supervisor = Supervisor::new();
        let mut spec = service("command-stop.service", "/bin/sh", &["-c", "sleep 30"]);
        let mut stop = fractald_core::CommandSpec::new("/bin/sh");
        stop.args = vec![
            "-c".into(),
            format!("printf '%s' \"$MAINPID\" > {marker}; kill -TERM \"$MAINPID\"").into(),
        ];
        spec.stop = Some(stop);
        supervisor.add(spec).expect("add service");
        supervisor
            .start("command-stop.service")
            .expect("start service");
        let pid = supervisor
            .snapshot("command-stop.service")
            .expect("snapshot")
            .pid
            .expect("pid");
        supervisor
            .stop("command-stop.service")
            .expect("stop service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("command-stop.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });
        assert_eq!(
            fs::read_to_string(&marker).expect("stop marker"),
            pid.to_string()
        );
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn sends_sighup_after_the_configured_stop_signal() {
        let marker = format!("/tmp/fractald-send-sighup-{}", std::process::id());
        let ready = format!("/tmp/fractald-send-sighup-ready-{}", std::process::id());
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(&ready);
        let mut supervisor = Supervisor::new();
        let script = format!(
            "trap 'printf term >> {marker}' USR1; trap 'printf hup >> {marker}; exit 0' HUP; printf ready > {ready}; while :; do sleep 30; done"
        );
        let mut spec = service("send-sighup.service", "/bin/sh", &["-c", &script]);
        spec.kill_signal = 10;
        spec.send_sighup = true;
        supervisor.add(spec).expect("add service");
        supervisor
            .start("send-sighup.service")
            .expect("start service");
        poll_until(&mut supervisor, |_| Path::new(&ready).exists());
        supervisor
            .stop("send-sighup.service")
            .expect("stop service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("send-sighup.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited && snapshot.pid.is_none()
                })
        });
        let marker_contents = fs::read_to_string(&marker).expect("signal marker");
        assert!(
            marker_contents.contains("hup"),
            "marker was {marker_contents:?}"
        );
        let _ = fs::remove_file(marker);
        let _ = fs::remove_file(ready);
    }

    #[test]
    fn uses_restart_kill_signal_for_an_explicit_restart() {
        let marker = format!("/tmp/fractald-restart-signal-{}", std::process::id());
        let ready = format!("/tmp/fractald-restart-signal-ready-{}", std::process::id());
        let _ = fs::remove_file(&marker);
        let _ = fs::remove_file(&ready);
        let mut supervisor = Supervisor::new();
        let script = format!(
            "trap 'printf normal >> {marker}; exit 0' USR1; trap 'printf restart >> {marker}; exit 0' USR2; printf ready > {ready}; while :; do sleep 30; done"
        );
        let mut spec = service("restart-signal.service", "/bin/sh", &["-c", &script]);
        spec.kill_signal = 10;
        spec.restart_kill_signal = Some(12);
        supervisor.add(spec).expect("add service");
        supervisor
            .start("restart-signal.service")
            .expect("start service");
        poll_until(&mut supervisor, |_| Path::new(&ready).exists());
        let _ = fs::remove_file(&ready);
        supervisor
            .restart("restart-signal.service")
            .expect("restart service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("restart-signal.service")
                .is_some_and(|snapshot| {
                    snapshot.generation >= 2
                        && snapshot.state == ServiceState::Running
                        && fs::read_to_string(&marker)
                            .ok()
                            .is_some_and(|value| value.starts_with("restart"))
                })
        });
        supervisor
            .stop("restart-signal.service")
            .expect("stop service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        let marker_contents = fs::read_to_string(&marker).expect("restart signal marker");
        assert!(marker_contents.starts_with("restart"));
        let _ = fs::remove_file(marker);
        let _ = fs::remove_file(ready);
    }

    #[test]
    fn executes_stop_command_for_a_remain_after_exit_service() {
        let marker = format!("/tmp/fractald-active-stop-{}", std::process::id());
        let mut supervisor = Supervisor::new();
        let mut spec = service("active-stop.service", "/bin/sh", &["-c", "exit 0"]);
        spec.service_type = ServiceType::Oneshot;
        spec.remain_after_exit = true;
        let mut stop = fractald_core::CommandSpec::new("/bin/sh");
        stop.args = vec!["-c".into(), format!("printf stopped > {marker}").into()];
        spec.stop = Some(stop);
        supervisor.add(spec).expect("add active service");
        supervisor
            .start("active-stop.service")
            .expect("start active service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("active-stop.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Active)
        });
        supervisor
            .stop("active-stop.service")
            .expect("stop active service");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        assert_eq!(
            fs::read_to_string(&marker).expect("active stop marker"),
            "stopped"
        );
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn applies_common_process_hardening_settings_before_exec() {
        let marker = format!("/tmp/fractald-process-settings-{}", std::process::id());
        let mut spec = service(
            "process-settings.service",
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "test \"$(umask)\" = 0027 && test \"$(ulimit -n)\" = 512 && test \"$(ulimit -r)\" = 0 && test \"$(cat /proc/self/oom_score_adj)\" = 200 && awk '$1 == \"Max\" && $2 == \"locked\" && $3 == \"memory\" {{exit ! ($4 == 65536 && $5 == 65536)}}' /proc/self/limits && awk '$1 == \"Max\" && $2 == \"processes\" {{exit ! ($3 == 4096 && $4 == 4096)}}' /proc/self/limits && grep -Eq '^NoNewPrivs:[[:space:]]+1$' /proc/self/status && printf ok > {marker}"
                ),
            ],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.no_new_privileges = true;
        spec.memory_deny_write_execute = true;
        spec.restrict_realtime = true;
        spec.umask = Some(0o027);
        spec.nice = Some(5);
        spec.oom_score_adjust = Some(200);
        spec.nofile = Some(LimitRange {
            soft: LimitValue::Value(512),
            hard: LimitValue::Value(512),
        });
        spec.memlock = Some(LimitRange {
            soft: LimitValue::Value(65_536),
            hard: LimitValue::Value(65_536),
        });
        spec.nproc = Some(LimitRange {
            soft: LimitValue::Value(4096),
            hard: LimitValue::Value(4096),
        });

        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add process settings service");
        supervisor
            .start("process-settings.service")
            .expect("start process settings service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("process-settings.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("ok")
                })
        });
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn installs_address_family_restrictions_before_exec() {
        let mut spec = service("address-families.service", "/bin/true", &[]);
        spec.service_type = ServiceType::Oneshot;
        spec.restrict_address_families = Some(1_u64 << 1);
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add address family service");
        supervisor
            .start("address-families.service")
            .expect("start address family service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("address-families.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
    }

    #[test]
    fn forwards_service_stdout_and_stderr_to_configured_files() {
        let stdout = format!("/tmp/fractald-stdout-{}", std::process::id());
        let stderr = format!("/tmp/fractald-stderr-{}", std::process::id());
        let mut spec = service(
            "output.service",
            "/bin/sh",
            &["-c", "printf stdout; printf stderr >&2"],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.stdout = OutputMode::File {
            path: stdout.clone().into(),
            append: true,
        };
        spec.stderr = OutputMode::File {
            path: stderr.clone().into(),
            append: true,
        };

        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add output service");
        supervisor
            .start("output.service")
            .expect("start output service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("output.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&stdout).ok().as_deref() == Some("stdout")
                        && fs::read_to_string(&stderr).ok().as_deref() == Some("stderr")
                })
        });
        let _ = fs::remove_file(stdout);
        let _ = fs::remove_file(stderr);
    }

    #[test]
    fn creates_log_files_through_dangling_directory_symlinks() {
        let root = PathBuf::from(format!(
            "/tmp/fractald-log-symlink-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("var/volatile")).expect("volatile directory");
        std::os::unix::fs::symlink("volatile/log", root.join("var/log"))
            .expect("volatile log symlink");

        let path = root.join("var/log/fractald/symlink.service.stdout.log");
        let file = open_log_file(&path, true).expect("open log through symlink");
        drop(file);
        assert!(root.join("var/volatile/log/fractald").is_dir());
        assert!(path.is_file());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn formats_native_journal_entries_with_service_identity() {
        let payload = fractald_journal::encode_entry(
            "journal-smoke.svc",
            "stderr",
            4242,
            b"message with nul\0 and newline\n",
        );
        let payload = String::from_utf8(payload).expect("journal payload");
        assert!(payload.contains("MESSAGE=message with nul and newline"));
        assert!(payload.contains("FRACTALD_SERVICE=journal-smoke.svc"));
        assert!(payload.contains("_PID=4242"));
        assert!(payload.contains("STREAM=stderr"));
        assert!(payload.ends_with("_TRANSPORT=stdout\n"));
    }

    #[test]
    fn forwards_configured_file_input_to_the_service() {
        let suffix = std::process::id();
        let input = format!("/tmp/fractald-stdin-{suffix}");
        let output = format!("/tmp/fractald-stdin-output-{suffix}");
        fs::write(&input, "input payload").expect("write input payload");
        let mut spec = service("input.service", "/bin/cat", &[]);
        spec.service_type = ServiceType::Oneshot;
        spec.standard_input = InputMode::File(input.clone().into());
        spec.stdout = OutputMode::File {
            path: output.clone().into(),
            append: false,
        };

        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add input service");
        supervisor
            .start("input.service")
            .expect("start input service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("input.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&output).ok().as_deref() == Some("input payload")
                })
        });
        let _ = fs::remove_file(input);
        let _ = fs::remove_file(output);
    }

    #[test]
    fn runs_exec_condition_before_starting_the_service() {
        let marker = format!("/tmp/fractald-exec-condition-{}", std::process::id());
        fs::write(&marker, "ready").expect("condition marker");
        let service_name = "exec-condition.service";
        let mut spec = service(service_name, "/bin/sh", &["-c", "exit 0"]);
        spec.service_type = ServiceType::Oneshot;
        let mut condition = fractald_core::CommandSpec::new("/bin/sh");
        condition.args = vec!["-c".into(), format!("test -f {marker}").into()];
        spec.exec_conditions = vec![condition];
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add conditional service");
        supervisor.start(service_name).expect("start service");
        assert_eq!(
            supervisor.snapshot(service_name).expect("snapshot").state,
            ServiceState::Starting
        );
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot(service_name)
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn an_exec_condition_exit_one_skips_the_service() {
        let mut spec = service(
            "skipped-exec-condition.service",
            "/bin/sh",
            &["-c", "exit 99"],
        );
        spec.service_type = ServiceType::Oneshot;
        let mut condition = fractald_core::CommandSpec::new("/bin/false");
        condition.args = Vec::new();
        spec.exec_conditions = vec![condition];
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add skipped service");
        supervisor
            .start("skipped-exec-condition.service")
            .expect("start service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("skipped-exec-condition.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Skipped && snapshot.pid.is_none()
                })
        });
    }

    #[test]
    fn timer_activation_starts_its_associated_service() {
        let marker = format!("/tmp/fractald-timer-{}", std::process::id());
        let mut target = service(
            "timer-target.service",
            "/bin/sh",
            &["-c", &format!("printf fired > {marker}")],
        );
        target.service_type = ServiceType::Oneshot;
        let mut timer = service("timer-target.timer", "/bin/true", &[]);
        timer.service_type = ServiceType::Timer;
        timer.remain_after_exit = true;
        timer.trigger = Some(TriggerSpec::Timer {
            service: "timer-target.service".to_owned(),
            on_boot: Some(Duration::from_millis(20)),
            on_unit_active: None,
            on_unit_inactive: None,
            on_calendar: Vec::new(),
            persistent: false,
            randomized_delay: None,
            accuracy: None,
        });
        let mut supervisor = Supervisor::new();
        supervisor.add(target).expect("add timer target");
        supervisor.add(timer).expect("add timer");
        supervisor.start("timer-target.timer").expect("start timer");
        assert_eq!(
            supervisor
                .snapshot("timer-target.timer")
                .expect("timer snapshot")
                .state,
            ServiceState::Active
        );
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("timer-target.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("fired")
                })
        });
        supervisor.stop_all().expect("stop timer");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn path_activation_starts_when_a_watched_path_appears() {
        let suffix = std::process::id();
        let watched = format!("/tmp/fractald-path-watch-{suffix}");
        let marker = format!("/tmp/fractald-path-target-{suffix}");
        let mut target = service(
            "path-target.service",
            "/bin/sh",
            &["-c", &format!("printf fired > {marker}")],
        );
        target.service_type = ServiceType::Oneshot;
        let mut path = service("path-target.path", "/bin/true", &[]);
        path.service_type = ServiceType::Path;
        path.remain_after_exit = true;
        path.trigger = Some(TriggerSpec::Path {
            service: "path-target.service".to_owned(),
            watches: vec![PathWatch::Exists(watched.clone().into())],
        });
        let mut supervisor = Supervisor::new();
        supervisor.add(target).expect("add path target");
        supervisor.add(path).expect("add path unit");
        let _ = fs::remove_file(&watched);
        supervisor
            .start("path-target.path")
            .expect("start path unit");
        fs::write(&watched, "ready").expect("watched path");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("path-target.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("fired")
                })
        });
        supervisor.stop_all().expect("stop path unit");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        let _ = fs::remove_file(watched);
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn path_glob_activation_starts_when_a_matching_path_appears() {
        let suffix = std::process::id();
        let directory = format!("/tmp/fractald-path-glob-{suffix}");
        let watched = format!("{directory}/ready.ready");
        let marker = format!("/tmp/fractald-path-glob-target-{suffix}");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).expect("glob directory");
        let mut target = service(
            "path-glob-target.service",
            "/bin/sh",
            &["-c", &format!("printf fired > {marker}")],
        );
        target.service_type = ServiceType::Oneshot;
        let mut path = service("path-glob-target.path", "/bin/true", &[]);
        path.service_type = ServiceType::Path;
        path.remain_after_exit = true;
        path.trigger = Some(TriggerSpec::Path {
            service: "path-glob-target.service".to_owned(),
            watches: vec![PathWatch::ExistsGlob(format!("{directory}/*.ready").into())],
        });
        let mut supervisor = Supervisor::new();
        supervisor.add(target).expect("add glob target");
        supervisor.add(path).expect("add glob path unit");
        supervisor
            .start("path-glob-target.path")
            .expect("start glob path");
        fs::write(&watched, "ready").expect("matching path");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("path-glob-target.service")
                .is_some_and(|snapshot| {
                    snapshot.state == ServiceState::Exited
                        && fs::read_to_string(&marker).ok().as_deref() == Some("fired")
                })
        });
        supervisor.stop_all().expect("stop glob path");
        poll_until(&mut supervisor, Supervisor::is_stopped);
        let _ = fs::remove_file(watched);
        let _ = fs::remove_file(marker);
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn service_directories_are_created_and_exported_to_the_child() {
        let directory_name = format!("fractald-test-{}", std::process::id());
        let root = directory_root(DirectoryKind::Logs).expect("logs root");
        let directory = root.join(&directory_name);
        let _ = fs::remove_dir(&directory);
        let mut spec = service(
            "directory.service",
            "/bin/sh",
            &[
                "-c",
                "test -d \"$LOGS_DIRECTORY\" && test \"$LOGS_DIRECTORY\" = \"$(dirname \"$LOGS_DIRECTORY\")/$(basename \"$LOGS_DIRECTORY\")\"",
            ],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.directories.push(DirectorySpec {
            kind: DirectoryKind::Logs,
            path: directory_name.into(),
            mode: 0o750,
            preserve: false,
        });
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add directory service");
        supervisor
            .start("directory.service")
            .expect("start directory service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot("directory.service")
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        assert!(directory.is_dir());
        let _ = fs::remove_dir(directory);
    }

    #[test]
    fn credentials_are_materialized_and_exported_to_the_child() {
        let suffix = std::process::id();
        let source = std::env::temp_dir().join(format!("fractald-credential-source-{suffix}"));
        let marker = std::env::temp_dir().join(format!("fractald-credential-marker-{suffix}"));
        fs::write(&source, "source-value\n").expect("credential source");
        let name = format!("credentials-{suffix}.service");
        let mut spec = service(
            &name,
            "/bin/sh",
            &[
                "-c",
                &format!(
                    "test -d \"$CREDENTIALS_DIRECTORY\" && test \"$(cat \"$CREDENTIALS_DIRECTORY/inline\")\" = fixture-value && test \"$(cat \"$CREDENTIALS_DIRECTORY/from-file\")\" = source-value && test \"$(stat -c %a \"$CREDENTIALS_DIRECTORY/inline\")\" = 400 && printf ok > {}",
                    marker.display()
                ),
            ],
        );
        spec.service_type = ServiceType::Oneshot;
        spec.credentials = vec![
            fractald_core::CredentialSpec {
                name: "inline".to_owned(),
                source: CredentialSource::Value(b"fixture-value".to_vec()),
            },
            fractald_core::CredentialSpec {
                name: "from-file".to_owned(),
                source: CredentialSource::File(source.clone()),
            },
        ];
        let credential_directory = credential_directory_path(&spec).expect("credential path");
        let _ = fs::remove_dir_all(&credential_directory);
        let mut supervisor = Supervisor::new();
        supervisor.add(spec).expect("add credential service");
        supervisor.start(&name).expect("start credential service");
        poll_until(&mut supervisor, |supervisor| {
            supervisor
                .snapshot(&name)
                .is_some_and(|snapshot| snapshot.state == ServiceState::Exited)
        });
        assert_eq!(
            fs::read_to_string(&marker).expect("credential marker"),
            "ok"
        );
        assert!(!credential_directory.exists());
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(marker);
    }

    #[test]
    fn imports_credential_store_entries_with_precedence_and_rename() {
        let suffix = std::process::id();
        let root = std::env::temp_dir().join(format!("fractald-credential-import-{suffix}"));
        let first_store = root.join("first");
        let second_store = root.join("second");
        let explicit = root.join("explicit");
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&first_store).expect("first credential store");
        fs::create_dir_all(&second_store).expect("second credential store");
        fs::write(first_store.join("app.one"), b"first-store").expect("first app.one");
        fs::write(second_store.join("app.one"), b"second-store").expect("second app.one");
        fs::write(first_store.join("app.two"), b"first-two").expect("first app.two");
        fs::write(second_store.join("app.two"), b"second-two").expect("second app.two");
        fs::write(second_store.join("app.three"), b"second-three").expect("second app.three");
        fs::write(first_store.join("store-token"), b"store-value").expect("store token");
        fs::write(&explicit, b"explicit-one").expect("explicit credential");

        let name = format!("credential-import-{suffix}.service");
        let mut spec = service(&name, "/bin/true", &[]);
        spec.credentials = vec![
            fractald_core::CredentialSpec {
                name: "app.one".to_owned(),
                source: CredentialSource::File(explicit.clone()),
            },
            fractald_core::CredentialSpec {
                name: "fallback".to_owned(),
                source: CredentialSource::Value(b"default".to_vec()),
            },
            fractald_core::CredentialSpec {
                name: "from-store".to_owned(),
                source: CredentialSource::Store("store-token".to_owned()),
            },
        ];
        spec.credential_imports = vec![
            fractald_core::CredentialImportSpec {
                pattern: "app.one".to_owned(),
                rename: None,
            },
            fractald_core::CredentialImportSpec {
                pattern: "app.*".to_owned(),
                rename: Some("imported.".to_owned()),
            },
            fractald_core::CredentialImportSpec {
                pattern: "app.three".to_owned(),
                rename: Some("renamed".to_owned()),
            },
        ];

        let directory = credential_directory_path(&spec).expect("credential path");
        fs::remove_dir_all(&directory).ok();
        let stores = vec![first_store.clone(), second_store.clone()];
        let directory = prepare_credentials_with_import_dirs(&spec, &stores)
            .expect("prepare imported credentials")
            .expect("credential directory");
        assert_eq!(
            fs::read(directory.join("app.one")).expect("explicit value"),
            b"explicit-one"
        );
        assert_eq!(
            fs::read(directory.join("fallback")).expect("set value"),
            b"default"
        );
        assert_eq!(
            fs::read(directory.join("from-store")).expect("store value"),
            b"store-value"
        );
        assert_eq!(
            fs::read(directory.join("imported.one")).expect("renamed app.one"),
            b"first-store"
        );
        assert_eq!(
            fs::read(directory.join("imported.two")).expect("renamed app.two"),
            b"first-two"
        );
        assert_eq!(
            fs::read(directory.join("imported.three")).expect("renamed app.three"),
            b"second-three"
        );
        assert_eq!(
            fs::read(directory.join("renamed")).expect("exact rename"),
            b"second-three"
        );

        fs::remove_dir_all(&directory).expect("remove credential directory");
        fs::remove_dir_all(root).expect("remove credential stores");
    }

    #[test]
    fn matches_common_calendar_aliases_and_fields() {
        let epoch = UNIX_EPOCH;
        assert!(calendar_match("hourly", epoch + Duration::from_secs(3_600)).is_some());
        assert!(
            calendar_match(
                "Sun *-*-1..7 01:00:00",
                epoch + Duration::from_secs(259_200 + 3_600)
            )
            .is_some()
        );
        assert!(calendar_match("*:0/15", epoch + Duration::from_secs(900)).is_some());
        assert!(calendar_match("daily", epoch + Duration::from_secs(86_400)).is_some());
        assert!(calendar_match("hourly", epoch + Duration::from_secs(3_601)).is_none());
    }

    #[test]
    fn detects_missed_persistent_calendar_windows() {
        assert!(calendar_missed_since(
            "hourly",
            UNIX_EPOCH + Duration::from_secs(1),
            UNIX_EPOCH + Duration::from_secs(7_201),
        ));
        assert!(!calendar_missed_since(
            "hourly",
            UNIX_EPOCH + Duration::from_secs(3_601),
            UNIX_EPOCH + Duration::from_secs(3_650),
        ));
    }

    #[test]
    fn randomized_timer_delay_stays_within_its_configured_window() {
        let spec = service("randomized.timer", "/bin/true", &[]);
        let maximum = Duration::from_secs(10);
        assert!(timer_random_delay(&spec, maximum) <= maximum);
    }

    #[test]
    fn timer_accuracy_delay_stays_within_its_configured_window() {
        let spec = service("accurate.timer", "/bin/true", &[]);
        let maximum = Duration::from_secs(10);
        assert!(timer_random_delay(&spec, maximum) <= maximum);
    }

    #[test]
    fn toolbox_selection_is_limited_to_native_mount_and_swap_helpers() {
        let binary = Path::new("/bin/true");
        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Mount,
            Some("ext4"),
            None,
            Some(binary),
        );
        assert_eq!(selected, binary);
        assert_eq!(argv0, Some(OsString::from("mount")));

        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Mount,
            Some("XFS"),
            None,
            Some(binary),
        );
        assert_eq!(selected, binary);
        assert_eq!(argv0, Some(OsString::from("mount")));

        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Mount,
            Some("ext"),
            None,
            Some(binary),
        );
        assert_eq!(selected, binary);
        assert_eq!(argv0, Some(OsString::from("mount")));

        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Simple,
            Some("ext4"),
            None,
            Some(binary),
        );
        assert_eq!(selected, PathBuf::from("mount"));
        assert_eq!(argv0, None);

        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Mount,
            Some("sshfs"),
            None,
            Some(binary),
        );
        assert_eq!(selected, PathBuf::from("mount"));
        assert_eq!(argv0, None);

        let (selected, argv0) = toolbox_selection(
            Path::new("mount"),
            ServiceType::Mount,
            Some("fuse.sshfs"),
            None,
            Some(binary),
        );
        assert_eq!(selected, PathBuf::from("mount"));
        assert_eq!(argv0, None);
    }

    #[test]
    fn kernel_mount_selection_follows_loaded_filesystems_and_excludes_helpers() {
        assert!(kernel_mount_filesystem(Some("proc")));
        assert!(!kernel_mount_filesystem(Some("sshfs")));
        assert!(!kernel_mount_filesystem(Some("fuse.sshfs")));
        assert!(!kernel_mount_filesystem(Some("nfs4")));
        assert!(kernel_mount_filesystem(Some("FAT")));
    }
}
