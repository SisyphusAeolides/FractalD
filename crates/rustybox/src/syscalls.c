#define _GNU_SOURCE

#include "rustybox.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/mount.h>
#include <signal.h>
#include <stdio.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/swap.h>
#include <sys/syscall.h>
#include <sys/utsname.h>
#include <time.h>
#include <unistd.h>

static int negative_errno(void)
{
    return errno == 0 ? -EIO : -errno;
}

int rustybox_write_all(int output_fd, const void *buffer, size_t length)
{
    const unsigned char *bytes = buffer;
    size_t offset = 0;

    while (offset < length) {
        ssize_t written = write(output_fd, bytes + offset, length - offset);
        if (written > 0) {
            offset += (size_t) written;
            continue;
        }
        if (written < 0 && errno == EINTR) {
            continue;
        }
        return negative_errno();
    }
    return 0;
}

int rustybox_copy_fd(int input_fd, int output_fd, uint64_t *copied)
{
    unsigned char buffer[64 * 1024];
    uint64_t total = 0;

    for (;;) {
        ssize_t count = read(input_fd, buffer, sizeof(buffer));
        if (count == 0) {
            if (copied != NULL) {
                *copied = total;
            }
            return 0;
        }
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            return negative_errno();
        }
        if (rustybox_write_all(output_fd, buffer, (size_t) count) != 0) {
            return negative_errno();
        }
        total += (uint64_t) count;
    }
}

int rustybox_sleep_milliseconds(uint64_t milliseconds)
{
    struct timespec remaining = {
        .tv_sec = (time_t) (milliseconds / 1000),
        .tv_nsec = (long) ((milliseconds % 1000) * 1000000),
    };

    while (nanosleep(&remaining, &remaining) != 0) {
        if (errno == EINTR) {
            continue;
        }
        return negative_errno();
    }
    return 0;
}

int rustybox_mkdir_one(const char *path, unsigned int mode)
{
    return mkdir(path, (mode_t) mode) == 0 ? 0 : negative_errno();
}

int rustybox_remove_file(const char *path)
{
    return unlink(path) == 0 ? 0 : negative_errno();
}

int rustybox_remove_directory(const char *path)
{
    return rmdir(path) == 0 ? 0 : negative_errno();
}

int rustybox_rename_path(const char *source, const char *destination)
{
    return rename(source, destination) == 0 ? 0 : negative_errno();
}

int rustybox_make_link(const char *source, const char *destination, int symbolic)
{
    int result = symbolic ? symlink(source, destination) : link(source, destination);
    return result == 0 ? 0 : negative_errno();
}

int rustybox_send_signal(int pid, int signal_number)
{
    return kill((pid_t) pid, signal_number) == 0 ? 0 : negative_errno();
}

int rustybox_mount_path(
    const char *source,
    const char *target,
    const char *filesystem,
    const char *options,
    unsigned long flags
)
{
    const char *mount_source = source != NULL && source[0] != '\0' ? source : NULL;
    const char *mount_filesystem = filesystem != NULL && filesystem[0] != '\0' ? filesystem : NULL;
    const void *mount_options = options != NULL && options[0] != '\0' ? options : NULL;

    return mount(mount_source, target, mount_filesystem, flags, mount_options) == 0
        ? 0
        : negative_errno();
}

int rustybox_unmount_path(const char *target, int flags)
{
    return umount2(target, flags) == 0 ? 0 : negative_errno();
}

int rustybox_enable_swap(const char *path, int flags)
{
    return swapon(path, flags) == 0 ? 0 : negative_errno();
}

int rustybox_disable_swap(const char *path)
{
    return swapoff(path) == 0 ? 0 : negative_errno();
}

int rustybox_change_root(const char *path)
{
    return chroot(path) == 0 ? 0 : negative_errno();
}

int rustybox_switch_root(const char *path)
{
    char old_root[PATH_MAX];
    char target[PATH_MAX];
    static const char *const mounts[] = {"/proc", "/sys", "/dev", "/run"};
    size_t index;

    if (path == NULL || path[0] != '/') {
        errno = EINVAL;
        return negative_errno();
    }
    if (snprintf(old_root, sizeof(old_root), "%s/.fractald-old-root", path)
        >= (int) sizeof(old_root)) {
        errno = ENAMETOOLONG;
        return negative_errno();
    }
    if (mkdir(old_root, 0700) != 0 && errno != EEXIST) {
        return negative_errno();
    }
    for (index = 0; index < sizeof(mounts) / sizeof(mounts[0]); index++) {
        if (snprintf(target, sizeof(target), "%s%s", path, mounts[index])
            >= (int) sizeof(target)) {
            errno = ENAMETOOLONG;
            return negative_errno();
        }
        if (mkdir(target, 0755) != 0 && errno != EEXIST) {
            return negative_errno();
        }
        if (mount(mounts[index], target, NULL, MS_MOVE, NULL) != 0 &&
            errno != ENOENT && errno != EINVAL && errno != ENOTDIR) {
            return negative_errno();
        }
    }
    if (syscall(SYS_pivot_root, path, old_root) != 0) {
        return negative_errno();
    }
    if (chdir("/") != 0) {
        return negative_errno();
    }
    if (umount2("/.fractald-old-root", MNT_DETACH) != 0) {
        return negative_errno();
    }
    if (rmdir("/.fractald-old-root") != 0) {
        return negative_errno();
    }
    return 0;
}

int rustybox_sync(void)
{
    sync();
    return 0;
}

int rustybox_insert_module(const char *path, const char *parameters)
{
    int file_descriptor;
    long result;

    if (path == NULL || parameters == NULL) {
        errno = EINVAL;
        return negative_errno();
    }
    file_descriptor = open(path, O_RDONLY | O_CLOEXEC);
    if (file_descriptor < 0) {
        return negative_errno();
    }
#ifdef SYS_finit_module
    result = syscall(SYS_finit_module, file_descriptor, parameters, 0U);
#else
    errno = ENOSYS;
    result = -1;
#endif
    close(file_descriptor);
    return result == 0 ? 0 : negative_errno();
}

int rustybox_remove_module(const char *name)
{
    if (name == NULL || name[0] == '\0') {
        errno = EINVAL;
        return negative_errno();
    }
#ifdef SYS_delete_module
    return syscall(SYS_delete_module, name, 0U) == 0 ? 0 : negative_errno();
#else
    errno = ENOSYS;
    return negative_errno();
#endif
}

int rustybox_uname_field(int field, char *buffer, size_t capacity)
{
    struct utsname values;
    const char *value;

    if (buffer == NULL || capacity == 0 || uname(&values) != 0) {
        return errno == 0 ? -EINVAL : negative_errno();
    }
    switch (field) {
    case 0:
        value = values.sysname;
        break;
    case 1:
        value = values.nodename;
        break;
    case 2:
        value = values.release;
        break;
    case 3:
        value = values.version;
        break;
    case 4:
        value = values.machine;
        break;
    default:
        return -EINVAL;
    }
    if (snprintf(buffer, capacity, "%s", value) < 0) {
        return negative_errno();
    }
    buffer[capacity - 1] = '\0';
    return 0;
}
