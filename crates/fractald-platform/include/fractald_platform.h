#ifndef FRACTALD_PLATFORM_H
#define FRACTALD_PLATFORM_H

#include <stdint.h>
#include <stddef.h>
#include <sys/types.h>

struct fractald_device_rule {
    uint32_t device_type;
    uint32_t major;
    uint32_t minor;
    uint32_t access;
};

int fractald_pidfd_open(uint32_t pid);
int fractald_pidfd_send_signal(int pidfd, int signal_number);
int fractald_pidfd_wait(int pidfd, int nohang, int *code, int *value);
int fractald_set_process_group(void);
int fractald_set_parent_death_signal(int signal_number, uint32_t expected_parent);
int fractald_set_signal_disposition(int signal_number, int ignored);
int fractald_set_no_new_privileges(void);
int fractald_set_memory_deny_write_execute(void);
int fractald_restrict_realtime(void);
int fractald_restrict_suid_sgid(void);
int fractald_restrict_namespaces(uint32_t allowed);
int fractald_set_umask(uint32_t mask);
int fractald_set_nice(int value);
int fractald_set_oom_score_adjust(int value);
int fractald_set_nofile_limit(uint64_t soft, uint64_t hard);
int fractald_set_memlock_limit(uint64_t soft, uint64_t hard);
int fractald_set_nproc_limit(uint64_t soft, uint64_t hard);
int fractald_enter_private_tmp(int mode, const char *tmp_path, const char *var_tmp_path);
int fractald_enter_private_devices(void);
int fractald_enter_private_users(
    int mode,
    const char *user,
    const char *group,
    const char *const *supplementary_groups,
    size_t supplementary_group_count);
int fractald_attach_device_filter(
    const char *cgroup_path,
    int default_allow,
    const struct fractald_device_rule *rules,
    size_t rule_count);
int fractald_detach_device_filter(const char *cgroup_path);
int fractald_enter_private_ipc(void);
int fractald_enter_private_network(void);
int fractald_enter_private_uts_namespace(void);
int fractald_install_hostname_filter(void);
int fractald_enter_private_uts(void);
int fractald_lock_personality(void);
int fractald_protect_clock(void);
int fractald_apply_filesystem_protection(
    int protect_system,
    int protect_home,
    int protect_proc,
    int proc_subset);
int fractald_make_path_writable(const char *path);
int fractald_make_path_read_only(const char *path);
int fractald_make_path_inaccessible(const char *path);
int fractald_signal_process(uint32_t pid, int signal_number);
int fractald_signal_process_group(uint32_t pid, int signal_number);
int fractald_set_identity(
    const char *user,
    const char *group,
    const char *const *supplementary_groups,
    size_t supplementary_group_count);
int fractald_set_keep_capabilities(void);
int fractald_drop_capability_bounding_set(uint64_t allowed);
int fractald_apply_capability_bounding_set(uint64_t allowed);
int fractald_apply_ambient_capabilities(uint64_t allowed);
int fractald_restrict_address_families(uint64_t allowed);
int fractald_install_system_call_filter(
    const char *const *names,
    const int32_t *actions,
    size_t count,
    int default_allow,
    int default_errno,
    int architecture);
int fractald_chown_path(const char *path, const char *user, const char *group);
int fractald_fd_close(int fd);
uint32_t fractald_effective_uid(void);
int fractald_prepare_activation_fds(const int *sources, size_t count);
int fractald_dup_fd(int fd);
int fractald_open_fifo(const char *path, uint32_t mode);
int fractald_open_special(const char *path, int writable);
int fractald_bind_unix_seqpacket(const char *address);
int fractald_send_unix_datagram(const char *address, const uint8_t *payload, size_t payload_length);
int fractald_enable_socket_credentials(int fd);
ssize_t fractald_receive_socket_credentials(
    int fd,
    uint8_t *payload,
    size_t payload_length,
    uint32_t *sender_pid);
int fractald_open_netlink(int protocol, uint32_t groups);
int fractald_accept_fd(int fd);
int fractald_set_activation_stdin(void);
int fractald_install_shutdown_handlers(void);
int fractald_shutdown_requested(void);
int fractald_power_action(int action);
int fractald_prepare_pid1_mounts(void);
int fractald_set_child_subreaper(void);
int fractald_reap_untracked_children(const uint32_t *managed_pids, size_t managed_count);

#endif
