use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFilterRule {
    pub device_type: u32,
    pub major: u32,
    pub minor: u32,
    pub access: u32,
}

unsafe extern "C" {
    fn fractald_pidfd_open(pid: u32) -> i32;
    fn fractald_pidfd_send_signal(pidfd: i32, signal_number: i32) -> i32;
    fn fractald_pidfd_wait(pidfd: i32, nohang: i32, code: *mut i32, value: *mut i32) -> i32;
    fn fractald_set_process_group() -> i32;
    fn fractald_set_parent_death_signal(signal_number: i32, expected_parent: u32) -> i32;
    fn fractald_set_signal_disposition(signal_number: i32, ignored: i32) -> i32;
    fn fractald_set_no_new_privileges() -> i32;
    fn fractald_set_memory_deny_write_execute() -> i32;
    fn fractald_restrict_realtime() -> i32;
    fn fractald_restrict_suid_sgid() -> i32;
    fn fractald_restrict_namespaces(allowed: u32) -> i32;
    fn fractald_set_umask(mask: u32) -> i32;
    fn fractald_set_nice(value: i32) -> i32;
    fn fractald_set_oom_score_adjust(value: i32) -> i32;
    fn fractald_set_nofile_limit(soft: u64, hard: u64) -> i32;
    fn fractald_set_memlock_limit(soft: u64, hard: u64) -> i32;
    fn fractald_set_nproc_limit(soft: u64, hard: u64) -> i32;
    fn fractald_enter_private_tmp(mode: i32, tmp_path: *const i8, var_tmp_path: *const i8) -> i32;
    fn fractald_enter_private_devices() -> i32;
    fn fractald_enter_private_users(
        mode: i32,
        user: *const i8,
        group: *const i8,
        supplementary_groups: *const *const i8,
        supplementary_group_count: usize,
    ) -> i32;
    fn fractald_attach_device_filter(
        cgroup_path: *const i8,
        default_allow: i32,
        rules: *const DeviceFilterRule,
        rule_count: usize,
    ) -> i32;
    fn fractald_detach_device_filter(cgroup_path: *const i8) -> i32;
    fn fractald_enter_private_ipc() -> i32;
    fn fractald_enter_private_network() -> i32;
    fn fractald_enter_private_uts_namespace() -> i32;
    fn fractald_install_hostname_filter() -> i32;
    fn fractald_enter_private_uts() -> i32;
    fn fractald_lock_personality() -> i32;
    fn fractald_protect_clock() -> i32;
    fn fractald_apply_filesystem_protection(
        protect_system: i32,
        protect_home: i32,
        protect_proc: i32,
        proc_subset: i32,
    ) -> i32;
    fn fractald_make_path_writable(path: *const i8) -> i32;
    fn fractald_make_path_read_only(path: *const i8) -> i32;
    fn fractald_make_path_inaccessible(path: *const i8) -> i32;
    fn fractald_signal_process(pid: u32, signal_number: i32) -> i32;
    fn fractald_signal_process_group(pid: u32, signal_number: i32) -> i32;
    fn fractald_set_identity(
        user: *const i8,
        group: *const i8,
        supplementary_groups: *const *const i8,
        supplementary_group_count: usize,
    ) -> i32;
    fn fractald_set_keep_capabilities() -> i32;
    fn fractald_drop_capability_bounding_set(allowed: u64) -> i32;
    fn fractald_apply_capability_bounding_set(allowed: u64) -> i32;
    fn fractald_apply_ambient_capabilities(allowed: u64) -> i32;
    fn fractald_restrict_address_families(allowed: u64) -> i32;
    fn fractald_install_system_call_filter(
        names: *const *const i8,
        actions: *const i32,
        count: usize,
        default_allow: i32,
        default_errno: i32,
        architecture: i32,
    ) -> i32;
    fn fractald_chown_path(path: *const i8, user: *const i8, group: *const i8) -> i32;
    fn fractald_fd_close(fd: i32) -> i32;
    fn fractald_effective_uid() -> u32;
    fn fractald_prepare_activation_fds(sources: *const i32, count: usize) -> i32;
    fn fractald_dup_fd(fd: i32) -> i32;
    fn fractald_open_fifo(path: *const i8, mode: u32) -> i32;
    fn fractald_open_special(path: *const i8, writable: i32) -> i32;
    fn fractald_bind_unix_seqpacket(address: *const i8) -> i32;
    fn fractald_send_unix_datagram(
        address: *const i8,
        payload: *const u8,
        payload_length: usize,
    ) -> i32;
    fn fractald_enable_socket_credentials(fd: i32) -> i32;
    fn fractald_receive_socket_credentials(
        fd: i32,
        payload: *mut u8,
        payload_length: usize,
        sender_pid: *mut u32,
    ) -> isize;
    fn fractald_open_netlink(protocol: i32, groups: u32) -> i32;
    fn fractald_accept_fd(fd: i32) -> i32;
    fn fractald_set_activation_stdin() -> i32;
    fn fractald_install_shutdown_handlers() -> i32;
    fn fractald_shutdown_requested() -> i32;
    fn fractald_power_action(action: i32) -> i32;
    fn fractald_set_child_subreaper() -> i32;
    fn fractald_reap_untracked_children(managed_pids: *const u32, managed_count: usize) -> i32;
}

const CLD_EXITED: i32 = 1;
const CLD_KILLED: i32 = 2;
const CLD_DUMPED: i32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Exited(i32),
    Signaled(i32),
    CoreDumped(i32),
}

#[derive(Debug)]
pub struct PidFd {
    raw_fd: RawFd,
}

impl PidFd {
    pub fn open(pid: u32) -> io::Result<Self> {
        let raw_fd = unsafe { fractald_pidfd_open(pid) };
        if raw_fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { raw_fd })
    }

    pub fn send_signal(&self, signal_number: i32) -> io::Result<()> {
        let result = unsafe { fractald_pidfd_send_signal(self.raw_fd, signal_number) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn try_wait(&self) -> io::Result<Option<ExitKind>> {
        self.wait_inner(true)
    }

    pub fn wait(&self) -> io::Result<ExitKind> {
        self.wait_inner(false)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "pidfd wait returned no exit")
        })
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.raw_fd
    }

    fn wait_inner(&self, nohang: bool) -> io::Result<Option<ExitKind>> {
        let mut code = 0;
        let mut value = 0;
        let result =
            unsafe { fractald_pidfd_wait(self.raw_fd, i32::from(nohang), &mut code, &mut value) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if result == 0 {
            return Ok(None);
        }
        let exit = match code {
            CLD_EXITED => ExitKind::Exited(value),
            CLD_KILLED => ExitKind::Signaled(value),
            CLD_DUMPED => ExitKind::CoreDumped(value),
            unknown => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unknown waitid code {unknown}"),
                ));
            }
        };
        Ok(Some(exit))
    }
}

pub fn effective_uid() -> u32 {
    unsafe { fractald_effective_uid() }
}

pub fn is_root() -> bool {
    effective_uid() == 0
}

pub fn prepare_activation_fds(sources: &[RawFd]) -> io::Result<()> {
    let result = unsafe { fractald_prepare_activation_fds(sources.as_ptr(), sources.len()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn duplicate_fd(fd: RawFd) -> io::Result<RawFd> {
    let result = unsafe { fractald_dup_fd(fd) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn close_fd(fd: RawFd) -> io::Result<()> {
    let result = unsafe { fractald_fd_close(fd) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn open_fifo(path: &std::path::Path, mode: u32) -> io::Result<RawFd> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "FIFO path contains NUL"))?;
    let result = unsafe { fractald_open_fifo(path.as_ptr(), mode) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn open_special(path: &std::path::Path, writable: bool) -> io::Result<RawFd> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "special path contains NUL"))?;
    let result = unsafe { fractald_open_special(path.as_ptr(), i32::from(writable)) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn bind_unix_seqpacket(address: &str) -> io::Result<RawFd> {
    let address = std::ffi::CString::new(address)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket address contains NUL"))?;
    let result = unsafe { fractald_bind_unix_seqpacket(address.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn open_netlink(protocol: i32, groups: u32) -> io::Result<RawFd> {
    let result = unsafe { fractald_open_netlink(protocol, groups) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn accept_fd(fd: RawFd) -> io::Result<RawFd> {
    let result = unsafe { fractald_accept_fd(fd) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result)
    }
}

pub fn set_activation_stdin() -> io::Result<()> {
    let result = unsafe { fractald_set_activation_stdin() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn install_shutdown_handlers() -> io::Result<()> {
    let result = unsafe { fractald_install_shutdown_handlers() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn shutdown_requested() -> bool {
    unsafe { fractald_shutdown_requested() != 0 }
}

pub fn set_child_subreaper() -> io::Result<()> {
    let result = unsafe { fractald_set_child_subreaper() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn reap_untracked_children(managed_pids: &[u32]) -> io::Result<usize> {
    let result =
        unsafe { fractald_reap_untracked_children(managed_pids.as_ptr(), managed_pids.len()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(result as usize)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    Poweroff,
    Reboot,
    Halt,
}

pub fn power_action(action: PowerAction) -> io::Result<()> {
    let action = match action {
        PowerAction::Poweroff => 0,
        PowerAction::Reboot => 1,
        PowerAction::Halt => 2,
    };
    let result = unsafe { fractald_power_action(action) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_process_group() -> io::Result<()> {
    let result = unsafe { fractald_set_process_group() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_parent_death_signal(signal_number: i32, expected_parent: u32) -> io::Result<()> {
    let result = unsafe { fractald_set_parent_death_signal(signal_number, expected_parent) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_signal_disposition(signal_number: i32, ignored: bool) -> io::Result<()> {
    let result = unsafe { fractald_set_signal_disposition(signal_number, i32::from(ignored)) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_no_new_privileges() -> io::Result<()> {
    let result = unsafe { fractald_set_no_new_privileges() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_memory_deny_write_execute() -> io::Result<()> {
    let result = unsafe { fractald_set_memory_deny_write_execute() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn restrict_realtime() -> io::Result<()> {
    let result = unsafe { fractald_restrict_realtime() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_umask(mask: u32) -> io::Result<()> {
    let result = unsafe { fractald_set_umask(mask) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_nice(value: i32) -> io::Result<()> {
    let result = unsafe { fractald_set_nice(value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_oom_score_adjust(value: i32) -> io::Result<()> {
    let result = unsafe { fractald_set_oom_score_adjust(value) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_nofile_limit(soft: u64, hard: u64) -> io::Result<()> {
    let result = unsafe { fractald_set_nofile_limit(soft, hard) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_memlock_limit(soft: u64, hard: u64) -> io::Result<()> {
    let result = unsafe { fractald_set_memlock_limit(soft, hard) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_nproc_limit(soft: u64, hard: u64) -> io::Result<()> {
    let result = unsafe { fractald_set_nproc_limit(soft, hard) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_tmp(
    mode: i32,
    tmp_path: Option<&std::path::Path>,
    var_tmp_path: Option<&std::path::Path>,
) -> io::Result<()> {
    let tmp_path = tmp_path
        .map(|path| std::ffi::CString::new(path.as_os_str().as_bytes()))
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "temporary path contains NUL"))?;
    let var_tmp_path = var_tmp_path
        .map(|path| std::ffi::CString::new(path.as_os_str().as_bytes()))
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "temporary path contains NUL"))?;
    let result = unsafe {
        fractald_enter_private_tmp(
            mode,
            tmp_path
                .as_ref()
                .map_or(std::ptr::null(), |path| path.as_ptr()),
            var_tmp_path
                .as_ref()
                .map_or(std::ptr::null(), |path| path.as_ptr()),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_network() -> io::Result<()> {
    let result = unsafe { fractald_enter_private_network() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_ipc() -> io::Result<()> {
    let result = unsafe { fractald_enter_private_ipc() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_devices() -> io::Result<()> {
    let result = unsafe { fractald_enter_private_devices() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_users(
    mode: i32,
    user: Option<&CStr>,
    group: Option<&CStr>,
    supplementary_groups: Option<&[CString]>,
) -> io::Result<()> {
    let supplementary_groups = supplementary_groups
        .unwrap_or_default()
        .iter()
        .map(|value| value.as_ptr())
        .collect::<Vec<_>>();
    let result = unsafe {
        fractald_enter_private_users(
            mode,
            user.map_or(std::ptr::null(), CStr::as_ptr),
            group.map_or(std::ptr::null(), CStr::as_ptr),
            supplementary_groups.as_ptr(),
            supplementary_groups.len(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn attach_device_filter(
    cgroup_path: &Path,
    default_allow: bool,
    rules: &[DeviceFilterRule],
) -> io::Result<()> {
    let cgroup_path = CString::new(cgroup_path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cgroup path contains NUL"))?;
    let result = unsafe {
        fractald_attach_device_filter(
            cgroup_path.as_ptr(),
            i32::from(default_allow),
            rules.as_ptr(),
            rules.len(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn detach_device_filter(cgroup_path: &Path) -> io::Result<()> {
    let cgroup_path = CString::new(cgroup_path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "cgroup path contains NUL"))?;
    let result = unsafe { fractald_detach_device_filter(cgroup_path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_uts() -> io::Result<()> {
    let result = unsafe { fractald_enter_private_uts() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enter_private_uts_namespace() -> io::Result<()> {
    let result = unsafe { fractald_enter_private_uts_namespace() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn install_hostname_filter() -> io::Result<()> {
    let result = unsafe { fractald_install_hostname_filter() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn lock_personality() -> io::Result<()> {
    let result = unsafe { fractald_lock_personality() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn protect_clock() -> io::Result<()> {
    let result = unsafe { fractald_protect_clock() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn apply_filesystem_protection(
    protect_system: i32,
    protect_home: i32,
    protect_proc: i32,
    proc_subset: i32,
) -> io::Result<()> {
    let result = unsafe {
        fractald_apply_filesystem_protection(
            protect_system,
            protect_home,
            protect_proc,
            proc_subset,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn make_path_writable(path: &std::path::Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let result = unsafe { fractald_make_path_writable(path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn make_path_read_only(path: &std::path::Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let result = unsafe { fractald_make_path_read_only(path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn make_path_inaccessible(path: &std::path::Path) -> io::Result<()> {
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let result = unsafe { fractald_make_path_inaccessible(path.as_ptr()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn signal_process(pid: u32, signal_number: i32) -> io::Result<()> {
    let result = unsafe { fractald_signal_process(pid, signal_number) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn signal_process_group(pid: u32, signal_number: i32) -> io::Result<()> {
    let result = unsafe { fractald_signal_process_group(pid, signal_number) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_identity(
    user: Option<&CStr>,
    group: Option<&CStr>,
    supplementary_groups: Option<&[CString]>,
) -> io::Result<()> {
    let pointers = supplementary_groups
        .map(|groups| {
            groups
                .iter()
                .map(|group| group.as_ptr())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let pointer = supplementary_groups.map_or(std::ptr::null(), |_| pointers.as_ptr());
    let result = unsafe {
        fractald_set_identity(
            user.map_or(std::ptr::null(), CStr::as_ptr),
            group.map_or(std::ptr::null(), CStr::as_ptr),
            pointer,
            pointers.len(),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn apply_capability_bounding_set(allowed: u64) -> io::Result<()> {
    let result = unsafe { fractald_apply_capability_bounding_set(allowed) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn drop_capability_bounding_set(allowed: u64) -> io::Result<()> {
    let result = unsafe { fractald_drop_capability_bounding_set(allowed) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn set_keep_capabilities() -> io::Result<()> {
    let result = unsafe { fractald_set_keep_capabilities() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn apply_ambient_capabilities(allowed: u64) -> io::Result<()> {
    let result = unsafe { fractald_apply_ambient_capabilities(allowed) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn restrict_address_families(allowed: u64) -> io::Result<()> {
    let result = unsafe { fractald_restrict_address_families(allowed) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn restrict_suid_sgid() -> io::Result<()> {
    let result = unsafe { fractald_restrict_suid_sgid() };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn restrict_namespaces(allowed: u32) -> io::Result<()> {
    let result = unsafe { fractald_restrict_namespaces(allowed) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn install_system_call_filter(
    names: &[CString],
    actions: &[i32],
    default_allow: bool,
    default_errno: Option<i32>,
    architecture: i32,
) -> io::Result<()> {
    if names.len() != actions.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "system call filter names and actions differ in length",
        ));
    }
    if names.is_empty() {
        return Ok(());
    }
    let pointers = names.iter().map(|name| name.as_ptr()).collect::<Vec<_>>();
    let result = unsafe {
        fractald_install_system_call_filter(
            pointers.as_ptr(),
            actions.as_ptr(),
            names.len(),
            i32::from(default_allow),
            default_errno.unwrap_or(0),
            architecture,
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn chown_path(path: &CStr, user: Option<&CStr>, group: Option<&CStr>) -> io::Result<()> {
    let result = unsafe {
        fractald_chown_path(
            path.as_ptr(),
            user.map_or(std::ptr::null(), CStr::as_ptr),
            group.map_or(std::ptr::null(), CStr::as_ptr),
        )
    };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn send_unix_datagram(address: &CStr, payload: &[u8]) -> io::Result<()> {
    let result =
        unsafe { fractald_send_unix_datagram(address.as_ptr(), payload.as_ptr(), payload.len()) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn enable_socket_credentials(fd: RawFd) -> io::Result<()> {
    let result = unsafe { fractald_enable_socket_credentials(fd) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub fn receive_socket_credentials(
    fd: RawFd,
    payload: &mut [u8],
) -> io::Result<Option<(usize, u32)>> {
    let mut sender_pid = 0_u32;
    let result = unsafe {
        fractald_receive_socket_credentials(
            fd,
            payload.as_mut_ptr(),
            payload.len(),
            &mut sender_pid,
        )
    };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        return Err(error);
    }
    Ok(Some((result as usize, sender_pid)))
}

impl Drop for PidFd {
    fn drop(&mut self) {
        let _ = unsafe { fractald_fd_close(self.raw_fd) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixDatagram;
    use std::os::unix::process::CommandExt;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Command;

    #[test]
    fn observes_exit_code_through_pidfd() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .expect("spawn child");
        let pidfd = PidFd::open(child.id()).expect("open pidfd");
        let exit = pidfd.wait().expect("wait through pidfd");
        assert_eq!(exit, ExitKind::Exited(7));
    }

    #[test]
    fn sends_signal_to_owned_process() {
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 30"])
            .spawn()
            .expect("spawn child");
        let pidfd = PidFd::open(child.id()).expect("open pidfd");
        pidfd.send_signal(15).expect("send SIGTERM");
        let exit = pidfd.wait().expect("wait through pidfd");
        assert_eq!(exit, ExitKind::Signaled(15));
    }

    #[test]
    fn restores_sigpipe_default_for_a_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "kill -PIPE $$"]);
        unsafe {
            command.pre_exec(|| set_signal_disposition(13, false));
        }
        let status = command.status().expect("run default-sigpipe child");
        assert_eq!(status.signal(), Some(13));
    }

    #[test]
    fn can_ignore_sigpipe_for_a_child() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "kill -PIPE $$; exit 7"]);
        unsafe {
            command.pre_exec(|| set_signal_disposition(13, true));
        }
        let status = command.status().expect("run ignored-sigpipe child");
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn validates_filesystem_protection_modes_before_namespace_setup() {
        assert!(apply_filesystem_protection(-1, 0, 0, 0).is_err());
        assert!(apply_filesystem_protection(0, 4, 0, 0).is_err());
    }

    #[test]
    fn ignores_missing_writable_paths() {
        make_path_writable(std::path::Path::new(
            "/this/path/does/not/exist-for-fractald",
        ))
        .expect("missing writable path is optional");
    }

    #[test]
    fn ignores_missing_read_only_and_inaccessible_paths() {
        make_path_read_only(std::path::Path::new(
            "/this/path/does/not/exist-for-fractald-read-only",
        ))
        .expect("missing read-only path is optional");
        make_path_inaccessible(std::path::Path::new(
            "/this/path/does/not/exist-for-fractald-inaccessible",
        ))
        .expect("missing inaccessible path is optional");
    }

    #[test]
    fn receives_unix_sender_credentials() {
        let path = std::env::temp_dir().join(format!(
            "fractald-platform-credentials-{}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let receiver = UnixDatagram::bind(&path).expect("credential receiver");
        enable_socket_credentials(receiver.as_raw_fd()).expect("enable credentials");
        let address = CString::new(path.as_os_str().as_bytes()).expect("socket path");
        send_unix_datagram(&address, b"READY=1\n").expect("send credential datagram");
        let mut payload = [0_u8; 128];
        let (length, sender_pid) = receive_socket_credentials(receiver.as_raw_fd(), &mut payload)
            .expect("receive credential datagram")
            .expect("credential datagram");
        assert_eq!(&payload[..length], b"READY=1\n");
        assert_eq!(sender_pid, std::process::id());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn denies_a_filtered_system_call_with_errno() {
        let names = vec![CString::new("write").expect("syscall name")];
        let actions = vec![1_i32];
        let mut command = Command::new("/bin/echo");
        command.arg("filtered");
        unsafe {
            command
                .pre_exec(move || install_system_call_filter(&names, &actions, true, Some(1), 1));
        }
        let status = command.status().expect("run filtered command");
        assert!(!status.success());
    }

    #[test]
    fn blocks_setting_suid_and_sgid_bits() {
        let path = format!("/tmp/fractald-suid-sgid-{}", std::process::id());
        std::fs::write(&path, b"test").expect("create test file");
        let mut command = Command::new("/bin/chmod");
        command.args(["4755", &path]);
        unsafe {
            command.pre_exec(restrict_suid_sgid);
        }
        let status = command.status().expect("run restricted chmod");
        assert!(!status.success());
        let mode = std::fs::metadata(&path)
            .expect("stat test file")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(mode, 0o644);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn installs_clock_protection_before_exec() {
        let mut command = Command::new("/bin/true");
        unsafe {
            command.pre_exec(protect_clock);
        }
        let status = command.status().expect("run clock-protected command");
        assert!(status.success());
    }

    #[test]
    fn installs_namespace_restrictions_before_exec() {
        let mut command = Command::new("/bin/true");
        unsafe {
            command.pre_exec(|| restrict_namespaces(0));
        }
        let status = command.status().expect("run namespace-restricted command");
        assert!(status.success());
    }

    #[test]
    fn clears_ambient_capabilities_without_privileges() {
        apply_ambient_capabilities(0).expect("clear ambient capabilities");
    }
}
