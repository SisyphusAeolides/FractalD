#define _GNU_SOURCE

#include "fractald_platform.h"

#include <errno.h>
#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <pwd.h>
#include <signal.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/resource.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <sys/sysmacros.h>
#include <linux/netlink.h>
#include <stddef.h>
#include <sched.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/prctl.h>
#include <linux/audit.h>
#include <linux/bpf.h>
#include <linux/capability.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <unistd.h>

#ifndef SYS_bpf
#ifdef __NR_bpf
#define SYS_bpf __NR_bpf
#endif
#endif

#ifndef P_PIDFD
#define P_PIDFD 3
#endif

#ifndef PR_CAP_AMBIENT
#define PR_CAP_AMBIENT 47
#endif
#ifndef PR_CAP_AMBIENT_RAISE
#define PR_CAP_AMBIENT_RAISE 2
#endif
#ifndef PR_CAP_AMBIENT_CLEAR_ALL
#define PR_CAP_AMBIENT_CLEAR_ALL 4
#endif

#ifndef PR_SET_KEEPCAPS
#define PR_SET_KEEPCAPS 8
#endif

#ifndef CLONE_NEWTIME
#define CLONE_NEWTIME 0x00000080
#endif

#ifndef SYS_pidfd_open
#ifdef __NR_pidfd_open
#define SYS_pidfd_open __NR_pidfd_open
#endif
#endif

#ifndef SYS_pidfd_send_signal
#ifdef __NR_pidfd_send_signal
#define SYS_pidfd_send_signal __NR_pidfd_send_signal
#endif
#endif

int fractald_pidfd_open(uint32_t pid)
{
#ifndef SYS_pidfd_open
    (void)pid;
    errno = ENOSYS;
    return -1;
#else
    int fd = (int)syscall(SYS_pidfd_open, (pid_t)pid, 0U);
    if (fd < 0) {
        return -1;
    }

    int descriptor_flags = fcntl(fd, F_GETFD);
    if (descriptor_flags < 0 || fcntl(fd, F_SETFD, descriptor_flags | FD_CLOEXEC) < 0) {
        int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return -1;
    }
    return fd;
#endif
}

int fractald_pidfd_send_signal(int pidfd, int signal_number)
{
#ifndef SYS_pidfd_send_signal
    (void)pidfd;
    (void)signal_number;
    errno = ENOSYS;
    return -1;
#else
    return (int)syscall(SYS_pidfd_send_signal, pidfd, signal_number, NULL, 0U);
#endif
}

int fractald_pidfd_wait(int pidfd, int nohang, int *code, int *value)
{
    if (code == NULL || value == NULL) {
        errno = EINVAL;
        return -1;
    }

    siginfo_t info;
    memset(&info, 0, sizeof(info));
    int options = WEXITED;
    if (nohang != 0) {
        options |= WNOHANG;
    }

    if (waitid(P_PIDFD, (id_t)pidfd, &info, options) < 0) {
        return -1;
    }
    if (info.si_pid == 0) {
        return 0;
    }

    *code = info.si_code;
    *value = info.si_status;
    return 1;
}

int fractald_set_process_group(void)
{
    return setpgid(0, 0);
}

int fractald_set_parent_death_signal(int signal_number, uint32_t expected_parent)
{
    if (signal_number <= 0) {
        errno = EINVAL;
        return -1;
    }
    if (prctl(PR_SET_PDEATHSIG, signal_number) < 0) {
        return -1;
    }
    if (expected_parent != 0U && getppid() != (pid_t)expected_parent) {
        (void)kill(getpid(), SIGKILL);
        errno = EPIPE;
        return -1;
    }
    return 0;
}

int fractald_set_signal_disposition(int signal_number, int ignored)
{
    if (signal_number <= 0) {
        errno = EINVAL;
        return -1;
    }

    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = ignored != 0 ? SIG_IGN : SIG_DFL;
    if (sigemptyset(&action.sa_mask) < 0) {
        return -1;
    }
    return sigaction(signal_number, &action, NULL);
}

int fractald_set_no_new_privileges(void)
{
    return prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL);
}

#ifndef PR_SET_MDWE
#define PR_SET_MDWE 65
#endif

#ifndef PR_MDWE_REFUSE_EXEC_GAIN
#define PR_MDWE_REFUSE_EXEC_GAIN 1
#endif

int fractald_set_memory_deny_write_execute(void)
{
    return prctl(PR_SET_MDWE, PR_MDWE_REFUSE_EXEC_GAIN, 0UL, 0UL, 0UL);
}

int fractald_restrict_realtime(void)
{
    struct rlimit limit = {0, 0};
    if (setrlimit(RLIMIT_RTPRIO, &limit) < 0) {
        return -1;
    }
    return setrlimit(RLIMIT_RTTIME, &limit);
}

int fractald_restrict_suid_sgid(void)
{
#if !defined(PR_SET_SECCOMP) || !defined(SECCOMP_MODE_FILTER)
    errno = ENOSYS;
    return -1;
#else
#if defined(__x86_64__)
    const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
    const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
    const uint32_t audit_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
    const uint32_t audit_arch = AUDIT_ARCH_ARM;
#else
    errno = ENOTSUP;
    return -1;
#endif
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[64];
    size_t length = 0U;
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, arch));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1U, 0U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS);

#ifdef SYS_chmod
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_chmod, 0U, 2U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[1]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, 06000U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_fchmod
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_fchmod, 0U, 2U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[1]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, 06000U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_fchmodat
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_fchmodat, 0U, 2U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[2]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, 06000U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_fchmodat2
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_fchmodat2, 0U, 2U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[2]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, 06000U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    struct sock_fprog program = {
        .len = (unsigned short)length,
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

int fractald_restrict_namespaces(uint32_t allowed)
{
    const uint32_t all_namespaces = UINT32_C(0x7e020080);
    const uint32_t blocked = all_namespaces & ~allowed;
    if (blocked == 0U) {
        return 0;
    }
#if !defined(PR_SET_SECCOMP) || !defined(SECCOMP_MODE_FILTER)
    (void)blocked;
    errno = ENOSYS;
    return -1;
#else
#if defined(__x86_64__)
    const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
    const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
    const uint32_t audit_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
    const uint32_t audit_arch = AUDIT_ARCH_ARM;
#else
    errno = ENOTSUP;
    return -1;
#endif
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[48];
    size_t length = 0U;
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, arch));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1U, 0U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS);
#ifdef SYS_unshare
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_unshare, 0U, 3U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[0]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, blocked, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clone
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clone, 0U, 3U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[0]));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, blocked, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clone3
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clone3, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_setns
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_setns, 0U, 5U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[1]));
    filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JSET | BPF_K, blocked, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    struct sock_fprog program = {
        .len = (unsigned short)length,
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

int fractald_set_umask(uint32_t mask)
{
    if (mask > 07777U) {
        errno = EINVAL;
        return -1;
    }
    (void)umask((mode_t)mask);
    return 0;
}

int fractald_set_nice(int value)
{
    errno = 0;
    if (setpriority(PRIO_PROCESS, 0, value) < 0) {
        return -1;
    }
    return 0;
}

int fractald_set_oom_score_adjust(int value)
{
    if (value < -1000 || value > 1000) {
        errno = EINVAL;
        return -1;
    }

    int descriptor = open("/proc/self/oom_score_adj", O_WRONLY | O_CLOEXEC);
    if (descriptor < 0) {
        return -1;
    }
    char text[32];
    int length = snprintf(text, sizeof(text), "%d", value);
    if (length < 0 || (size_t)length >= sizeof(text)) {
        int saved_errno = EOVERFLOW;
        (void)close(descriptor);
        errno = saved_errno;
        return -1;
    }
    ssize_t written = write(descriptor, text, (size_t)length);
    int saved_errno = errno;
    (void)close(descriptor);
    if (written != (ssize_t)length) {
        errno = written < 0 ? saved_errno : EIO;
        return -1;
    }
    return 0;
}

static int convert_rlimit(uint64_t value, rlim_t *result)
{
    if (value == UINT64_MAX) {
        *result = RLIM_INFINITY;
        return 0;
    }
    rlim_t converted = (rlim_t)value;
    if ((uint64_t)converted != value) {
        errno = ERANGE;
        return -1;
    }
    *result = converted;
    return 0;
}

static int set_resource_limit(int resource, uint64_t soft, uint64_t hard)
{
    struct rlimit limit;
    if (convert_rlimit(soft, &limit.rlim_cur) < 0
        || convert_rlimit(hard, &limit.rlim_max) < 0) {
        return -1;
    }
    if (limit.rlim_cur > limit.rlim_max) {
        errno = EINVAL;
        return -1;
    }
    return setrlimit(resource, &limit);
}

int fractald_set_nofile_limit(uint64_t soft, uint64_t hard)
{
    return set_resource_limit(RLIMIT_NOFILE, soft, hard);
}

int fractald_set_memlock_limit(uint64_t soft, uint64_t hard)
{
    return set_resource_limit(RLIMIT_MEMLOCK, soft, hard);
}

int fractald_set_nproc_limit(uint64_t soft, uint64_t hard)
{
    return set_resource_limit(RLIMIT_NPROC, soft, hard);
}

static int mount_private_tmp_path(const char *path)
{
    return mount("tmpfs", path, "tmpfs", MS_NOSUID | MS_NODEV, "mode=1777");
}

static int enter_private_mount_namespace(void)
{
    if (unshare(CLONE_NEWNS) < 0) {
        return -1;
    }
    return mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL);
}

static int path_metadata(const char *path, struct stat *metadata)
{
    if (path == NULL || *path != '/' || metadata == NULL) {
        errno = EINVAL;
        return -1;
    }
    if (stat(path, metadata) < 0) {
        if (errno == ENOENT || errno == ENOTDIR) {
            return 0;
        }
        return -1;
    }
    return 1;
}

static int remount_path(const char *path, int read_only)
{
    struct stat metadata;
    int present = path_metadata(path, &metadata);
    if (present <= 0) {
        return present;
    }

    unsigned long bind_flags = MS_BIND;
    unsigned long remount_flags = MS_BIND | MS_REMOUNT;
    if (S_ISDIR(metadata.st_mode)) {
        bind_flags |= MS_REC;
        remount_flags |= MS_REC;
    }
    if (mount(path, path, NULL, bind_flags, NULL) < 0) {
        return -1;
    }
    if (read_only != 0) {
        remount_flags |= MS_RDONLY;
    }
    return mount(NULL, path, NULL, remount_flags, NULL);
}

static int remount_root_read_only(void)
{
    return mount(NULL, "/", NULL, MS_BIND | MS_REMOUNT | MS_RDONLY | MS_REC, NULL);
}

static int mount_protected_home(const char *path, const char *options)
{
    struct stat metadata;
    int present = path_metadata(path, &metadata);
    if (present <= 0) {
        return present;
    }
    if (!S_ISDIR(metadata.st_mode)) {
        errno = ENOTDIR;
        return -1;
    }
    return mount("tmpfs", path, "tmpfs", MS_NOSUID | MS_NODEV | MS_NOEXEC, options);
}

int fractald_apply_filesystem_protection(
    int protect_system,
    int protect_home,
    int protect_proc,
    int proc_subset)
{
    if (protect_system < 0 || protect_system > 3
        || protect_home < 0 || protect_home > 3
        || protect_proc < 0 || protect_proc > 3
        || proc_subset < 0 || proc_subset > 1) {
        errno = EINVAL;
        return -1;
    }
    if (enter_private_mount_namespace() < 0) {
        return -1;
    }

    if (protect_system == 1 || protect_system == 2) {
        const char *paths_yes[] = {"/usr", "/boot", "/efi"};
        const char *paths_full[] = {"/usr", "/boot", "/efi", "/etc"};
        const char **paths = protect_system == 1 ? paths_yes : paths_full;
        size_t path_count = protect_system == 1
            ? sizeof(paths_yes) / sizeof(paths_yes[0])
            : sizeof(paths_full) / sizeof(paths_full[0]);
        for (size_t index = 0U; index < path_count; ++index) {
            if (remount_path(paths[index], 1) < 0) {
                return -1;
            }
        }
    } else if (protect_system == 3) {
        if (remount_root_read_only() < 0) {
            return -1;
        }
        const char *api_paths[] = {"/dev", "/proc", "/sys", "/run"};
        for (size_t index = 0U; index < sizeof(api_paths) / sizeof(api_paths[0]); ++index) {
            if (remount_path(api_paths[index], 0) < 0) {
                return -1;
            }
        }
    }

    if (protect_home == 1 || protect_home == 3) {
        const char *paths[] = {"/home", "/root", "/run/user"};
        const char *options = protect_home == 1 ? "mode=000" : "mode=0755";
        for (size_t index = 0U; index < sizeof(paths) / sizeof(paths[0]); ++index) {
            if (mount_protected_home(paths[index], options) < 0) {
                return -1;
            }
        }
    } else if (protect_home == 2) {
        const char *paths[] = {"/home", "/root", "/run/user"};
        for (size_t index = 0U; index < sizeof(paths) / sizeof(paths[0]); ++index) {
            if (remount_path(paths[index], 1) < 0) {
                return -1;
            }
        }
    }
    if (protect_proc != 0 || proc_subset != 0) {
        char options[64];
        size_t offset = 0U;
        options[0] = '\0';
        if (protect_proc != 0) {
            int written = snprintf(options, sizeof(options), "hidepid=%d", protect_proc);
            if (written < 0 || (size_t)written >= sizeof(options)) {
                errno = EOVERFLOW;
                return -1;
            }
            offset = (size_t)written;
        }
        if (proc_subset != 0) {
            int written = snprintf(
                options + offset,
                sizeof(options) - offset,
                "%ssubset=pid",
                offset == 0U ? "" : ","
            );
            if (written < 0 || (size_t)written >= sizeof(options) - offset) {
                errno = EOVERFLOW;
                return -1;
            }
        }
        if (mount(
                "proc",
                "/proc",
                "proc",
                MS_NOSUID | MS_NODEV | MS_NOEXEC,
                options
            ) < 0) {
            if (errno == EINVAL || errno == EOPNOTSUPP || errno == ENOTSUP) {
                return 0;
            }
            return -1;
        }
    }
    return 0;
}

int fractald_make_path_writable(const char *path)
{
    return remount_path(path, 0);
}

int fractald_make_path_read_only(const char *path)
{
    return remount_path(path, 1);
}

static int create_inaccessible_placeholder(char *path, size_t path_size)
{
    const char *directories[] = {"/dev/shm", "/run", "/tmp"};
    for (size_t index = 0U; index < sizeof(directories) / sizeof(directories[0]); ++index) {
        int length = snprintf(
            path,
            path_size,
            "%s/.fractald-inaccessible-XXXXXX",
            directories[index]
        );
        if (length < 0 || (size_t)length >= path_size) {
            continue;
        }
        int descriptor = mkstemp(path);
        if (descriptor >= 0) {
            return descriptor;
        }
    }
    return -1;
}

int fractald_make_path_inaccessible(const char *path)
{
    struct stat metadata;
    int present = path_metadata(path, &metadata);
    if (present <= 0) {
        return present;
    }
    if (S_ISDIR(metadata.st_mode)) {
        return mount(
            "tmpfs",
            path,
            "tmpfs",
            MS_NOSUID | MS_NODEV | MS_NOEXEC | MS_RDONLY,
            "mode=000"
        );
    }

    char placeholder[PATH_MAX];
    int descriptor = create_inaccessible_placeholder(placeholder, sizeof(placeholder));
    if (descriptor < 0) {
        return -1;
    }
    int result = 0;
    int saved_errno = 0;
    if (fchmod(descriptor, 0) < 0) {
        result = -1;
        saved_errno = errno;
    } else if (mount(placeholder, path, NULL, MS_BIND, NULL) < 0) {
        result = -1;
        saved_errno = errno;
    } else if (mount(NULL, path, NULL, MS_BIND | MS_REMOUNT | MS_RDONLY, NULL) < 0) {
        result = -1;
        saved_errno = errno;
    }
    (void)close(descriptor);
    (void)unlink(placeholder);
    if (result < 0) {
        errno = saved_errno;
    }
    return result;
}

static int bind_private_tmp_path(const char *source, const char *target)
{
    if (source == NULL || *source == '\0') {
        errno = EINVAL;
        return -1;
    }
    if (mount(source, target, NULL, MS_BIND | MS_REC, NULL) < 0) {
        return -1;
    }
    return mount(NULL, target, NULL, MS_BIND | MS_REMOUNT | MS_NOSUID | MS_NODEV, NULL);
}

int fractald_enter_private_tmp(int mode, const char *tmp_path, const char *var_tmp_path)
{
    if (mode != 1 && mode != 2) {
        errno = EINVAL;
        return -1;
    }
    if (enter_private_mount_namespace() < 0) {
        return -1;
    }
    if (mode == 1) {
        if (bind_private_tmp_path(tmp_path, "/tmp") < 0) {
            return -1;
        }
        return bind_private_tmp_path(var_tmp_path, "/var/tmp");
    }
    if (mount_private_tmp_path("/tmp") < 0) {
        return -1;
    }
    return mount_private_tmp_path("/var/tmp");
}

static int make_private_device(const char *path, mode_t mode, unsigned int major_number, unsigned int minor_number)
{
    return mknod(path, S_IFCHR | mode, makedev(major_number, minor_number));
}

static int make_private_directory(const char *path, mode_t mode)
{
    if (mkdir(path, mode) < 0 && errno != EEXIST) {
        return -1;
    }
    return 0;
}

int fractald_enter_private_devices(void)
{
    if (enter_private_mount_namespace() < 0) {
        return -1;
    }
    if (mount("tmpfs", "/dev", "tmpfs", MS_NOSUID | MS_NOEXEC, "mode=0755") < 0) {
        return -1;
    }
    if (make_private_device("/dev/null", 0666, 1U, 3U) < 0
        || make_private_device("/dev/zero", 0666, 1U, 5U) < 0
        || make_private_device("/dev/full", 0666, 1U, 7U) < 0
        || make_private_device("/dev/random", 0666, 1U, 8U) < 0
        || make_private_device("/dev/urandom", 0666, 1U, 9U) < 0
        || make_private_device("/dev/tty", 0666, 5U, 0U) < 0) {
        return -1;
    }
    if (make_private_directory("/dev/pts", 0755) < 0
        || mount(
            "devpts",
            "/dev/pts",
            "devpts",
            MS_NOSUID | MS_NOEXEC,
            "newinstance,ptmxmode=0666,mode=0620"
        ) < 0) {
        return -1;
    }
    if (symlink("pts/ptmx", "/dev/ptmx") < 0
        || make_private_directory("/dev/shm", 01777) < 0
        || mount(
            "tmpfs",
            "/dev/shm",
            "tmpfs",
            MS_NOSUID | MS_NODEV | MS_NOEXEC,
            "mode=1777"
        ) < 0
        || make_private_directory("/dev/mqueue", 0755) < 0
        || symlink("/proc/self/fd", "/dev/fd") < 0
        || symlink("/proc/self/fd/0", "/dev/stdin") < 0
        || symlink("/proc/self/fd/1", "/dev/stdout") < 0
        || symlink("/proc/self/fd/2", "/dev/stderr") < 0) {
        return -1;
    }
    return 0;
}

static struct bpf_insn fractald_bpf_instruction(
    uint8_t code,
    uint8_t dst_reg,
    uint8_t src_reg,
    int16_t offset,
    int32_t immediate)
{
    struct bpf_insn instruction = {
        .code = code,
        .dst_reg = dst_reg,
        .src_reg = src_reg,
        .off = offset,
        .imm = immediate,
    };
    return instruction;
}

static int fractald_bpf_load_device_filter(
    const struct fractald_device_rule *rules,
    size_t rule_count,
    int default_allow)
{
#ifndef SYS_bpf
    (void)rules;
    (void)rule_count;
    (void)default_allow;
    errno = ENOTSUP;
    return -1;
#else
    if (rule_count > (SIZE_MAX - 2U) / 14U) {
        errno = E2BIG;
        return -1;
    }
    size_t instruction_capacity = 2U + rule_count * 14U;
    struct bpf_insn *instructions = calloc(instruction_capacity, sizeof(*instructions));
    if (instructions == NULL) {
        errno = ENOMEM;
        return -1;
    }
    size_t instruction_count = 0U;
    for (size_t index = 0U; index < rule_count; index++) {
        const struct fractald_device_rule *rule = &rules[index];
        size_t condition_indices[4U];
        size_t condition_count = 0U;

        if (rule->device_type != 0U) {
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_LDX | BPF_W | BPF_MEM),
                BPF_REG_2,
                BPF_REG_1,
                0,
                0);
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_ALU | BPF_AND | BPF_K),
                BPF_REG_2,
                0U,
                0,
                0xffff);
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_JMP | BPF_JEQ | BPF_K),
                BPF_REG_2,
                0U,
                0,
                (int32_t)rule->device_type);
            condition_indices[condition_count++] = instruction_count - 1U;
        }
        if (rule->major != UINT32_MAX) {
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_LDX | BPF_W | BPF_MEM),
                BPF_REG_2,
                BPF_REG_1,
                4,
                0);
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_JMP | BPF_JEQ | BPF_K),
                BPF_REG_2,
                0U,
                0,
                (int32_t)rule->major);
            condition_indices[condition_count++] = instruction_count - 1U;
        }
        if (rule->minor != UINT32_MAX) {
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_LDX | BPF_W | BPF_MEM),
                BPF_REG_2,
                BPF_REG_1,
                8,
                0);
            instructions[instruction_count++] = fractald_bpf_instruction(
                (uint8_t)(BPF_JMP | BPF_JEQ | BPF_K),
                BPF_REG_2,
                0U,
                0,
                (int32_t)rule->minor);
            condition_indices[condition_count++] = instruction_count - 1U;
        }

        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_LDX | BPF_W | BPF_MEM),
            BPF_REG_2,
            BPF_REG_1,
            0,
            0);
        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_ALU | BPF_RSH | BPF_K),
            BPF_REG_2,
            0U,
            0,
            16);
        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_ALU | BPF_AND | BPF_K),
            BPF_REG_2,
            0U,
            0,
            7);
        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_ALU | BPF_AND | BPF_K),
            BPF_REG_2,
            0U,
            0,
            (int32_t)((~rule->access) & 7U));
        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_JMP | BPF_JEQ | BPF_K),
            BPF_REG_2,
            0U,
            0,
            0);
        condition_indices[condition_count++] = instruction_count - 1U;

        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_ALU64 | BPF_MOV | BPF_K),
            BPF_REG_0,
            0U,
            0,
            1);
        instructions[instruction_count++] = fractald_bpf_instruction(
            (uint8_t)(BPF_JMP | BPF_EXIT),
            0U,
            0U,
            0,
            0);
        size_t next_rule = instruction_count;
        for (size_t condition = 0U; condition < condition_count; condition++) {
            size_t instruction_index = condition_indices[condition];
            size_t jump = next_rule - instruction_index - 1U;
            if (jump > UINT16_MAX) {
                free(instructions);
                errno = E2BIG;
                return -1;
            }
            instructions[instruction_index].off = (int16_t)jump;
        }
    }
    instructions[instruction_count++] = fractald_bpf_instruction(
        (uint8_t)(BPF_ALU64 | BPF_MOV | BPF_K),
        BPF_REG_0,
        0U,
        0,
        default_allow ? 1 : 0);
    instructions[instruction_count++] = fractald_bpf_instruction(
        (uint8_t)(BPF_JMP | BPF_EXIT),
        0U,
        0U,
        0,
        0);

    char verifier_log[65536];
    memset(verifier_log, 0, sizeof(verifier_log));
    union bpf_attr load = {};
    load.prog_type = BPF_PROG_TYPE_CGROUP_DEVICE;
    load.expected_attach_type = BPF_CGROUP_DEVICE;
    load.insn_cnt = (uint32_t)instruction_count;
    load.insns = (uint64_t)(uintptr_t)instructions;
    load.license = (uint64_t)(uintptr_t)"GPL";
    load.log_level = 1U;
    load.log_size = (uint32_t)sizeof(verifier_log);
    load.log_buf = (uint64_t)(uintptr_t)verifier_log;
    (void)snprintf((char *)load.prog_name, sizeof(load.prog_name), "fractald_device");
    int program_fd = (int)syscall(SYS_bpf, BPF_PROG_LOAD, &load, sizeof(load));
    int saved_errno = errno;
    free(instructions);
    if (program_fd < 0) {
        errno = saved_errno;
        return -1;
    }
    return program_fd;
#endif
}

int fractald_attach_device_filter(
    const char *cgroup_path,
    int default_allow,
    const struct fractald_device_rule *rules,
    size_t rule_count)
{
#ifndef SYS_bpf
    (void)cgroup_path;
    (void)default_allow;
    (void)rules;
    (void)rule_count;
    errno = ENOTSUP;
    return -1;
#else
    if (cgroup_path == NULL || (rule_count != 0U && rules == NULL)) {
        errno = EINVAL;
        return -1;
    }
    int cgroup_fd = open(cgroup_path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (cgroup_fd < 0) {
        return -1;
    }
    int program_fd = fractald_bpf_load_device_filter(rules, rule_count, default_allow);
    if (program_fd < 0) {
        int saved_errno = errno;
        (void)close(cgroup_fd);
        errno = saved_errno;
        return -1;
    }
    union bpf_attr attach = {};
    attach.target_fd = (uint32_t)cgroup_fd;
    attach.attach_bpf_fd = (uint32_t)program_fd;
    attach.attach_type = BPF_CGROUP_DEVICE;
    int result = (int)syscall(SYS_bpf, BPF_PROG_ATTACH, &attach, sizeof(attach));
    int saved_errno = errno;
    (void)close(program_fd);
    (void)close(cgroup_fd);
    if (result < 0) {
        errno = saved_errno;
        return -1;
    }
    return 0;
#endif
}

int fractald_detach_device_filter(const char *cgroup_path)
{
#ifndef SYS_bpf
    (void)cgroup_path;
    errno = ENOTSUP;
    return -1;
#else
    if (cgroup_path == NULL) {
        errno = EINVAL;
        return -1;
    }
    int cgroup_fd = open(cgroup_path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (cgroup_fd < 0) {
        return -1;
    }
    union bpf_attr detach = {};
    detach.target_fd = (uint32_t)cgroup_fd;
    detach.attach_type = BPF_CGROUP_DEVICE;
    int result = (int)syscall(SYS_bpf, BPF_PROG_DETACH, &detach, sizeof(detach));
    int saved_errno = errno;
    (void)close(cgroup_fd);
    if (result < 0) {
        errno = saved_errno;
        return -1;
    }
    return 0;
#endif
}

int fractald_enter_private_network(void)
{
    return unshare(CLONE_NEWNET);
}

int fractald_enter_private_ipc(void)
{
    return unshare(CLONE_NEWIPC);
}

int fractald_enter_private_uts_namespace(void)
{
    return unshare(CLONE_NEWUTS);
}

int fractald_install_hostname_filter(void)
{
#if defined(__x86_64__)
    const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
    const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
    const uint32_t audit_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
    const uint32_t audit_arch = AUDIT_ARCH_ARM;
#else
    errno = ENOTSUP;
    return -1;
#endif
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
            (unsigned int)offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1U, 0U),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
            (unsigned int)offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_sethostname, 1U, 2U),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_setdomainname, 0U, 1U),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = (unsigned short)(sizeof(filter) / sizeof(filter[0])),
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
}

int fractald_enter_private_uts(void)
{
    if (fractald_enter_private_uts_namespace() < 0) {
        return -1;
    }
    return fractald_install_hostname_filter();
}

int fractald_lock_personality(void)
{
#if !defined(SYS_personality)
    errno = ENOTSUP;
    return -1;
#else
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
            (unsigned int)offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_personality, 0U, 1U),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = (unsigned short)(sizeof(filter) / sizeof(filter[0])),
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

int fractald_protect_clock(void)
{
#if defined(PR_CAPBSET_READ) && defined(PR_CAPBSET_DROP)
    int present = prctl(PR_CAPBSET_READ, CAP_SYS_TIME, 0UL, 0UL, 0UL);
    if (present > 0 && prctl(PR_CAPBSET_DROP, CAP_SYS_TIME, 0UL, 0UL, 0UL) < 0
        && errno != EPERM && errno != EINVAL) {
        return -1;
    }
#endif
#if defined(SYS_capget) && defined(SYS_capset)
    struct __user_cap_header_struct header = {
        .version = _LINUX_CAPABILITY_VERSION_3,
        .pid = 0,
    };
    struct __user_cap_data_struct data[2];
    if (syscall(SYS_capget, &header, data) < 0) {
        return -1;
    }
    uint32_t before_low = data[0].effective | data[0].permitted | data[0].inheritable;
    uint32_t before_high = data[1].effective | data[1].permitted | data[1].inheritable;
    data[0].effective &= ~(UINT32_C(1) << CAP_SYS_TIME);
    data[0].permitted &= ~(UINT32_C(1) << CAP_SYS_TIME);
    data[0].inheritable &= ~(UINT32_C(1) << CAP_SYS_TIME);
    if (before_low != (data[0].effective | data[0].permitted | data[0].inheritable)
        || before_high != (data[1].effective | data[1].permitted | data[1].inheritable)) {
        if (syscall(SYS_capset, &header, data) < 0) {
            return -1;
        }
    }
#endif
#if !defined(PR_SET_SECCOMP) || !defined(SECCOMP_MODE_FILTER)
    errno = ENOSYS;
    return -1;
#else
#if defined(__x86_64__)
    const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
    const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
    const uint32_t audit_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
    const uint32_t audit_arch = AUDIT_ARCH_ARM;
#else
    errno = ENOTSUP;
    return -1;
#endif
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[32];
    size_t length = 0U;
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, arch));
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1U, 0U);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS);
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
#ifdef SYS_adjtimex
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_adjtimex, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clock_adjtime
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_adjtime, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clock_adjtime64
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_adjtime64, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clock_settime
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_settime, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_clock_settime64
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_settime64, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_settimeofday
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_settimeofday, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
#ifdef SYS_stime
    filter[length++] = (struct sock_filter)BPF_JUMP(
        BPF_JMP | BPF_JEQ | BPF_K, SYS_stime, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
#endif
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    struct sock_fprog program = {
        .len = (unsigned short)length,
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

int fractald_signal_process(uint32_t pid, int signal_number)
{
    if (pid == 0U) {
        errno = EINVAL;
        return -1;
    }
    return kill((pid_t)pid, signal_number);
}

int fractald_signal_process_group(uint32_t pid, int signal_number)
{
    if (pid == 0U) {
        errno = EINVAL;
        return -1;
    }
    return kill(-(pid_t)pid, signal_number);
}

int fractald_set_keep_capabilities(void)
{
    return prctl(PR_SET_KEEPCAPS, 1UL, 0UL, 0UL, 0UL);
}

int fractald_drop_capability_bounding_set(uint64_t allowed)
{
#if !defined(PR_CAPBSET_READ) || !defined(PR_CAPBSET_DROP)
    (void)allowed;
    errno = ENOSYS;
    return -1;
#else
    for (unsigned int capability = 0U; capability <= 40U; ++capability) {
        if ((allowed & (UINT64_C(1) << capability)) != 0U) {
            continue;
        }
        int present = prctl(PR_CAPBSET_READ, capability, 0UL, 0UL, 0UL);
        if (present < 0) {
            return -1;
        }
        if (present != 0 && prctl(PR_CAPBSET_DROP, capability, 0UL, 0UL, 0UL) < 0) {
            return -1;
        }
    }
    return 0;
#endif
}

int fractald_apply_capability_bounding_set(uint64_t allowed)
{
#if !defined(SYS_capget) || !defined(SYS_capset)
    (void)allowed;
    errno = ENOSYS;
    return -1;
#else
    if (fractald_drop_capability_bounding_set(allowed) < 0) {
        return -1;
    }

    struct __user_cap_header_struct header = {
        .version = _LINUX_CAPABILITY_VERSION_3,
        .pid = 0,
    };
    struct __user_cap_data_struct data[2];
    if (syscall(SYS_capget, &header, data) < 0) {
        return -1;
    }
    uint32_t low = (uint32_t)allowed;
    uint32_t high = (uint32_t)(allowed >> 32U);
    data[0].effective &= low;
    data[0].permitted &= low;
    data[0].inheritable &= low;
    data[1].effective &= high;
    data[1].permitted &= high;
    data[1].inheritable &= high;
    if (syscall(SYS_capset, &header, data) < 0) {
        return -1;
    }
    if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0UL, 0UL, 0UL) < 0
        && errno != EINVAL) {
        return -1;
    }
    return 0;
#endif
}

int fractald_apply_ambient_capabilities(uint64_t allowed)
{
#if !defined(SYS_capget) || !defined(SYS_capset)
    (void)allowed;
    errno = ENOSYS;
    return -1;
#else
    struct __user_cap_header_struct header = {
        .version = _LINUX_CAPABILITY_VERSION_3,
        .pid = 0,
    };
    struct __user_cap_data_struct data[2];
    memset(data, 0, sizeof(data));
    if (syscall(SYS_capget, &header, data) < 0) {
        return -1;
    }

    uint32_t low = (uint32_t)allowed;
    uint32_t high = (uint32_t)(allowed >> 32U);
    if ((data[0].permitted & low) != low || (data[1].permitted & high) != high) {
        errno = EPERM;
        return -1;
    }
    data[0].inheritable |= low;
    data[1].inheritable |= high;
    data[0].effective |= low;
    data[1].effective |= high;
    if (syscall(SYS_capset, &header, data) < 0) {
        return -1;
    }

    if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0UL, 0UL, 0UL) < 0) {
        if (errno == EINVAL && allowed == 0U) {
            return 0;
        }
        if (errno == EINVAL) {
            errno = ENOSYS;
        }
        return -1;
    }
    for (unsigned int capability = 0U; capability <= 40U; ++capability) {
        if ((allowed & (UINT64_C(1) << capability)) == 0U) {
            continue;
        }
        if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, capability, 0UL, 0UL) < 0) {
            return -1;
        }
    }
    return 0;
#endif
}

int fractald_restrict_address_families(uint64_t allowed)
{
#if !defined(SYS_socket) || !defined(SYS_socketpair)
    (void)allowed;
    errno = ENOSYS;
    return -1;
#else
#if defined(__x86_64__)
    const uint32_t audit_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
    const uint32_t audit_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
    const uint32_t audit_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
    const uint32_t audit_arch = AUDIT_ARCH_ARM;
#else
    errno = ENOTSUP;
    return -1;
#endif
    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }
    struct sock_filter filter[128];
    size_t length = 0U;
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, arch));
    filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1U, 0U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_socket, 2U, 0U);
    filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_socketpair, 1U, 0U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, args[0]));
    for (unsigned int family = 0U; family <= 44U; ++family) {
        if ((allowed & (UINT64_C(1) << family)) != 0U) {
            continue;
        }
        filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, family, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
    }
    filter[length++] = (struct sock_filter)BPF_JUMP(BPF_JMP | BPF_JGT | BPF_K, 44U, 0U, 1U);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM);
    filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    struct sock_fprog program = {
        .len = (unsigned short)length,
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

static int fractald_syscall_number(const char *name)
{
    if (name == NULL) {
        return -1;
    }
#include "syscall_numbers.inc"
#ifdef SYS_lseek
    if (strcmp(name, "_llseek") == 0 || strcmp(name, "llseek") == 0) {
        return SYS_lseek;
    }
#endif
    return -1;
}

static unsigned int fractald_syscall_action(int32_t action, int32_t default_errno)
{
    if (action == -1) {
        return SECCOMP_RET_ALLOW;
    }
    if (action > 0) {
        return SECCOMP_RET_ERRNO | ((unsigned int)action & SECCOMP_RET_DATA);
    }
    if (default_errno > 0) {
        return SECCOMP_RET_ERRNO | ((unsigned int)default_errno & SECCOMP_RET_DATA);
    }
    return SECCOMP_RET_TRAP;
}

int fractald_install_system_call_filter(
    const char *const *names,
    const int32_t *actions,
    size_t count,
    int default_allow,
    int default_errno,
    int architecture)
{
#if !defined(PR_SET_SECCOMP) || !defined(SECCOMP_MODE_FILTER)
    (void)names;
    (void)actions;
    (void)count;
    (void)default_allow;
    (void)default_errno;
    (void)architecture;
    errno = ENOSYS;
    return -1;
#else
    if (names == NULL || actions == NULL || count == 0U) {
        errno = EINVAL;
        return -1;
    }
    if (default_allow != 0 && default_allow != 1) {
        errno = EINVAL;
        return -1;
    }
    if (default_errno < 0 || default_errno > 4095) {
        errno = EINVAL;
        return -1;
    }
    if (architecture < 0 || architecture > 2) {
        errno = EINVAL;
        return -1;
    }

    uint32_t expected_arch = 0U;
    if (architecture == 1) {
#if defined(__x86_64__)
        expected_arch = AUDIT_ARCH_X86_64;
#elif defined(__aarch64__)
        expected_arch = AUDIT_ARCH_AARCH64;
#elif defined(__i386__)
        expected_arch = AUDIT_ARCH_I386;
#elif defined(__arm__)
        expected_arch = AUDIT_ARCH_ARM;
#else
        errno = ENOTSUP;
        return -1;
#endif
    } else if (architecture == 2) {
#if defined(__x86_64__)
        expected_arch = AUDIT_ARCH_X86_64;
#else
        errno = ENOTSUP;
        return -1;
#endif
    }

    if (prctl(PR_SET_NO_NEW_PRIVS, 1UL, 0UL, 0UL, 0UL) < 0) {
        return -1;
    }

    int32_t numbers[1024];
    int32_t resolved_actions[1024];
    size_t resolved = 0U;
    if (count > sizeof(numbers) / sizeof(numbers[0])) {
        errno = E2BIG;
        return -1;
    }
    for (size_t index = 0U; index < count; ++index) {
        int number = fractald_syscall_number(names[index]);
        if (number < 0) {
            continue;
        }
        numbers[resolved] = (int32_t)number;
        resolved_actions[resolved] = actions[index];
        ++resolved;
    }
    if (resolved == 0U) {
        errno = ENOSYS;
        return -1;
    }

    struct sock_filter filter[4096];
    size_t length = 0U;
    if (expected_arch != 0U) {
        filter[length++] = (struct sock_filter)BPF_STMT(
            BPF_LD | BPF_W | BPF_ABS,
            (unsigned int)offsetof(struct seccomp_data, arch));
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, expected_arch, 1U, 0U);
        filter[length++] = (struct sock_filter)BPF_STMT(
            BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS);
    }
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_LD | BPF_W | BPF_ABS,
        (unsigned int)offsetof(struct seccomp_data, nr));
    for (size_t index = 0U; index < resolved; ++index) {
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K,
            (unsigned int)numbers[index],
            0U,
            1U);
        filter[length++] = (struct sock_filter)BPF_STMT(
            BPF_RET | BPF_K,
            fractald_syscall_action(resolved_actions[index], default_errno));
    }

    if (default_allow == 0) {
#ifdef SYS_execve
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_execve, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_exit
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_exit, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_exit_group
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_exit_group, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_getrlimit
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_getrlimit, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_rt_sigreturn
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_rt_sigreturn, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_sigreturn
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_sigreturn, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_gettimeofday
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_gettimeofday, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_clock_gettime
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_gettime, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_clock_getres
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_getres, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_clock_nanosleep
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_clock_nanosleep, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_nanosleep
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_nanosleep, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
#ifdef SYS_time
        filter[length++] = (struct sock_filter)BPF_JUMP(
            BPF_JMP | BPF_JEQ | BPF_K, SYS_time, 0U, 1U);
        filter[length++] = (struct sock_filter)BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
#endif
    }
    filter[length++] = (struct sock_filter)BPF_STMT(
        BPF_RET | BPF_K,
        default_allow != 0
            ? SECCOMP_RET_ALLOW
            : fractald_syscall_action(0, default_errno));
    struct sock_fprog program = {
        .len = (unsigned short)length,
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
#endif
}

static int parse_id(const char *value, unsigned long *result)
{
    if (value == NULL || *value == '\0') {
        return 0;
    }
    char *end = NULL;
    errno = 0;
    unsigned long parsed = strtoul(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0') {
        return 0;
    }
    *result = parsed;
    return 1;
}

static int resolve_gid(const char *value, gid_t *result)
{
    unsigned long parsed = 0;
    if (parse_id(value, &parsed)) {
        if (parsed > (unsigned long)UINT_MAX) {
            errno = EINVAL;
            return -1;
        }
        *result = (gid_t)parsed;
        return 0;
    }
    struct group *entry = getgrnam(value);
    if (entry == NULL) {
        errno = ENOENT;
        return -1;
    }
    *result = entry->gr_gid;
    return 0;
}

static int resolve_uid(const char *value, uid_t *result, gid_t *primary_group)
{
    unsigned long parsed = 0;
    if (parse_id(value, &parsed)) {
        if (parsed > (unsigned long)UINT_MAX) {
            errno = EINVAL;
            return -1;
        }
        *result = (uid_t)parsed;
        *primary_group = (gid_t)-1;
        return 0;
    }
    struct passwd *entry = getpwnam(value);
    if (entry == NULL) {
        errno = ENOENT;
        return -1;
    }
    *result = entry->pw_uid;
    *primary_group = entry->pw_gid;
    return 0;
}

static int write_proc_value(const char *path, const char *value)
{
    int descriptor = open(path, O_WRONLY | O_CLOEXEC);
    if (descriptor < 0) {
        return -1;
    }
    size_t length = strlen(value);
    size_t offset = 0U;
    while (offset < length) {
        ssize_t written = write(descriptor, value + offset, length - offset);
        if (written < 0) {
            int saved_errno = errno;
            (void)close(descriptor);
            errno = saved_errno;
            return -1;
        }
        if (written == 0) {
            (void)close(descriptor);
            errno = EIO;
            return -1;
        }
        offset += (size_t)written;
    }
    int result = close(descriptor);
    return result;
}

static int append_private_user_mapping(
    char *buffer,
    size_t buffer_size,
    size_t *used,
    unsigned long identifier)
{
    if (identifier == 0UL) {
        return 0;
    }
    size_t cursor = 0U;
    while (cursor < *used) {
        char *end = NULL;
        unsigned long existing = strtoul(buffer + cursor, &end, 10);
        if (end == buffer + cursor) {
            break;
        }
        if (existing == identifier) {
            return 0;
        }
        char *newline = memchr(end, '\n', buffer + *used - end);
        if (newline == NULL) {
            break;
        }
        cursor = (size_t)(newline - buffer) + 1U;
    }
    char mapping[96];
    int length = snprintf(
        mapping,
        sizeof(mapping),
        "%lu %lu 1\n",
        identifier,
        identifier);
    if (length < 0 || (size_t)length >= sizeof(mapping)
        || *used > buffer_size - (size_t)length) {
        errno = E2BIG;
        return -1;
    }
    memcpy(buffer + *used, mapping, (size_t)length);
    *used += (size_t)length;
    return 0;
}

static int append_private_group_mapping(
    char *buffer,
    size_t buffer_size,
    size_t *used,
    unsigned long identifier)
{
    return append_private_user_mapping(buffer, buffer_size, used, identifier);
}

int fractald_enter_private_users(
    int mode,
    const char *user,
    const char *group,
    const char *const *supplementary_groups,
    size_t supplementary_group_count)
{
    if (mode < 1 || mode > 3
        || (supplementary_group_count != 0U && supplementary_groups == NULL)) {
        errno = EINVAL;
        return -1;
    }
    if (supplementary_group_count > NGROUPS_MAX) {
        errno = E2BIG;
        return -1;
    }

    uid_t service_uid = 0;
    gid_t primary_group = (gid_t)-1;
    if (user != NULL && resolve_uid(user, &service_uid, &primary_group) < 0) {
        return -1;
    }
    gid_t service_gid = primary_group == (gid_t)-1 ? 0 : primary_group;
    if (group != NULL && resolve_gid(group, &service_gid) < 0) {
        return -1;
    }
    gid_t *additional_groups = NULL;
    if (supplementary_group_count > 0U) {
        additional_groups = calloc(supplementary_group_count, sizeof(*additional_groups));
        if (additional_groups == NULL) {
            errno = ENOMEM;
            return -1;
        }
        for (size_t index = 0U; index < supplementary_group_count; index++) {
            if (supplementary_groups[index] == NULL
                || resolve_gid(supplementary_groups[index], &additional_groups[index]) < 0) {
                int saved_errno = errno;
                free(additional_groups);
                errno = saved_errno;
                return -1;
            }
        }
    }

    if (unshare(CLONE_NEWUSER) < 0) {
        int saved_errno = errno;
        free(additional_groups);
        errno = saved_errno;
        return -1;
    }

    char uid_mapping[256];
    char *gid_mapping = calloc(65536U, 1U);
    if (gid_mapping == NULL) {
        int saved_errno = ENOMEM;
        free(additional_groups);
        errno = saved_errno;
        return -1;
    }
    size_t uid_used = 0U;
    size_t gid_used = 0U;
    int result = 0;
    if (mode == 1) {
        result = snprintf(uid_mapping, sizeof(uid_mapping), "0 0 1\n");
        if (result < 0 || (size_t)result >= sizeof(uid_mapping)) {
            errno = E2BIG;
            result = -1;
        } else {
            uid_used = (size_t)result;
            if (append_private_user_mapping(
                    uid_mapping,
                    sizeof(uid_mapping),
                    &uid_used,
                    (unsigned long)service_uid) < 0
                || append_private_group_mapping(
                    gid_mapping,
                    65536U,
                    &gid_used,
                    (unsigned long)service_gid) < 0) {
                result = -1;
            }
            if (result == 0) {
                result = snprintf(gid_mapping + gid_used, 65536U - gid_used, "0 0 1\n");
                if (result < 0 || gid_used + (size_t)result >= 65536U) {
                    errno = E2BIG;
                    result = -1;
                } else {
                    gid_used += (size_t)result;
                    for (size_t index = 0U; index < supplementary_group_count; index++) {
                        if (append_private_group_mapping(
                                gid_mapping,
                                65536U,
                                &gid_used,
                                (unsigned long)additional_groups[index]) < 0) {
                            result = -1;
                            break;
                        }
                    }
                }
            }
        }
    } else {
        result = snprintf(uid_mapping, sizeof(uid_mapping), "0 0 65536\n");
        if (result < 0 || (size_t)result >= sizeof(uid_mapping)) {
            errno = E2BIG;
            result = -1;
        } else {
            uid_used = (size_t)result;
            result = snprintf(gid_mapping, 65536U, "0 0 65536\n");
            if (result < 0 || (size_t)result >= 65536U) {
                errno = E2BIG;
                result = -1;
            } else {
                gid_used = (size_t)result;
            }
        }
    }
    if (result == 0) {
        if (write_proc_value("/proc/self/setgroups", "allow\n") < 0
            && errno != ENOENT) {
            result = -1;
        }
    }
    if (result == 0) {
        uid_mapping[uid_used] = '\0';
        gid_mapping[gid_used] = '\0';
        if (write_proc_value("/proc/self/uid_map", uid_mapping) < 0
            || write_proc_value("/proc/self/gid_map", gid_mapping) < 0) {
            result = -1;
        }
    }
    int saved_errno = errno;
    free(gid_mapping);
    free(additional_groups);
    errno = saved_errno;
    return result < 0 ? -1 : 0;
}

int fractald_set_identity(
    const char *user,
    const char *group,
    const char *const *supplementary_groups,
    size_t supplementary_group_count)
{
    if (user == NULL && group == NULL && supplementary_group_count == 0U) {
        return 0;
    }

    if (supplementary_group_count > 0U && supplementary_groups == NULL) {
        errno = EINVAL;
        return -1;
    }

    uid_t uid = (uid_t)-1;
    gid_t primary_group = (gid_t)-1;
    if (user != NULL && resolve_uid(user, &uid, &primary_group) < 0) {
        return -1;
    }

    gid_t gid = primary_group;
    if (group != NULL && resolve_gid(group, &gid) < 0) {
        return -1;
    }

    if (uid != (uid_t)-1 && uid == geteuid()
        && (gid == (gid_t)-1 || gid == getegid())) {
        return 0;
    }
    if (geteuid() != 0) {
        errno = EPERM;
        return -1;
    }
    if (user != NULL && primary_group != (gid_t)-1 && initgroups(user, gid) < 0) {
        return -1;
    }
    if (supplementary_group_count > 0U || supplementary_groups != NULL) {
        if (supplementary_group_count > NGROUPS_MAX) {
            errno = EINVAL;
            return -1;
        }
        gid_t *groups = NULL;
        if (supplementary_group_count > 0U) {
            groups = calloc(supplementary_group_count, sizeof(*groups));
            if (groups == NULL) {
                errno = ENOMEM;
                return -1;
            }
        }
        for (size_t index = 0U; index < supplementary_group_count; ++index) {
            if (supplementary_groups[index] == NULL
                || resolve_gid(supplementary_groups[index], &groups[index]) < 0) {
                int saved_errno = errno;
                free(groups);
                errno = saved_errno;
                return -1;
            }
        }
        int result = setgroups(supplementary_group_count, groups);
        int saved_errno = errno;
        free(groups);
        if (result < 0) {
            errno = saved_errno;
            return -1;
        }
    }
    if (gid != (gid_t)-1 && setgid(gid) < 0) {
        return -1;
    }
    if (uid != (uid_t)-1 && setuid(uid) < 0) {
        return -1;
    }
    return 0;
}

int fractald_chown_path(const char *path, const char *user, const char *group)
{
    if (path == NULL) {
        errno = EINVAL;
        return -1;
    }
    uid_t uid = (uid_t)-1;
    gid_t primary_group = (gid_t)-1;
    if (user != NULL && resolve_uid(user, &uid, &primary_group) < 0) {
        return -1;
    }
    gid_t gid = primary_group;
    if (group != NULL && resolve_gid(group, &gid) < 0) {
        return -1;
    }
    if (uid == (uid_t)-1 && gid == (gid_t)-1) {
        return 0;
    }
    if (uid == geteuid() && (gid == (gid_t)-1 || gid == getegid())) {
        return 0;
    }
    return lchown(path, uid, gid);
}

int fractald_fd_close(int fd)
{
    return close(fd);
}

uint32_t fractald_effective_uid(void)
{
    return (uint32_t)geteuid();
}

static volatile sig_atomic_t fractald_shutdown_flag = 0;

static void fractald_shutdown_handler(int signal_number)
{
    (void)signal_number;
    fractald_shutdown_flag = 1;
}

int fractald_install_shutdown_handlers(void)
{
    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_handler = fractald_shutdown_handler;
    if (sigemptyset(&action.sa_mask) < 0) {
        return -1;
    }
    if (sigaction(SIGTERM, &action, NULL) < 0
        || sigaction(SIGINT, &action, NULL) < 0) {
        return -1;
    }

    struct sigaction ignore;
    memset(&ignore, 0, sizeof(ignore));
    ignore.sa_handler = SIG_IGN;
    if (sigemptyset(&ignore.sa_mask) < 0 || sigaction(SIGPIPE, &ignore, NULL) < 0) {
        return -1;
    }
    return 0;
}

int fractald_shutdown_requested(void)
{
    return fractald_shutdown_flag != 0;
}

int fractald_power_action(int action)
{
#ifndef SYS_reboot
    (void)action;
    errno = ENOSYS;
    return -1;
#else
    if (getpid() != 1) {
        errno = EPERM;
        return -1;
    }

    int command;
    switch (action) {
    case 0:
        command = 0x4321FEDC; /* LINUX_REBOOT_CMD_POWER_OFF */
        break;
    case 1:
        command = 0x01234567; /* LINUX_REBOOT_CMD_RESTART */
        break;
    case 2:
        command = 0xCDEF0123; /* LINUX_REBOOT_CMD_HALT */
        break;
    default:
        errno = EINVAL;
        return -1;
    }
    return (int)syscall(SYS_reboot, 0xfee1dead, 672274793, command, NULL);
#endif
}

int fractald_set_child_subreaper(void)
{
#ifdef PR_SET_CHILD_SUBREAPER
    return prctl(PR_SET_CHILD_SUBREAPER, 1UL, 0UL, 0UL, 0UL);
#else
    errno = ENOSYS;
    return -1;
#endif
}

static int fractald_pid_is_managed(
    pid_t pid,
    const uint32_t *managed_pids,
    size_t managed_count)
{
    if (managed_pids == NULL && managed_count != 0U) {
        errno = EINVAL;
        return -1;
    }
    for (size_t index = 0U; index < managed_count; ++index) {
        if (managed_pids[index] == (uint32_t)pid) {
            return 1;
        }
    }
    return 0;
}

int fractald_reap_untracked_children(const uint32_t *managed_pids, size_t managed_count)
{
    if (getpid() != (pid_t)1) {
        errno = EPERM;
        return -1;
    }
    int reaped = 0;
    for (;;) {
        siginfo_t info;
        memset(&info, 0, sizeof(info));
        if (waitid(P_ALL, 0, &info, WEXITED | WNOHANG | WNOWAIT) < 0) {
            if (errno == ECHILD) {
                return reaped;
            }
            return -1;
        }
        if (info.si_pid == 0) {
            return reaped;
        }
        int managed = fractald_pid_is_managed(info.si_pid, managed_pids, managed_count);
        if (managed < 0) {
            return -1;
        }
        if (managed != 0) {
            return reaped;
        }
        if (waitid(P_PID, info.si_pid, &info, WEXITED) < 0) {
            if (errno == ECHILD || errno == ESRCH) {
                continue;
            }
            return -1;
        }
        if (reaped == INT_MAX) {
            errno = EOVERFLOW;
            return -1;
        }
        ++reaped;
    }
}

int fractald_prepare_activation_fds(const int *sources, size_t count)
{
    if (sources == NULL && count != 0U) {
        errno = EINVAL;
        return -1;
    }
    if (count > (size_t)INT_MAX) {
        errno = E2BIG;
        return -1;
    }

    int *copies = NULL;
    if (count != 0U) {
        copies = (int *)calloc(count, sizeof(*copies));
        if (copies == NULL) {
            errno = ENOMEM;
            return -1;
        }
    }
    for (size_t index = 0U; index < count; ++index) {
        copies[index] = fcntl(sources[index], F_DUPFD_CLOEXEC, 10);
        if (copies[index] < 0) {
            int saved_errno = errno;
            for (size_t close_index = 0U; close_index < index; ++close_index) {
                (void)close(copies[close_index]);
            }
            free(copies);
            errno = saved_errno;
            return -1;
        }
    }
    for (size_t index = 0U; index < count; ++index) {
        int target = 3 + (int)index;
        if (dup2(copies[index], target) < 0) {
            int saved_errno = errno;
            for (size_t close_index = 0U; close_index < count; ++close_index) {
                (void)close(copies[close_index]);
            }
            free(copies);
            errno = saved_errno;
            return -1;
        }
        int flags = fcntl(target, F_GETFD);
        if (flags < 0 || fcntl(target, F_SETFD, flags & ~FD_CLOEXEC) < 0) {
            int saved_errno = errno;
            for (size_t close_index = 0U; close_index < count; ++close_index) {
                (void)close(copies[close_index]);
            }
            free(copies);
            errno = saved_errno;
            return -1;
        }
    }
    for (size_t index = 0U; index < count; ++index) {
        (void)close(copies[index]);
    }
    free(copies);

    char count_text[32];
    char pid_text[32];
    (void)snprintf(count_text, sizeof(count_text), "%zu", count);
    (void)snprintf(pid_text, sizeof(pid_text), "%ld", (long)getpid());
    if (setenv("LISTEN_FDS", count_text, 1) < 0
        || setenv("LISTEN_PID", pid_text, 1) < 0) {
        return -1;
    }
    return 0;
}

int fractald_dup_fd(int fd)
{
    if (fd < 0) {
        errno = EINVAL;
        return -1;
    }
    return fcntl(fd, F_DUPFD_CLOEXEC, 10);
}

int fractald_open_fifo(const char *path, uint32_t mode)
{
    if (path == NULL || *path == '\0') {
        errno = EINVAL;
        return -1;
    }
    struct stat metadata;
    int created = 0;
    if (lstat(path, &metadata) < 0) {
        if (errno != ENOENT || mkfifo(path, (mode_t)mode) < 0) {
            return -1;
        }
        created = 1;
    } else if (!S_ISFIFO(metadata.st_mode)) {
        errno = EEXIST;
        return -1;
    }
    int fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0 && created) {
        int saved_errno = errno;
        (void)unlink(path);
        errno = saved_errno;
    }
    return fd;
}

int fractald_open_special(const char *path, int writable)
{
    if (path == NULL || *path == '\0') {
        errno = EINVAL;
        return -1;
    }
    int flags = (writable != 0 ? O_RDWR : O_RDONLY) | O_NONBLOCK | O_CLOEXEC;
    return open(path, flags);
}

static int fill_unix_address(const char *address, struct sockaddr_un *result, socklen_t *length)
{
    if (address == NULL || *address == '\0' || result == NULL || length == NULL) {
        errno = EINVAL;
        return -1;
    }
    memset(result, 0, sizeof(*result));
    result->sun_family = AF_UNIX;
    size_t name_length = strlen(address);
    if (address[0] == '@') {
        if (name_length == 1U || name_length > sizeof(result->sun_path)) {
            errno = ENAMETOOLONG;
            return -1;
        }
        memcpy(result->sun_path + 1, address + 1, name_length - 1U);
        *length = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + name_length);
        return 0;
    }
    if (name_length >= sizeof(result->sun_path)) {
        errno = ENAMETOOLONG;
        return -1;
    }
    memcpy(result->sun_path, address, name_length + 1U);
    *length = (socklen_t)(offsetof(struct sockaddr_un, sun_path) + name_length + 1U);
    return 0;
}

int fractald_send_unix_datagram(const char *address, const uint8_t *payload, size_t payload_length)
{
    if (payload == NULL && payload_length != 0U) {
        errno = EINVAL;
        return -1;
    }
    struct sockaddr_un socket_address;
    socklen_t address_length = 0;
    if (fill_unix_address(address, &socket_address, &address_length) < 0) {
        return -1;
    }
    int fd = socket(AF_UNIX, SOCK_DGRAM | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return -1;
    }
    ssize_t sent = sendto(
        fd,
        payload,
        payload_length,
        MSG_DONTWAIT,
        (const struct sockaddr *)&socket_address,
        address_length);
    int saved_errno = errno;
    (void)close(fd);
    if (sent < 0) {
        errno = saved_errno;
        return -1;
    }
    if ((size_t)sent != payload_length) {
        errno = EIO;
        return -1;
    }
    return 0;
}

int fractald_enable_socket_credentials(int fd)
{
    if (fd < 0) {
        errno = EINVAL;
        return -1;
    }
    int enabled = 1;
    return setsockopt(fd, SOL_SOCKET, SO_PASSCRED, &enabled, sizeof(enabled));
}

ssize_t fractald_receive_socket_credentials(
    int fd,
    uint8_t *payload,
    size_t payload_length,
    uint32_t *sender_pid)
{
    if (fd < 0 || sender_pid == NULL || (payload == NULL && payload_length != 0U)) {
        errno = EINVAL;
        return -1;
    }

    char control[CMSG_SPACE(sizeof(struct ucred))];
    memset(control, 0, sizeof(control));
    struct iovec iov = {
        .iov_base = payload,
        .iov_len = payload_length,
    };
    struct msghdr message = {
        .msg_iov = &iov,
        .msg_iovlen = 1U,
        .msg_control = control,
        .msg_controllen = sizeof(control),
    };
    ssize_t received = recvmsg(fd, &message, MSG_DONTWAIT);
    if (received < 0) {
        return -1;
    }
    if ((message.msg_flags & (MSG_TRUNC | MSG_CTRUNC)) != 0) {
        errno = EMSGSIZE;
        return -1;
    }

    const struct ucred *credentials = NULL;
    for (struct cmsghdr *header = CMSG_FIRSTHDR(&message);
         header != NULL;
         header = CMSG_NXTHDR(&message, header)) {
        if (header->cmsg_level == SOL_SOCKET
            && header->cmsg_type == SCM_CREDENTIALS
            && header->cmsg_len >= CMSG_LEN(sizeof(struct ucred))) {
            credentials = (const struct ucred *)CMSG_DATA(header);
            break;
        }
    }
    if (credentials == NULL || credentials->pid <= 0) {
        errno = EPROTO;
        return -1;
    }
    *sender_pid = (uint32_t)credentials->pid;
    return received;
}

int fractald_bind_unix_seqpacket(const char *address)
{
    struct sockaddr_un socket_address;
    socklen_t address_length = 0;
    if (fill_unix_address(address, &socket_address, &address_length) < 0) {
        return -1;
    }
    int fd = socket(AF_UNIX, SOCK_SEQPACKET | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
    if (fd < 0) {
        return -1;
    }
    if (bind(fd, (const struct sockaddr *)&socket_address, address_length) < 0
        || listen(fd, 128) < 0) {
        int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return -1;
    }
    return fd;
}

int fractald_open_netlink(int protocol, uint32_t groups)
{
    int fd = socket(AF_NETLINK, SOCK_RAW | SOCK_NONBLOCK | SOCK_CLOEXEC, protocol);
    if (fd < 0) {
        return -1;
    }
    struct sockaddr_nl address;
    memset(&address, 0, sizeof(address));
    address.nl_family = AF_NETLINK;
    address.nl_groups = groups;
    if (bind(fd, (const struct sockaddr *)&address, sizeof(address)) < 0) {
        int saved_errno = errno;
        (void)close(fd);
        errno = saved_errno;
        return -1;
    }
    return fd;
}

int fractald_accept_fd(int fd)
{
    if (fd < 0) {
        errno = EINVAL;
        return -1;
    }
    return accept4(fd, NULL, NULL, SOCK_CLOEXEC);
}

int fractald_set_activation_stdin(void)
{
    if (dup2(3, STDIN_FILENO) < 0) {
        return -1;
    }
    int flags = fcntl(STDIN_FILENO, F_GETFD);
    if (flags < 0 || fcntl(STDIN_FILENO, F_SETFD, flags & ~FD_CLOEXEC) < 0) {
        return -1;
    }
    return 0;
}
