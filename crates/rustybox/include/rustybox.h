#ifndef RUSTYBOX_H
#define RUSTYBOX_H

#include <stddef.h>
#include <stdint.h>

/* Every function returns zero on success or a negative errno value. */
int rustybox_copy_fd(int input_fd, int output_fd, uint64_t *copied);
int rustybox_write_all(int output_fd, const void *buffer, size_t length);
int rustybox_sleep_milliseconds(uint64_t milliseconds);
int rustybox_mkdir_one(const char *path, unsigned int mode);
int rustybox_remove_file(const char *path);
int rustybox_remove_directory(const char *path);
int rustybox_rename_path(const char *source, const char *destination);
int rustybox_make_link(const char *source, const char *destination, int symbolic);
int rustybox_send_signal(int pid, int signal_number);
int rustybox_mount_path(
    const char *source,
    const char *target,
    const char *filesystem,
    const char *options,
    unsigned long flags
);
int rustybox_unmount_path(const char *target, int flags);
int rustybox_enable_swap(const char *path, int flags);
int rustybox_disable_swap(const char *path);
int rustybox_change_root(const char *path);
int rustybox_switch_root(const char *path);
int rustybox_sync(void);
int rustybox_insert_module(const char *path, const char *parameters);
int rustybox_remove_module(const char *name);
int rustybox_uname_field(int field, char *buffer, size_t capacity);

#endif
