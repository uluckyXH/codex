#define _GNU_SOURCE
#include <errno.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

/* Read-only capability queries and restrictions on disposable child processes.
 * Never change system settings, capabilities, credentials, files, or mounts. */
static void report(const char *name, long result, int error) {
    printf("probe=%s result=%ld errno=%d\n", name, result, error);
    fflush(stdout);
}

static void run_unshare(const char *name, int flags) {
    errno = 0;
    long result = syscall(SYS_unshare, flags);
    report(name, result, result < 0 ? errno : 0);
}

static void run_clone(const char *name, int flags) {
    errno = 0;
    long result = syscall(SYS_clone, flags | SIGCHLD, NULL, NULL, 0, NULL);
    int error = result < 0 ? errno : 0;
    if (result == 0)
        _Exit(0);
    report(name, result < 0 ? result : 0, error);
    if (result > 0) {
        int status;
        while (waitpid((pid_t)result, &status, 0) < 0 && errno == EINTR) {}
    }
}

static void run_seccomp(void) {
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, SYS_getpid, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | EPERM),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = sizeof(filter) / sizeof(filter[0]),
        .filter = filter,
    };
    errno = 0;
    long result = prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
    report("no_new_privs", result, result < 0 ? errno : 0);
    if (result < 0)
        return;
    errno = 0;
    result = syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, 0, &program);
    report("seccomp_install", result, result < 0 ? errno : 0);
    if (result < 0)
        return;
    errno = 0;
    result = syscall(SYS_getpid);
    report("seccomp_denied_getpid", result, result < 0 ? errno : 0);
}

static void run_landlock(void) {
    /* OHOS SDK headers predate Landlock. 444 is the asm-generic/aarch64
     * landlock_create_ruleset syscall; VERSION=1 only asks for the ABI. */
#if defined(__aarch64__)
    errno = 0;
    long result = syscall(444, NULL, 0, 1);
    report("landlock_abi", result, result < 0 ? errno : 0);
#else
    report("landlock_abi", -1, ENOTSUP);
#endif
}

int main(void) {
    static const struct {
        const char *name;
        int kind;
        int flags;
    } probes[] = {
        {"unshare_none", 0, 0},
        {"unshare_user", 0, CLONE_NEWUSER},
        {"unshare_mount", 0, CLONE_NEWNS},
        {"unshare_user_mount", 0, CLONE_NEWUSER | CLONE_NEWNS},
        {"unshare_net", 0, CLONE_NEWNET},
        {"unshare_pid", 0, CLONE_NEWPID},
        {"clone_none", 1, 0},
        {"clone_user", 1, CLONE_NEWUSER},
        {"clone_user_mount", 1, CLONE_NEWUSER | CLONE_NEWNS},
        {"seccomp", 2, 0},
        {"landlock", 3, 0},
    };
    printf("uid=%u gid=%u\n", (unsigned)getuid(), (unsigned)getgid());
    fflush(stdout);
    for (size_t i = 0; i < sizeof(probes) / sizeof(probes[0]); ++i) {
        pid_t child = fork();
        if (child < 0) {
            report("fork", -1, errno);
            return 1;
        }
        if (child == 0) {
            switch (probes[i].kind) {
                case 0: run_unshare(probes[i].name, probes[i].flags); break;
                case 1: run_clone(probes[i].name, probes[i].flags); break;
                case 2: run_seccomp(); break;
                case 3: run_landlock(); break;
            }
            fflush(stdout);
            _Exit(0);
        }
        int status;
        pid_t waited;
        do {
            waited = waitpid(child, &status, 0);
        } while (waited < 0 && errno == EINTR);
        if (waited < 0) {
            report("waitpid", -1, errno);
            return 1;
        }
        printf("probe=%s child_exit=%d child_signal=%d raw_wait_status=%d\n",
               probes[i].name, WIFEXITED(status) ? WEXITSTATUS(status) : -1,
               WIFSIGNALED(status) ? WTERMSIG(status) : 0, status);
        fflush(stdout);
    }
    return 0;
}
