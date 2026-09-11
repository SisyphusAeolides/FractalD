#ifdef SYS_read
    if (strcmp(name, "read") == 0) {
        return SYS_read;
    }
#endif
#ifdef SYS_write
    if (strcmp(name, "write") == 0) {
        return SYS_write;
    }
#endif
#ifdef SYS_open
    if (strcmp(name, "open") == 0) {
        return SYS_open;
    }
#endif
#ifdef SYS_close
    if (strcmp(name, "close") == 0) {
        return SYS_close;
    }
#endif
#ifdef SYS_stat
    if (strcmp(name, "stat") == 0) {
        return SYS_stat;
    }
#endif
#ifdef SYS_fstat
    if (strcmp(name, "fstat") == 0) {
        return SYS_fstat;
    }
#endif
#ifdef SYS_lstat
    if (strcmp(name, "lstat") == 0) {
        return SYS_lstat;
    }
#endif
#ifdef SYS_poll
    if (strcmp(name, "poll") == 0) {
        return SYS_poll;
    }
#endif
#ifdef SYS_lseek
    if (strcmp(name, "lseek") == 0) {
        return SYS_lseek;
    }
#endif
#ifdef SYS_mmap
    if (strcmp(name, "mmap") == 0) {
        return SYS_mmap;
    }
#endif
#ifdef SYS_mprotect
    if (strcmp(name, "mprotect") == 0) {
        return SYS_mprotect;
    }
#endif
#ifdef SYS_munmap
    if (strcmp(name, "munmap") == 0) {
        return SYS_munmap;
    }
#endif
#ifdef SYS_brk
    if (strcmp(name, "brk") == 0) {
        return SYS_brk;
    }
#endif
#ifdef SYS_rt_sigaction
    if (strcmp(name, "rt_sigaction") == 0) {
        return SYS_rt_sigaction;
    }
#endif
#ifdef SYS_rt_sigprocmask
    if (strcmp(name, "rt_sigprocmask") == 0) {
        return SYS_rt_sigprocmask;
    }
#endif
#ifdef SYS_rt_sigreturn
    if (strcmp(name, "rt_sigreturn") == 0) {
        return SYS_rt_sigreturn;
    }
#endif
#ifdef SYS_ioctl
    if (strcmp(name, "ioctl") == 0) {
        return SYS_ioctl;
    }
#endif
#ifdef SYS_pread64
    if (strcmp(name, "pread64") == 0) {
        return SYS_pread64;
    }
#endif
#ifdef SYS_pwrite64
    if (strcmp(name, "pwrite64") == 0) {
        return SYS_pwrite64;
    }
#endif
#ifdef SYS_readv
    if (strcmp(name, "readv") == 0) {
        return SYS_readv;
    }
#endif
#ifdef SYS_writev
    if (strcmp(name, "writev") == 0) {
        return SYS_writev;
    }
#endif
#ifdef SYS_access
    if (strcmp(name, "access") == 0) {
        return SYS_access;
    }
#endif
#ifdef SYS_pipe
    if (strcmp(name, "pipe") == 0) {
        return SYS_pipe;
    }
#endif
#ifdef SYS_select
    if (strcmp(name, "select") == 0) {
        return SYS_select;
    }
#endif
#ifdef SYS_sched_yield
    if (strcmp(name, "sched_yield") == 0) {
        return SYS_sched_yield;
    }
#endif
#ifdef SYS_mremap
    if (strcmp(name, "mremap") == 0) {
        return SYS_mremap;
    }
#endif
#ifdef SYS_msync
    if (strcmp(name, "msync") == 0) {
        return SYS_msync;
    }
#endif
#ifdef SYS_mincore
    if (strcmp(name, "mincore") == 0) {
        return SYS_mincore;
    }
#endif
#ifdef SYS_madvise
    if (strcmp(name, "madvise") == 0) {
        return SYS_madvise;
    }
#endif
#ifdef SYS_shmget
    if (strcmp(name, "shmget") == 0) {
        return SYS_shmget;
    }
#endif
#ifdef SYS_shmat
    if (strcmp(name, "shmat") == 0) {
        return SYS_shmat;
    }
#endif
#ifdef SYS_shmctl
    if (strcmp(name, "shmctl") == 0) {
        return SYS_shmctl;
    }
#endif
#ifdef SYS_dup
    if (strcmp(name, "dup") == 0) {
        return SYS_dup;
    }
#endif
#ifdef SYS_dup2
    if (strcmp(name, "dup2") == 0) {
        return SYS_dup2;
    }
#endif
#ifdef SYS_pause
    if (strcmp(name, "pause") == 0) {
        return SYS_pause;
    }
#endif
#ifdef SYS_nanosleep
    if (strcmp(name, "nanosleep") == 0) {
        return SYS_nanosleep;
    }
#endif
#ifdef SYS_getitimer
    if (strcmp(name, "getitimer") == 0) {
        return SYS_getitimer;
    }
#endif
#ifdef SYS_alarm
    if (strcmp(name, "alarm") == 0) {
        return SYS_alarm;
    }
#endif
#ifdef SYS_setitimer
    if (strcmp(name, "setitimer") == 0) {
        return SYS_setitimer;
    }
#endif
#ifdef SYS_getpid
    if (strcmp(name, "getpid") == 0) {
        return SYS_getpid;
    }
#endif
#ifdef SYS_sendfile
    if (strcmp(name, "sendfile") == 0) {
        return SYS_sendfile;
    }
#endif
#ifdef SYS_socket
    if (strcmp(name, "socket") == 0) {
        return SYS_socket;
    }
#endif
#ifdef SYS_connect
    if (strcmp(name, "connect") == 0) {
        return SYS_connect;
    }
#endif
#ifdef SYS_accept
    if (strcmp(name, "accept") == 0) {
        return SYS_accept;
    }
#endif
#ifdef SYS_sendto
    if (strcmp(name, "sendto") == 0) {
        return SYS_sendto;
    }
#endif
#ifdef SYS_recvfrom
    if (strcmp(name, "recvfrom") == 0) {
        return SYS_recvfrom;
    }
#endif
#ifdef SYS_sendmsg
    if (strcmp(name, "sendmsg") == 0) {
        return SYS_sendmsg;
    }
#endif
#ifdef SYS_recvmsg
    if (strcmp(name, "recvmsg") == 0) {
        return SYS_recvmsg;
    }
#endif
#ifdef SYS_shutdown
    if (strcmp(name, "shutdown") == 0) {
        return SYS_shutdown;
    }
#endif
#ifdef SYS_bind
    if (strcmp(name, "bind") == 0) {
        return SYS_bind;
    }
#endif
#ifdef SYS_listen
    if (strcmp(name, "listen") == 0) {
        return SYS_listen;
    }
#endif
#ifdef SYS_getsockname
    if (strcmp(name, "getsockname") == 0) {
        return SYS_getsockname;
    }
#endif
#ifdef SYS_getpeername
    if (strcmp(name, "getpeername") == 0) {
        return SYS_getpeername;
    }
#endif
#ifdef SYS_socketpair
    if (strcmp(name, "socketpair") == 0) {
        return SYS_socketpair;
    }
#endif
#ifdef SYS_setsockopt
    if (strcmp(name, "setsockopt") == 0) {
        return SYS_setsockopt;
    }
#endif
#ifdef SYS_getsockopt
    if (strcmp(name, "getsockopt") == 0) {
        return SYS_getsockopt;
    }
#endif
#ifdef SYS_clone
    if (strcmp(name, "clone") == 0) {
        return SYS_clone;
    }
#endif
#ifdef SYS_fork
    if (strcmp(name, "fork") == 0) {
        return SYS_fork;
    }
#endif
#ifdef SYS_vfork
    if (strcmp(name, "vfork") == 0) {
        return SYS_vfork;
    }
#endif
#ifdef SYS_execve
    if (strcmp(name, "execve") == 0) {
        return SYS_execve;
    }
#endif
#ifdef SYS_exit
    if (strcmp(name, "exit") == 0) {
        return SYS_exit;
    }
#endif
#ifdef SYS_wait4
    if (strcmp(name, "wait4") == 0) {
        return SYS_wait4;
    }
#endif
#ifdef SYS_kill
    if (strcmp(name, "kill") == 0) {
        return SYS_kill;
    }
#endif
#ifdef SYS_uname
    if (strcmp(name, "uname") == 0) {
        return SYS_uname;
    }
#endif
#ifdef SYS_semget
    if (strcmp(name, "semget") == 0) {
        return SYS_semget;
    }
#endif
#ifdef SYS_semop
    if (strcmp(name, "semop") == 0) {
        return SYS_semop;
    }
#endif
#ifdef SYS_semctl
    if (strcmp(name, "semctl") == 0) {
        return SYS_semctl;
    }
#endif
#ifdef SYS_shmdt
    if (strcmp(name, "shmdt") == 0) {
        return SYS_shmdt;
    }
#endif
#ifdef SYS_msgget
    if (strcmp(name, "msgget") == 0) {
        return SYS_msgget;
    }
#endif
#ifdef SYS_msgsnd
    if (strcmp(name, "msgsnd") == 0) {
        return SYS_msgsnd;
    }
#endif
#ifdef SYS_msgrcv
    if (strcmp(name, "msgrcv") == 0) {
        return SYS_msgrcv;
    }
#endif
#ifdef SYS_msgctl
    if (strcmp(name, "msgctl") == 0) {
        return SYS_msgctl;
    }
#endif
#ifdef SYS_fcntl
    if (strcmp(name, "fcntl") == 0) {
        return SYS_fcntl;
    }
#endif
#ifdef SYS_flock
    if (strcmp(name, "flock") == 0) {
        return SYS_flock;
    }
#endif
#ifdef SYS_fsync
    if (strcmp(name, "fsync") == 0) {
        return SYS_fsync;
    }
#endif
#ifdef SYS_fdatasync
    if (strcmp(name, "fdatasync") == 0) {
        return SYS_fdatasync;
    }
#endif
#ifdef SYS_truncate
    if (strcmp(name, "truncate") == 0) {
        return SYS_truncate;
    }
#endif
#ifdef SYS_ftruncate
    if (strcmp(name, "ftruncate") == 0) {
        return SYS_ftruncate;
    }
#endif
#ifdef SYS_getdents
    if (strcmp(name, "getdents") == 0) {
        return SYS_getdents;
    }
#endif
#ifdef SYS_getcwd
    if (strcmp(name, "getcwd") == 0) {
        return SYS_getcwd;
    }
#endif
#ifdef SYS_chdir
    if (strcmp(name, "chdir") == 0) {
        return SYS_chdir;
    }
#endif
#ifdef SYS_fchdir
    if (strcmp(name, "fchdir") == 0) {
        return SYS_fchdir;
    }
#endif
#ifdef SYS_rename
    if (strcmp(name, "rename") == 0) {
        return SYS_rename;
    }
#endif
#ifdef SYS_mkdir
    if (strcmp(name, "mkdir") == 0) {
        return SYS_mkdir;
    }
#endif
#ifdef SYS_rmdir
    if (strcmp(name, "rmdir") == 0) {
        return SYS_rmdir;
    }
#endif
#ifdef SYS_creat
    if (strcmp(name, "creat") == 0) {
        return SYS_creat;
    }
#endif
#ifdef SYS_link
    if (strcmp(name, "link") == 0) {
        return SYS_link;
    }
#endif
#ifdef SYS_unlink
    if (strcmp(name, "unlink") == 0) {
        return SYS_unlink;
    }
#endif
#ifdef SYS_symlink
    if (strcmp(name, "symlink") == 0) {
        return SYS_symlink;
    }
#endif
#ifdef SYS_readlink
    if (strcmp(name, "readlink") == 0) {
        return SYS_readlink;
    }
#endif
#ifdef SYS_chmod
    if (strcmp(name, "chmod") == 0) {
        return SYS_chmod;
    }
#endif
#ifdef SYS_fchmod
    if (strcmp(name, "fchmod") == 0) {
        return SYS_fchmod;
    }
#endif
#ifdef SYS_chown
    if (strcmp(name, "chown") == 0) {
        return SYS_chown;
    }
#endif
#ifdef SYS_fchown
    if (strcmp(name, "fchown") == 0) {
        return SYS_fchown;
    }
#endif
#ifdef SYS_lchown
    if (strcmp(name, "lchown") == 0) {
        return SYS_lchown;
    }
#endif
#ifdef SYS_umask
    if (strcmp(name, "umask") == 0) {
        return SYS_umask;
    }
#endif
#ifdef SYS_gettimeofday
    if (strcmp(name, "gettimeofday") == 0) {
        return SYS_gettimeofday;
    }
#endif
#ifdef SYS_getrlimit
    if (strcmp(name, "getrlimit") == 0) {
        return SYS_getrlimit;
    }
#endif
#ifdef SYS_getrusage
    if (strcmp(name, "getrusage") == 0) {
        return SYS_getrusage;
    }
#endif
#ifdef SYS_sysinfo
    if (strcmp(name, "sysinfo") == 0) {
        return SYS_sysinfo;
    }
#endif
#ifdef SYS_times
    if (strcmp(name, "times") == 0) {
        return SYS_times;
    }
#endif
#ifdef SYS_ptrace
    if (strcmp(name, "ptrace") == 0) {
        return SYS_ptrace;
    }
#endif
#ifdef SYS_getuid
    if (strcmp(name, "getuid") == 0) {
        return SYS_getuid;
    }
#endif
#ifdef SYS_syslog
    if (strcmp(name, "syslog") == 0) {
        return SYS_syslog;
    }
#endif
#ifdef SYS_getgid
    if (strcmp(name, "getgid") == 0) {
        return SYS_getgid;
    }
#endif
#ifdef SYS_setuid
    if (strcmp(name, "setuid") == 0) {
        return SYS_setuid;
    }
#endif
#ifdef SYS_setgid
    if (strcmp(name, "setgid") == 0) {
        return SYS_setgid;
    }
#endif
#ifdef SYS_geteuid
    if (strcmp(name, "geteuid") == 0) {
        return SYS_geteuid;
    }
#endif
#ifdef SYS_getegid
    if (strcmp(name, "getegid") == 0) {
        return SYS_getegid;
    }
#endif
#ifdef SYS_setpgid
    if (strcmp(name, "setpgid") == 0) {
        return SYS_setpgid;
    }
#endif
#ifdef SYS_getppid
    if (strcmp(name, "getppid") == 0) {
        return SYS_getppid;
    }
#endif
#ifdef SYS_getpgrp
    if (strcmp(name, "getpgrp") == 0) {
        return SYS_getpgrp;
    }
#endif
#ifdef SYS_setsid
    if (strcmp(name, "setsid") == 0) {
        return SYS_setsid;
    }
#endif
#ifdef SYS_setreuid
    if (strcmp(name, "setreuid") == 0) {
        return SYS_setreuid;
    }
#endif
#ifdef SYS_setregid
    if (strcmp(name, "setregid") == 0) {
        return SYS_setregid;
    }
#endif
#ifdef SYS_getgroups
    if (strcmp(name, "getgroups") == 0) {
        return SYS_getgroups;
    }
#endif
#ifdef SYS_setgroups
    if (strcmp(name, "setgroups") == 0) {
        return SYS_setgroups;
    }
#endif
#ifdef SYS_setresuid
    if (strcmp(name, "setresuid") == 0) {
        return SYS_setresuid;
    }
#endif
#ifdef SYS_getresuid
    if (strcmp(name, "getresuid") == 0) {
        return SYS_getresuid;
    }
#endif
#ifdef SYS_setresgid
    if (strcmp(name, "setresgid") == 0) {
        return SYS_setresgid;
    }
#endif
#ifdef SYS_getresgid
    if (strcmp(name, "getresgid") == 0) {
        return SYS_getresgid;
    }
#endif
#ifdef SYS_getpgid
    if (strcmp(name, "getpgid") == 0) {
        return SYS_getpgid;
    }
#endif
#ifdef SYS_setfsuid
    if (strcmp(name, "setfsuid") == 0) {
        return SYS_setfsuid;
    }
#endif
#ifdef SYS_setfsgid
    if (strcmp(name, "setfsgid") == 0) {
        return SYS_setfsgid;
    }
#endif
#ifdef SYS_getsid
    if (strcmp(name, "getsid") == 0) {
        return SYS_getsid;
    }
#endif
#ifdef SYS_capget
    if (strcmp(name, "capget") == 0) {
        return SYS_capget;
    }
#endif
#ifdef SYS_capset
    if (strcmp(name, "capset") == 0) {
        return SYS_capset;
    }
#endif
#ifdef SYS_rt_sigpending
    if (strcmp(name, "rt_sigpending") == 0) {
        return SYS_rt_sigpending;
    }
#endif
#ifdef SYS_rt_sigtimedwait
    if (strcmp(name, "rt_sigtimedwait") == 0) {
        return SYS_rt_sigtimedwait;
    }
#endif
#ifdef SYS_rt_sigqueueinfo
    if (strcmp(name, "rt_sigqueueinfo") == 0) {
        return SYS_rt_sigqueueinfo;
    }
#endif
#ifdef SYS_rt_sigsuspend
    if (strcmp(name, "rt_sigsuspend") == 0) {
        return SYS_rt_sigsuspend;
    }
#endif
#ifdef SYS_sigaltstack
    if (strcmp(name, "sigaltstack") == 0) {
        return SYS_sigaltstack;
    }
#endif
#ifdef SYS_utime
    if (strcmp(name, "utime") == 0) {
        return SYS_utime;
    }
#endif
#ifdef SYS_mknod
    if (strcmp(name, "mknod") == 0) {
        return SYS_mknod;
    }
#endif
#ifdef SYS_uselib
    if (strcmp(name, "uselib") == 0) {
        return SYS_uselib;
    }
#endif
#ifdef SYS_personality
    if (strcmp(name, "personality") == 0) {
        return SYS_personality;
    }
#endif
#ifdef SYS_ustat
    if (strcmp(name, "ustat") == 0) {
        return SYS_ustat;
    }
#endif
#ifdef SYS_statfs
    if (strcmp(name, "statfs") == 0) {
        return SYS_statfs;
    }
#endif
#ifdef SYS_fstatfs
    if (strcmp(name, "fstatfs") == 0) {
        return SYS_fstatfs;
    }
#endif
#ifdef SYS_sysfs
    if (strcmp(name, "sysfs") == 0) {
        return SYS_sysfs;
    }
#endif
#ifdef SYS_getpriority
    if (strcmp(name, "getpriority") == 0) {
        return SYS_getpriority;
    }
#endif
#ifdef SYS_setpriority
    if (strcmp(name, "setpriority") == 0) {
        return SYS_setpriority;
    }
#endif
#ifdef SYS_sched_setparam
    if (strcmp(name, "sched_setparam") == 0) {
        return SYS_sched_setparam;
    }
#endif
#ifdef SYS_sched_getparam
    if (strcmp(name, "sched_getparam") == 0) {
        return SYS_sched_getparam;
    }
#endif
#ifdef SYS_sched_setscheduler
    if (strcmp(name, "sched_setscheduler") == 0) {
        return SYS_sched_setscheduler;
    }
#endif
#ifdef SYS_sched_getscheduler
    if (strcmp(name, "sched_getscheduler") == 0) {
        return SYS_sched_getscheduler;
    }
#endif
#ifdef SYS_sched_get_priority_max
    if (strcmp(name, "sched_get_priority_max") == 0) {
        return SYS_sched_get_priority_max;
    }
#endif
#ifdef SYS_sched_get_priority_min
    if (strcmp(name, "sched_get_priority_min") == 0) {
        return SYS_sched_get_priority_min;
    }
#endif
#ifdef SYS_sched_rr_get_interval
    if (strcmp(name, "sched_rr_get_interval") == 0) {
        return SYS_sched_rr_get_interval;
    }
#endif
#ifdef SYS_mlock
    if (strcmp(name, "mlock") == 0) {
        return SYS_mlock;
    }
#endif
#ifdef SYS_munlock
    if (strcmp(name, "munlock") == 0) {
        return SYS_munlock;
    }
#endif
#ifdef SYS_mlockall
    if (strcmp(name, "mlockall") == 0) {
        return SYS_mlockall;
    }
#endif
#ifdef SYS_munlockall
    if (strcmp(name, "munlockall") == 0) {
        return SYS_munlockall;
    }
#endif
#ifdef SYS_vhangup
    if (strcmp(name, "vhangup") == 0) {
        return SYS_vhangup;
    }
#endif
#ifdef SYS_modify_ldt
    if (strcmp(name, "modify_ldt") == 0) {
        return SYS_modify_ldt;
    }
#endif
#ifdef SYS_pivot_root
    if (strcmp(name, "pivot_root") == 0) {
        return SYS_pivot_root;
    }
#endif
#ifdef SYS__sysctl
    if (strcmp(name, "_sysctl") == 0) {
        return SYS__sysctl;
    }
#endif
#ifdef SYS_prctl
    if (strcmp(name, "prctl") == 0) {
        return SYS_prctl;
    }
#endif
#ifdef SYS_arch_prctl
    if (strcmp(name, "arch_prctl") == 0) {
        return SYS_arch_prctl;
    }
#endif
#ifdef SYS_adjtimex
    if (strcmp(name, "adjtimex") == 0) {
        return SYS_adjtimex;
    }
#endif
#ifdef SYS_setrlimit
    if (strcmp(name, "setrlimit") == 0) {
        return SYS_setrlimit;
    }
#endif
#ifdef SYS_chroot
    if (strcmp(name, "chroot") == 0) {
        return SYS_chroot;
    }
#endif
#ifdef SYS_sync
    if (strcmp(name, "sync") == 0) {
        return SYS_sync;
    }
#endif
#ifdef SYS_acct
    if (strcmp(name, "acct") == 0) {
        return SYS_acct;
    }
#endif
#ifdef SYS_settimeofday
    if (strcmp(name, "settimeofday") == 0) {
        return SYS_settimeofday;
    }
#endif
#ifdef SYS_mount
    if (strcmp(name, "mount") == 0) {
        return SYS_mount;
    }
#endif
#ifdef SYS_umount2
    if (strcmp(name, "umount2") == 0) {
        return SYS_umount2;
    }
#endif
#ifdef SYS_swapon
    if (strcmp(name, "swapon") == 0) {
        return SYS_swapon;
    }
#endif
#ifdef SYS_swapoff
    if (strcmp(name, "swapoff") == 0) {
        return SYS_swapoff;
    }
#endif
#ifdef SYS_reboot
    if (strcmp(name, "reboot") == 0) {
        return SYS_reboot;
    }
#endif
#ifdef SYS_sethostname
    if (strcmp(name, "sethostname") == 0) {
        return SYS_sethostname;
    }
#endif
#ifdef SYS_setdomainname
    if (strcmp(name, "setdomainname") == 0) {
        return SYS_setdomainname;
    }
#endif
#ifdef SYS_iopl
    if (strcmp(name, "iopl") == 0) {
        return SYS_iopl;
    }
#endif
#ifdef SYS_ioperm
    if (strcmp(name, "ioperm") == 0) {
        return SYS_ioperm;
    }
#endif
#ifdef SYS_create_module
    if (strcmp(name, "create_module") == 0) {
        return SYS_create_module;
    }
#endif
#ifdef SYS_init_module
    if (strcmp(name, "init_module") == 0) {
        return SYS_init_module;
    }
#endif
#ifdef SYS_delete_module
    if (strcmp(name, "delete_module") == 0) {
        return SYS_delete_module;
    }
#endif
#ifdef SYS_get_kernel_syms
    if (strcmp(name, "get_kernel_syms") == 0) {
        return SYS_get_kernel_syms;
    }
#endif
#ifdef SYS_query_module
    if (strcmp(name, "query_module") == 0) {
        return SYS_query_module;
    }
#endif
#ifdef SYS_quotactl
    if (strcmp(name, "quotactl") == 0) {
        return SYS_quotactl;
    }
#endif
#ifdef SYS_nfsservctl
    if (strcmp(name, "nfsservctl") == 0) {
        return SYS_nfsservctl;
    }
#endif
#ifdef SYS_getpmsg
    if (strcmp(name, "getpmsg") == 0) {
        return SYS_getpmsg;
    }
#endif
#ifdef SYS_putpmsg
    if (strcmp(name, "putpmsg") == 0) {
        return SYS_putpmsg;
    }
#endif
#ifdef SYS_afs_syscall
    if (strcmp(name, "afs_syscall") == 0) {
        return SYS_afs_syscall;
    }
#endif
#ifdef SYS_tuxcall
    if (strcmp(name, "tuxcall") == 0) {
        return SYS_tuxcall;
    }
#endif
#ifdef SYS_security
    if (strcmp(name, "security") == 0) {
        return SYS_security;
    }
#endif
#ifdef SYS_gettid
    if (strcmp(name, "gettid") == 0) {
        return SYS_gettid;
    }
#endif
#ifdef SYS_readahead
    if (strcmp(name, "readahead") == 0) {
        return SYS_readahead;
    }
#endif
#ifdef SYS_setxattr
    if (strcmp(name, "setxattr") == 0) {
        return SYS_setxattr;
    }
#endif
#ifdef SYS_lsetxattr
    if (strcmp(name, "lsetxattr") == 0) {
        return SYS_lsetxattr;
    }
#endif
#ifdef SYS_fsetxattr
    if (strcmp(name, "fsetxattr") == 0) {
        return SYS_fsetxattr;
    }
#endif
#ifdef SYS_getxattr
    if (strcmp(name, "getxattr") == 0) {
        return SYS_getxattr;
    }
#endif
#ifdef SYS_lgetxattr
    if (strcmp(name, "lgetxattr") == 0) {
        return SYS_lgetxattr;
    }
#endif
#ifdef SYS_fgetxattr
    if (strcmp(name, "fgetxattr") == 0) {
        return SYS_fgetxattr;
    }
#endif
#ifdef SYS_listxattr
    if (strcmp(name, "listxattr") == 0) {
        return SYS_listxattr;
    }
#endif
#ifdef SYS_llistxattr
    if (strcmp(name, "llistxattr") == 0) {
        return SYS_llistxattr;
    }
#endif
#ifdef SYS_flistxattr
    if (strcmp(name, "flistxattr") == 0) {
        return SYS_flistxattr;
    }
#endif
#ifdef SYS_removexattr
    if (strcmp(name, "removexattr") == 0) {
        return SYS_removexattr;
    }
#endif
#ifdef SYS_lremovexattr
    if (strcmp(name, "lremovexattr") == 0) {
        return SYS_lremovexattr;
    }
#endif
#ifdef SYS_fremovexattr
    if (strcmp(name, "fremovexattr") == 0) {
        return SYS_fremovexattr;
    }
#endif
#ifdef SYS_tkill
    if (strcmp(name, "tkill") == 0) {
        return SYS_tkill;
    }
#endif
#ifdef SYS_time
    if (strcmp(name, "time") == 0) {
        return SYS_time;
    }
#endif
#ifdef SYS_futex
    if (strcmp(name, "futex") == 0) {
        return SYS_futex;
    }
#endif
#ifdef SYS_sched_setaffinity
    if (strcmp(name, "sched_setaffinity") == 0) {
        return SYS_sched_setaffinity;
    }
#endif
#ifdef SYS_sched_getaffinity
    if (strcmp(name, "sched_getaffinity") == 0) {
        return SYS_sched_getaffinity;
    }
#endif
#ifdef SYS_set_thread_area
    if (strcmp(name, "set_thread_area") == 0) {
        return SYS_set_thread_area;
    }
#endif
#ifdef SYS_io_setup
    if (strcmp(name, "io_setup") == 0) {
        return SYS_io_setup;
    }
#endif
#ifdef SYS_io_destroy
    if (strcmp(name, "io_destroy") == 0) {
        return SYS_io_destroy;
    }
#endif
#ifdef SYS_io_getevents
    if (strcmp(name, "io_getevents") == 0) {
        return SYS_io_getevents;
    }
#endif
#ifdef SYS_io_submit
    if (strcmp(name, "io_submit") == 0) {
        return SYS_io_submit;
    }
#endif
#ifdef SYS_io_cancel
    if (strcmp(name, "io_cancel") == 0) {
        return SYS_io_cancel;
    }
#endif
#ifdef SYS_get_thread_area
    if (strcmp(name, "get_thread_area") == 0) {
        return SYS_get_thread_area;
    }
#endif
#ifdef SYS_lookup_dcookie
    if (strcmp(name, "lookup_dcookie") == 0) {
        return SYS_lookup_dcookie;
    }
#endif
#ifdef SYS_epoll_create
    if (strcmp(name, "epoll_create") == 0) {
        return SYS_epoll_create;
    }
#endif
#ifdef SYS_epoll_ctl_old
    if (strcmp(name, "epoll_ctl_old") == 0) {
        return SYS_epoll_ctl_old;
    }
#endif
#ifdef SYS_epoll_wait_old
    if (strcmp(name, "epoll_wait_old") == 0) {
        return SYS_epoll_wait_old;
    }
#endif
#ifdef SYS_remap_file_pages
    if (strcmp(name, "remap_file_pages") == 0) {
        return SYS_remap_file_pages;
    }
#endif
#ifdef SYS_getdents64
    if (strcmp(name, "getdents64") == 0) {
        return SYS_getdents64;
    }
#endif
#ifdef SYS_set_tid_address
    if (strcmp(name, "set_tid_address") == 0) {
        return SYS_set_tid_address;
    }
#endif
#ifdef SYS_restart_syscall
    if (strcmp(name, "restart_syscall") == 0) {
        return SYS_restart_syscall;
    }
#endif
#ifdef SYS_semtimedop
    if (strcmp(name, "semtimedop") == 0) {
        return SYS_semtimedop;
    }
#endif
#ifdef SYS_fadvise64
    if (strcmp(name, "fadvise64") == 0) {
        return SYS_fadvise64;
    }
#endif
#ifdef SYS_timer_create
    if (strcmp(name, "timer_create") == 0) {
        return SYS_timer_create;
    }
#endif
#ifdef SYS_timer_settime
    if (strcmp(name, "timer_settime") == 0) {
        return SYS_timer_settime;
    }
#endif
#ifdef SYS_timer_gettime
    if (strcmp(name, "timer_gettime") == 0) {
        return SYS_timer_gettime;
    }
#endif
#ifdef SYS_timer_getoverrun
    if (strcmp(name, "timer_getoverrun") == 0) {
        return SYS_timer_getoverrun;
    }
#endif
#ifdef SYS_timer_delete
    if (strcmp(name, "timer_delete") == 0) {
        return SYS_timer_delete;
    }
#endif
#ifdef SYS_clock_settime
    if (strcmp(name, "clock_settime") == 0) {
        return SYS_clock_settime;
    }
#endif
#ifdef SYS_clock_gettime
    if (strcmp(name, "clock_gettime") == 0) {
        return SYS_clock_gettime;
    }
#endif
#ifdef SYS_clock_getres
    if (strcmp(name, "clock_getres") == 0) {
        return SYS_clock_getres;
    }
#endif
#ifdef SYS_clock_nanosleep
    if (strcmp(name, "clock_nanosleep") == 0) {
        return SYS_clock_nanosleep;
    }
#endif
#ifdef SYS_exit_group
    if (strcmp(name, "exit_group") == 0) {
        return SYS_exit_group;
    }
#endif
#ifdef SYS_epoll_wait
    if (strcmp(name, "epoll_wait") == 0) {
        return SYS_epoll_wait;
    }
#endif
#ifdef SYS_epoll_ctl
    if (strcmp(name, "epoll_ctl") == 0) {
        return SYS_epoll_ctl;
    }
#endif
#ifdef SYS_tgkill
    if (strcmp(name, "tgkill") == 0) {
        return SYS_tgkill;
    }
#endif
#ifdef SYS_utimes
    if (strcmp(name, "utimes") == 0) {
        return SYS_utimes;
    }
#endif
#ifdef SYS_vserver
    if (strcmp(name, "vserver") == 0) {
        return SYS_vserver;
    }
#endif
#ifdef SYS_mbind
    if (strcmp(name, "mbind") == 0) {
        return SYS_mbind;
    }
#endif
#ifdef SYS_set_mempolicy
    if (strcmp(name, "set_mempolicy") == 0) {
        return SYS_set_mempolicy;
    }
#endif
#ifdef SYS_get_mempolicy
    if (strcmp(name, "get_mempolicy") == 0) {
        return SYS_get_mempolicy;
    }
#endif
#ifdef SYS_mq_open
    if (strcmp(name, "mq_open") == 0) {
        return SYS_mq_open;
    }
#endif
#ifdef SYS_mq_unlink
    if (strcmp(name, "mq_unlink") == 0) {
        return SYS_mq_unlink;
    }
#endif
#ifdef SYS_mq_timedsend
    if (strcmp(name, "mq_timedsend") == 0) {
        return SYS_mq_timedsend;
    }
#endif
#ifdef SYS_mq_timedreceive
    if (strcmp(name, "mq_timedreceive") == 0) {
        return SYS_mq_timedreceive;
    }
#endif
#ifdef SYS_mq_notify
    if (strcmp(name, "mq_notify") == 0) {
        return SYS_mq_notify;
    }
#endif
#ifdef SYS_mq_getsetattr
    if (strcmp(name, "mq_getsetattr") == 0) {
        return SYS_mq_getsetattr;
    }
#endif
#ifdef SYS_kexec_load
    if (strcmp(name, "kexec_load") == 0) {
        return SYS_kexec_load;
    }
#endif
#ifdef SYS_waitid
    if (strcmp(name, "waitid") == 0) {
        return SYS_waitid;
    }
#endif
#ifdef SYS_add_key
    if (strcmp(name, "add_key") == 0) {
        return SYS_add_key;
    }
#endif
#ifdef SYS_request_key
    if (strcmp(name, "request_key") == 0) {
        return SYS_request_key;
    }
#endif
#ifdef SYS_keyctl
    if (strcmp(name, "keyctl") == 0) {
        return SYS_keyctl;
    }
#endif
#ifdef SYS_ioprio_set
    if (strcmp(name, "ioprio_set") == 0) {
        return SYS_ioprio_set;
    }
#endif
#ifdef SYS_ioprio_get
    if (strcmp(name, "ioprio_get") == 0) {
        return SYS_ioprio_get;
    }
#endif
#ifdef SYS_inotify_init
    if (strcmp(name, "inotify_init") == 0) {
        return SYS_inotify_init;
    }
#endif
#ifdef SYS_inotify_add_watch
    if (strcmp(name, "inotify_add_watch") == 0) {
        return SYS_inotify_add_watch;
    }
#endif
#ifdef SYS_inotify_rm_watch
    if (strcmp(name, "inotify_rm_watch") == 0) {
        return SYS_inotify_rm_watch;
    }
#endif
#ifdef SYS_migrate_pages
    if (strcmp(name, "migrate_pages") == 0) {
        return SYS_migrate_pages;
    }
#endif
#ifdef SYS_openat
    if (strcmp(name, "openat") == 0) {
        return SYS_openat;
    }
#endif
#ifdef SYS_mkdirat
    if (strcmp(name, "mkdirat") == 0) {
        return SYS_mkdirat;
    }
#endif
#ifdef SYS_mknodat
    if (strcmp(name, "mknodat") == 0) {
        return SYS_mknodat;
    }
#endif
#ifdef SYS_fchownat
    if (strcmp(name, "fchownat") == 0) {
        return SYS_fchownat;
    }
#endif
#ifdef SYS_futimesat
    if (strcmp(name, "futimesat") == 0) {
        return SYS_futimesat;
    }
#endif
#ifdef SYS_newfstatat
    if (strcmp(name, "newfstatat") == 0) {
        return SYS_newfstatat;
    }
#endif
#ifdef SYS_unlinkat
    if (strcmp(name, "unlinkat") == 0) {
        return SYS_unlinkat;
    }
#endif
#ifdef SYS_renameat
    if (strcmp(name, "renameat") == 0) {
        return SYS_renameat;
    }
#endif
#ifdef SYS_linkat
    if (strcmp(name, "linkat") == 0) {
        return SYS_linkat;
    }
#endif
#ifdef SYS_symlinkat
    if (strcmp(name, "symlinkat") == 0) {
        return SYS_symlinkat;
    }
#endif
#ifdef SYS_readlinkat
    if (strcmp(name, "readlinkat") == 0) {
        return SYS_readlinkat;
    }
#endif
#ifdef SYS_fchmodat
    if (strcmp(name, "fchmodat") == 0) {
        return SYS_fchmodat;
    }
#endif
#ifdef SYS_faccessat
    if (strcmp(name, "faccessat") == 0) {
        return SYS_faccessat;
    }
#endif
#ifdef SYS_pselect6
    if (strcmp(name, "pselect6") == 0) {
        return SYS_pselect6;
    }
#endif
#ifdef SYS_ppoll
    if (strcmp(name, "ppoll") == 0) {
        return SYS_ppoll;
    }
#endif
#ifdef SYS_unshare
    if (strcmp(name, "unshare") == 0) {
        return SYS_unshare;
    }
#endif
#ifdef SYS_set_robust_list
    if (strcmp(name, "set_robust_list") == 0) {
        return SYS_set_robust_list;
    }
#endif
#ifdef SYS_get_robust_list
    if (strcmp(name, "get_robust_list") == 0) {
        return SYS_get_robust_list;
    }
#endif
#ifdef SYS_splice
    if (strcmp(name, "splice") == 0) {
        return SYS_splice;
    }
#endif
#ifdef SYS_tee
    if (strcmp(name, "tee") == 0) {
        return SYS_tee;
    }
#endif
#ifdef SYS_sync_file_range
    if (strcmp(name, "sync_file_range") == 0) {
        return SYS_sync_file_range;
    }
#endif
#ifdef SYS_vmsplice
    if (strcmp(name, "vmsplice") == 0) {
        return SYS_vmsplice;
    }
#endif
#ifdef SYS_move_pages
    if (strcmp(name, "move_pages") == 0) {
        return SYS_move_pages;
    }
#endif
#ifdef SYS_utimensat
    if (strcmp(name, "utimensat") == 0) {
        return SYS_utimensat;
    }
#endif
#ifdef SYS_epoll_pwait
    if (strcmp(name, "epoll_pwait") == 0) {
        return SYS_epoll_pwait;
    }
#endif
#ifdef SYS_signalfd
    if (strcmp(name, "signalfd") == 0) {
        return SYS_signalfd;
    }
#endif
#ifdef SYS_timerfd_create
    if (strcmp(name, "timerfd_create") == 0) {
        return SYS_timerfd_create;
    }
#endif
#ifdef SYS_eventfd
    if (strcmp(name, "eventfd") == 0) {
        return SYS_eventfd;
    }
#endif
#ifdef SYS_fallocate
    if (strcmp(name, "fallocate") == 0) {
        return SYS_fallocate;
    }
#endif
#ifdef SYS_timerfd_settime
    if (strcmp(name, "timerfd_settime") == 0) {
        return SYS_timerfd_settime;
    }
#endif
#ifdef SYS_timerfd_gettime
    if (strcmp(name, "timerfd_gettime") == 0) {
        return SYS_timerfd_gettime;
    }
#endif
#ifdef SYS_accept4
    if (strcmp(name, "accept4") == 0) {
        return SYS_accept4;
    }
#endif
#ifdef SYS_signalfd4
    if (strcmp(name, "signalfd4") == 0) {
        return SYS_signalfd4;
    }
#endif
#ifdef SYS_eventfd2
    if (strcmp(name, "eventfd2") == 0) {
        return SYS_eventfd2;
    }
#endif
#ifdef SYS_epoll_create1
    if (strcmp(name, "epoll_create1") == 0) {
        return SYS_epoll_create1;
    }
#endif
#ifdef SYS_dup3
    if (strcmp(name, "dup3") == 0) {
        return SYS_dup3;
    }
#endif
#ifdef SYS_pipe2
    if (strcmp(name, "pipe2") == 0) {
        return SYS_pipe2;
    }
#endif
#ifdef SYS_inotify_init1
    if (strcmp(name, "inotify_init1") == 0) {
        return SYS_inotify_init1;
    }
#endif
#ifdef SYS_preadv
    if (strcmp(name, "preadv") == 0) {
        return SYS_preadv;
    }
#endif
#ifdef SYS_pwritev
    if (strcmp(name, "pwritev") == 0) {
        return SYS_pwritev;
    }
#endif
#ifdef SYS_rt_tgsigqueueinfo
    if (strcmp(name, "rt_tgsigqueueinfo") == 0) {
        return SYS_rt_tgsigqueueinfo;
    }
#endif
#ifdef SYS_perf_event_open
    if (strcmp(name, "perf_event_open") == 0) {
        return SYS_perf_event_open;
    }
#endif
#ifdef SYS_recvmmsg
    if (strcmp(name, "recvmmsg") == 0) {
        return SYS_recvmmsg;
    }
#endif
#ifdef SYS_fanotify_init
    if (strcmp(name, "fanotify_init") == 0) {
        return SYS_fanotify_init;
    }
#endif
#ifdef SYS_fanotify_mark
    if (strcmp(name, "fanotify_mark") == 0) {
        return SYS_fanotify_mark;
    }
#endif
#ifdef SYS_prlimit64
    if (strcmp(name, "prlimit64") == 0) {
        return SYS_prlimit64;
    }
#endif
#ifdef SYS_name_to_handle_at
    if (strcmp(name, "name_to_handle_at") == 0) {
        return SYS_name_to_handle_at;
    }
#endif
#ifdef SYS_open_by_handle_at
    if (strcmp(name, "open_by_handle_at") == 0) {
        return SYS_open_by_handle_at;
    }
#endif
#ifdef SYS_clock_adjtime
    if (strcmp(name, "clock_adjtime") == 0) {
        return SYS_clock_adjtime;
    }
#endif
#ifdef SYS_syncfs
    if (strcmp(name, "syncfs") == 0) {
        return SYS_syncfs;
    }
#endif
#ifdef SYS_sendmmsg
    if (strcmp(name, "sendmmsg") == 0) {
        return SYS_sendmmsg;
    }
#endif
#ifdef SYS_setns
    if (strcmp(name, "setns") == 0) {
        return SYS_setns;
    }
#endif
#ifdef SYS_getcpu
    if (strcmp(name, "getcpu") == 0) {
        return SYS_getcpu;
    }
#endif
#ifdef SYS_process_vm_readv
    if (strcmp(name, "process_vm_readv") == 0) {
        return SYS_process_vm_readv;
    }
#endif
#ifdef SYS_process_vm_writev
    if (strcmp(name, "process_vm_writev") == 0) {
        return SYS_process_vm_writev;
    }
#endif
#ifdef SYS_kcmp
    if (strcmp(name, "kcmp") == 0) {
        return SYS_kcmp;
    }
#endif
#ifdef SYS_finit_module
    if (strcmp(name, "finit_module") == 0) {
        return SYS_finit_module;
    }
#endif
#ifdef SYS_sched_setattr
    if (strcmp(name, "sched_setattr") == 0) {
        return SYS_sched_setattr;
    }
#endif
#ifdef SYS_sched_getattr
    if (strcmp(name, "sched_getattr") == 0) {
        return SYS_sched_getattr;
    }
#endif
#ifdef SYS_renameat2
    if (strcmp(name, "renameat2") == 0) {
        return SYS_renameat2;
    }
#endif
#ifdef SYS_seccomp
    if (strcmp(name, "seccomp") == 0) {
        return SYS_seccomp;
    }
#endif
#ifdef SYS_getrandom
    if (strcmp(name, "getrandom") == 0) {
        return SYS_getrandom;
    }
#endif
#ifdef SYS_memfd_create
    if (strcmp(name, "memfd_create") == 0) {
        return SYS_memfd_create;
    }
#endif
#ifdef SYS_kexec_file_load
    if (strcmp(name, "kexec_file_load") == 0) {
        return SYS_kexec_file_load;
    }
#endif
#ifdef SYS_bpf
    if (strcmp(name, "bpf") == 0) {
        return SYS_bpf;
    }
#endif
#ifdef SYS_execveat
    if (strcmp(name, "execveat") == 0) {
        return SYS_execveat;
    }
#endif
#ifdef SYS_userfaultfd
    if (strcmp(name, "userfaultfd") == 0) {
        return SYS_userfaultfd;
    }
#endif
#ifdef SYS_membarrier
    if (strcmp(name, "membarrier") == 0) {
        return SYS_membarrier;
    }
#endif
#ifdef SYS_mlock2
    if (strcmp(name, "mlock2") == 0) {
        return SYS_mlock2;
    }
#endif
#ifdef SYS_copy_file_range
    if (strcmp(name, "copy_file_range") == 0) {
        return SYS_copy_file_range;
    }
#endif
#ifdef SYS_preadv2
    if (strcmp(name, "preadv2") == 0) {
        return SYS_preadv2;
    }
#endif
#ifdef SYS_pwritev2
    if (strcmp(name, "pwritev2") == 0) {
        return SYS_pwritev2;
    }
#endif
#ifdef SYS_pkey_mprotect
    if (strcmp(name, "pkey_mprotect") == 0) {
        return SYS_pkey_mprotect;
    }
#endif
#ifdef SYS_pkey_alloc
    if (strcmp(name, "pkey_alloc") == 0) {
        return SYS_pkey_alloc;
    }
#endif
#ifdef SYS_pkey_free
    if (strcmp(name, "pkey_free") == 0) {
        return SYS_pkey_free;
    }
#endif
#ifdef SYS_statx
    if (strcmp(name, "statx") == 0) {
        return SYS_statx;
    }
#endif
#ifdef SYS_io_pgetevents
    if (strcmp(name, "io_pgetevents") == 0) {
        return SYS_io_pgetevents;
    }
#endif
#ifdef SYS_rseq
    if (strcmp(name, "rseq") == 0) {
        return SYS_rseq;
    }
#endif
#ifdef SYS_uretprobe
    if (strcmp(name, "uretprobe") == 0) {
        return SYS_uretprobe;
    }
#endif
#ifdef SYS_uprobe
    if (strcmp(name, "uprobe") == 0) {
        return SYS_uprobe;
    }
#endif
#ifdef SYS_pidfd_send_signal
    if (strcmp(name, "pidfd_send_signal") == 0) {
        return SYS_pidfd_send_signal;
    }
#endif
#ifdef SYS_io_uring_setup
    if (strcmp(name, "io_uring_setup") == 0) {
        return SYS_io_uring_setup;
    }
#endif
#ifdef SYS_io_uring_enter
    if (strcmp(name, "io_uring_enter") == 0) {
        return SYS_io_uring_enter;
    }
#endif
#ifdef SYS_io_uring_register
    if (strcmp(name, "io_uring_register") == 0) {
        return SYS_io_uring_register;
    }
#endif
#ifdef SYS_open_tree
    if (strcmp(name, "open_tree") == 0) {
        return SYS_open_tree;
    }
#endif
#ifdef SYS_move_mount
    if (strcmp(name, "move_mount") == 0) {
        return SYS_move_mount;
    }
#endif
#ifdef SYS_fsopen
    if (strcmp(name, "fsopen") == 0) {
        return SYS_fsopen;
    }
#endif
#ifdef SYS_fsconfig
    if (strcmp(name, "fsconfig") == 0) {
        return SYS_fsconfig;
    }
#endif
#ifdef SYS_fsmount
    if (strcmp(name, "fsmount") == 0) {
        return SYS_fsmount;
    }
#endif
#ifdef SYS_fspick
    if (strcmp(name, "fspick") == 0) {
        return SYS_fspick;
    }
#endif
#ifdef SYS_pidfd_open
    if (strcmp(name, "pidfd_open") == 0) {
        return SYS_pidfd_open;
    }
#endif
#ifdef SYS_clone3
    if (strcmp(name, "clone3") == 0) {
        return SYS_clone3;
    }
#endif
#ifdef SYS_close_range
    if (strcmp(name, "close_range") == 0) {
        return SYS_close_range;
    }
#endif
#ifdef SYS_openat2
    if (strcmp(name, "openat2") == 0) {
        return SYS_openat2;
    }
#endif
#ifdef SYS_pidfd_getfd
    if (strcmp(name, "pidfd_getfd") == 0) {
        return SYS_pidfd_getfd;
    }
#endif
#ifdef SYS_faccessat2
    if (strcmp(name, "faccessat2") == 0) {
        return SYS_faccessat2;
    }
#endif
#ifdef SYS_process_madvise
    if (strcmp(name, "process_madvise") == 0) {
        return SYS_process_madvise;
    }
#endif
#ifdef SYS_epoll_pwait2
    if (strcmp(name, "epoll_pwait2") == 0) {
        return SYS_epoll_pwait2;
    }
#endif
#ifdef SYS_mount_setattr
    if (strcmp(name, "mount_setattr") == 0) {
        return SYS_mount_setattr;
    }
#endif
#ifdef SYS_quotactl_fd
    if (strcmp(name, "quotactl_fd") == 0) {
        return SYS_quotactl_fd;
    }
#endif
#ifdef SYS_landlock_create_ruleset
    if (strcmp(name, "landlock_create_ruleset") == 0) {
        return SYS_landlock_create_ruleset;
    }
#endif
#ifdef SYS_landlock_add_rule
    if (strcmp(name, "landlock_add_rule") == 0) {
        return SYS_landlock_add_rule;
    }
#endif
#ifdef SYS_landlock_restrict_self
    if (strcmp(name, "landlock_restrict_self") == 0) {
        return SYS_landlock_restrict_self;
    }
#endif
#ifdef SYS_memfd_secret
    if (strcmp(name, "memfd_secret") == 0) {
        return SYS_memfd_secret;
    }
#endif
#ifdef SYS_process_mrelease
    if (strcmp(name, "process_mrelease") == 0) {
        return SYS_process_mrelease;
    }
#endif
#ifdef SYS_futex_waitv
    if (strcmp(name, "futex_waitv") == 0) {
        return SYS_futex_waitv;
    }
#endif
#ifdef SYS_set_mempolicy_home_node
    if (strcmp(name, "set_mempolicy_home_node") == 0) {
        return SYS_set_mempolicy_home_node;
    }
#endif
#ifdef SYS_cachestat
    if (strcmp(name, "cachestat") == 0) {
        return SYS_cachestat;
    }
#endif
#ifdef SYS_fchmodat2
    if (strcmp(name, "fchmodat2") == 0) {
        return SYS_fchmodat2;
    }
#endif
#ifdef SYS_map_shadow_stack
    if (strcmp(name, "map_shadow_stack") == 0) {
        return SYS_map_shadow_stack;
    }
#endif
#ifdef SYS_futex_wake
    if (strcmp(name, "futex_wake") == 0) {
        return SYS_futex_wake;
    }
#endif
#ifdef SYS_futex_wait
    if (strcmp(name, "futex_wait") == 0) {
        return SYS_futex_wait;
    }
#endif
#ifdef SYS_futex_requeue
    if (strcmp(name, "futex_requeue") == 0) {
        return SYS_futex_requeue;
    }
#endif
#ifdef SYS_statmount
    if (strcmp(name, "statmount") == 0) {
        return SYS_statmount;
    }
#endif
#ifdef SYS_listmount
    if (strcmp(name, "listmount") == 0) {
        return SYS_listmount;
    }
#endif
#ifdef SYS_lsm_get_self_attr
    if (strcmp(name, "lsm_get_self_attr") == 0) {
        return SYS_lsm_get_self_attr;
    }
#endif
#ifdef SYS_lsm_set_self_attr
    if (strcmp(name, "lsm_set_self_attr") == 0) {
        return SYS_lsm_set_self_attr;
    }
#endif
#ifdef SYS_lsm_list_modules
    if (strcmp(name, "lsm_list_modules") == 0) {
        return SYS_lsm_list_modules;
    }
#endif
#ifdef SYS_mseal
    if (strcmp(name, "mseal") == 0) {
        return SYS_mseal;
    }
#endif
#ifdef SYS_setxattrat
    if (strcmp(name, "setxattrat") == 0) {
        return SYS_setxattrat;
    }
#endif
#ifdef SYS_getxattrat
    if (strcmp(name, "getxattrat") == 0) {
        return SYS_getxattrat;
    }
#endif
#ifdef SYS_listxattrat
    if (strcmp(name, "listxattrat") == 0) {
        return SYS_listxattrat;
    }
#endif
#ifdef SYS_removexattrat
    if (strcmp(name, "removexattrat") == 0) {
        return SYS_removexattrat;
    }
#endif
#ifdef SYS_open_tree_attr
    if (strcmp(name, "open_tree_attr") == 0) {
        return SYS_open_tree_attr;
    }
#endif
#ifdef SYS_file_getattr
    if (strcmp(name, "file_getattr") == 0) {
        return SYS_file_getattr;
    }
#endif
#ifdef SYS_file_setattr
    if (strcmp(name, "file_setattr") == 0) {
        return SYS_file_setattr;
    }
#endif
#ifdef SYS_listns
    if (strcmp(name, "listns") == 0) {
        return SYS_listns;
    }
#endif
#ifdef SYS_rseq_slice_yield
    if (strcmp(name, "rseq_slice_yield") == 0) {
        return SYS_rseq_slice_yield;
    }
#endif
