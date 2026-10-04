#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/statvfs.h>
#include <sys/types.h>
#include <sys/xattr.h>
#include <unistd.h>

static void Describe(FILE *out, const char *path) {
    struct stat st = {0};
    errno = 0;
    int result = lstat(path, &st);
    fprintf(out, "path=%s lstat=%d errno=%d uid=%u gid=%u mode=%o type=%o\n",
            path, result, errno, st.st_uid, st.st_gid, st.st_mode & 07777, st.st_mode & S_IFMT);
    errno = 0;
    int fd = open(path, O_PATH | O_NOFOLLOW | O_CLOEXEC);
    int error = errno;
    memset(&st, 0, sizeof(st));
    int fst = fd >= 0 ? fstat(fd, &st) : -1;
    fprintf(out, "opath=%d errno=%d fstat=%d uid=%u gid=%u mode=%o\n",
            fd >= 0 ? 0 : -1, error, fst, st.st_uid, st.st_gid, st.st_mode & 07777);
    if (fd >= 0) close(fd);
    char label[512] = {0};
    errno = 0;
    ssize_t length = lgetxattr(path, "security.selinux", label, sizeof(label) - 1);
    fprintf(out, "selinux_length=%ld errno=%d label=%s\n", (long)length, errno,
            length >= 0 ? label : "(unavailable)");
    struct statvfs fs = {0};
    errno = 0;
    result = statvfs(path, &fs);
    fprintf(out, "statvfs=%d errno=%d flags=%lu noexec=%d readonly=%d\n", result, errno,
            fs.f_flag, (fs.f_flag & ST_NOEXEC) != 0, (fs.f_flag & ST_RDONLY) != 0);
    fflush(out);
}

static int CreateProtected(FILE *out, int parent, const char *name) {
    errno = 0;
    int made = mkdirat(parent, name, 0700);
    int makeError = errno;
    if (made != 0 && makeError != EEXIST) {
        fprintf(out, "mkdirat=%s result=%d errno=%d\n", name, made, makeError);
        return -1;
    }
    struct stat st = {0};
    int checked = fstatat(parent, name, &st, AT_SYMLINK_NOFOLLOW);
    int safe = checked == 0 && S_ISDIR(st.st_mode) && st.st_uid == getuid() &&
               (st.st_mode & 07777) == 0700;
    fprintf(out, "mkdirat=%s result=%d errno=%d fstatat=%d uid=%u gid=%u mode=%o safe=%d\n",
            name, made, makeError, checked, st.st_uid, st.st_gid, st.st_mode & 07777, safe);
    if (!safe) return -1; // Never chmod/chown a pre-existing directory.
    errno = 0;
    int fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    fprintf(out, "protected_openat=%s result=%d errno=%d\n", name, fd >= 0 ? 0 : -1, errno);
    return fd;
}

void ProbeDirectories(FILE *out, const char *directory) {
    fprintf(out, "directory_probe_uid=%u gid=%u pid=%d context=%s\n", getuid(), getgid(), getpid(), directory);
    fprintf(out, "identity_uid=%u euid=%u gid=%u egid=%u\n", getuid(), geteuid(), getgid(), getegid());
    gid_t groups[128];
    int groupCount = getgroups(128, groups);
    fprintf(out, "supplementary_count=%d errno=%d groups=", groupCount, groupCount < 0 ? errno : 0);
    for (int i = 0; i < groupCount; ++i) fprintf(out, "%s%u", i ? "," : "", groups[i]);
    fprintf(out, "\n");
    char selfLabel[512] = {0};
    FILE *labelFile = fopen("/proc/self/attr/current", "r");
    if (labelFile) {
        if (fgets(selfLabel, sizeof(selfLabel), labelFile))
            fprintf(out, "process_selinux=%s\n", selfLabel);
        fclose(labelFile);
    } else fprintf(out, "process_selinux_errno=%d\n", errno);
    if (!directory || directory[0] != '/' || strlen(directory) >= PATH_MAX) return;
    Describe(out, "/");
    char path[PATH_MAX];
    snprintf(path, sizeof(path), "%s", directory);
    int parent = open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    for (char *end = path + 1; ; ++end) {
        if (*end != '/' && *end != 0) continue;
        char old = *end;
        *end = 0;
        Describe(out, path);
        const char *component = strrchr(path, '/') + 1;
        errno = 0;
        int next = parent >= 0 ? openat(parent, component, O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) : -1;
        int error = errno;
        struct stat st = {0};
        int checked = next >= 0 ? fstat(next, &st) : -1;
        fprintf(out, "component_openat=%s result=%d errno=%d fstat=%d uid=%u gid=%u mode=%o\n",
                component, next >= 0 ? 0 : -1, error, checked, st.st_uid, st.st_gid, st.st_mode & 07777);
        if (parent >= 0) close(parent);
        parent = next;
        *end = old;
        if (!old) break;
    }
    if (parent >= 0) {
        int shortRoot = CreateProtected(out, parent, "r");
        if (shortRoot >= 0) close(shortRoot);
        int runtime = CreateProtected(out, parent, "codex-runtime");
        if (runtime >= 0) {
            const char *children[] = {"state", "workspace", "tmp", "logs"};
            for (unsigned i = 0; i < sizeof(children) / sizeof(children[0]); ++i) {
                int child = CreateProtected(out, runtime, children[i]);
                if (child >= 0) close(child);
            }
            close(runtime);
        }
        close(parent);
    }
    errno = 0;
    FILE *mounts = fopen("/proc/self/mountinfo", "r");
    if (!mounts) fprintf(out, "mountinfo_open_errno=%d\n", errno);
    else {
        char line[16384];
        while (fgets(line, sizeof(line), mounts)) {
            char mountpoint[PATH_MAX] = {0};
            if (sscanf(line, "%*s %*s %*s %*s %4095s", mountpoint) == 1) {
                size_t length = strlen(mountpoint);
                if (strcmp(mountpoint, "/") == 0 || strncmp(mountpoint, "/data/app", 9) == 0 ||
                    (strncmp(directory, mountpoint, length) == 0 &&
                     (directory[length] == '/' || directory[length] == 0)))
                    fprintf(out, "ancestor_mount=%s", line);
            }
        }
        fclose(mounts);
    }
    const char *hnpPaths[] = {"/data/app", "/data/app/codexnetprobe.org", "/data/app/codexnetprobe.org/codexnetprobe_0.1", "/data/app/codexnetprobe.org/codexnetprobe_0.1/bin", "/data/app/codexnetprobe.org/codexnetprobe_0.1/bin/network-probe", "/data/app/codexnetprobe.org/codexnetprobe_0.1/codex-path"};
    for (unsigned i = 0; i < sizeof(hnpPaths)/sizeof(hnpPaths[0]); ++i) Describe(out, hnpPaths[i]);
    fprintf(out, "directory_probe_complete=yes\n");
    fflush(out);
}

#ifndef HNP_LIBRARY_ONLY
int main(int argc, char **argv) {
    if (argc != 2) return 2;
    alarm(15);
    ProbeDirectories(stdout, argv[1]);
    return 0;
}
#endif
