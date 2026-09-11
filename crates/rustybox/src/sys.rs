use std::ffi::{CStr, CString, OsStr};
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

unsafe extern "C" {
    fn rustybox_copy_fd(input_fd: RawFd, output_fd: RawFd, copied: *mut u64) -> i32;
    fn rustybox_write_all(output_fd: RawFd, buffer: *const u8, length: usize) -> i32;
    fn rustybox_sleep_milliseconds(milliseconds: u64) -> i32;
    fn rustybox_mkdir_one(path: *const i8, mode: u32) -> i32;
    fn rustybox_remove_file(path: *const i8) -> i32;
    fn rustybox_remove_directory(path: *const i8) -> i32;
    fn rustybox_rename_path(source: *const i8, destination: *const i8) -> i32;
    fn rustybox_make_link(source: *const i8, destination: *const i8, symbolic: i32) -> i32;
    fn rustybox_send_signal(pid: i32, signal_number: i32) -> i32;
    fn rustybox_mount_path(
        source: *const i8,
        target: *const i8,
        filesystem: *const i8,
        options: *const i8,
        flags: u64,
    ) -> i32;
    fn rustybox_unmount_path(target: *const i8, flags: i32) -> i32;
    fn rustybox_enable_swap(path: *const i8, flags: i32) -> i32;
    fn rustybox_disable_swap(path: *const i8) -> i32;
    fn rustybox_change_root(path: *const i8) -> i32;
    fn rustybox_switch_root(path: *const i8) -> i32;
    fn rustybox_sync() -> i32;
    fn rustybox_insert_module(path: *const i8, parameters: *const i8) -> i32;
    fn rustybox_remove_module(name: *const i8) -> i32;
    fn rustybox_uname_field(field: i32, buffer: *mut i8, capacity: usize) -> i32;
}

fn path_string(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path contains NUL: {}", path.display()),
        )
    })
}

fn os_string(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "argument contains NUL"))
}

fn result(code: i32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(-code))
    }
}

pub fn copy_fd(input_fd: RawFd, output_fd: RawFd) -> io::Result<u64> {
    let mut copied = 0_u64;
    let code = unsafe { rustybox_copy_fd(input_fd, output_fd, &mut copied) };
    result(code).map(|()| copied)
}

pub fn write_all(output_fd: RawFd, bytes: &[u8]) -> io::Result<()> {
    let code = unsafe { rustybox_write_all(output_fd, bytes.as_ptr(), bytes.len()) };
    result(code)
}

pub fn sleep_milliseconds(milliseconds: u64) -> io::Result<()> {
    result(unsafe { rustybox_sleep_milliseconds(milliseconds) })
}

pub fn mkdir_one(path: &Path, mode: u32) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_mkdir_one(path.as_ptr(), mode) })
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_remove_file(path.as_ptr()) })
}

pub fn remove_directory(path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_remove_directory(path.as_ptr()) })
}

pub fn rename(source: &Path, destination: &Path) -> io::Result<()> {
    let source = path_string(source)?;
    let destination = path_string(destination)?;
    result(unsafe { rustybox_rename_path(source.as_ptr(), destination.as_ptr()) })
}

pub fn link(source: &OsStr, destination: &Path, symbolic: bool) -> io::Result<()> {
    let source = os_string(source)?;
    let destination = path_string(destination)?;
    result(unsafe {
        rustybox_make_link(source.as_ptr(), destination.as_ptr(), i32::from(symbolic))
    })
}

pub fn send_signal(pid: i32, signal_number: i32) -> io::Result<()> {
    result(unsafe { rustybox_send_signal(pid, signal_number) })
}

pub fn mount(
    source: Option<&OsStr>,
    target: &Path,
    filesystem: Option<&str>,
    options: Option<&str>,
    flags: u64,
) -> io::Result<()> {
    let source = source.map(os_string).transpose()?;
    let target = path_string(target)?;
    let filesystem = filesystem
        .map(CString::new)
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "filesystem name contains NUL"))?;
    let options = options
        .map(CString::new)
        .transpose()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "mount options contain NUL"))?;
    result(unsafe {
        rustybox_mount_path(
            source
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            target.as_ptr(),
            filesystem
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            options
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            flags,
        )
    })
}

pub fn unmount(target: &Path, flags: i32) -> io::Result<()> {
    let target = path_string(target)?;
    result(unsafe { rustybox_unmount_path(target.as_ptr(), flags) })
}

pub fn enable_swap(path: &Path, flags: i32) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_enable_swap(path.as_ptr(), flags) })
}

pub fn disable_swap(path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_disable_swap(path.as_ptr()) })
}

pub fn change_root(path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_change_root(path.as_ptr()) })
}

pub fn switch_root(path: &Path) -> io::Result<()> {
    let path = path_string(path)?;
    result(unsafe { rustybox_switch_root(path.as_ptr()) })
}

pub fn sync() -> io::Result<()> {
    result(unsafe { rustybox_sync() })
}

pub fn insert_module(path: &Path, parameters: &str) -> io::Result<()> {
    let path = path_string(path)?;
    let parameters = CString::new(parameters).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "module parameters contain NUL")
    })?;
    result(unsafe { rustybox_insert_module(path.as_ptr(), parameters.as_ptr()) })
}

pub fn remove_module(name: &OsStr) -> io::Result<()> {
    let name = os_string(name)?;
    result(unsafe { rustybox_remove_module(name.as_ptr()) })
}

pub fn uname_field(field: i32) -> io::Result<String> {
    let mut buffer = [0_i8; 512];
    result(unsafe { rustybox_uname_field(field, buffer.as_mut_ptr(), buffer.len()) })?;
    let value = unsafe { CStr::from_ptr(buffer.as_ptr()) };
    Ok(value.to_string_lossy().into_owned())
}
