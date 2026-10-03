/* Installed HNP alias entry. Preserve the alias argv[0] when re-executing Codex. */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define HNP_INSTALLER_UID 3060

static int protected_metadata(const struct stat *metadata, int directory, int installed_subtree)
{
    int installer_directory = directory && installed_subtree &&
        metadata->st_uid == HNP_INSTALLER_UID && metadata->st_gid == HNP_INSTALLER_UID &&
        (metadata->st_mode & 07777) == 0775;
    return (metadata->st_uid == 0 || metadata->st_uid == HNP_INSTALLER_UID) &&
        (!(metadata->st_mode & (S_IWGRP | S_IWOTH | S_ISUID | S_ISGID | S_ISVTX)) || installer_directory) &&
        (directory ? S_ISDIR(metadata->st_mode) :
         (S_ISREG(metadata->st_mode) && (metadata->st_mode & S_IXOTH)));
}

/* Every component must belong to the OS/installer and be immutable to apps. */
static int protected_file(const char *path)
{
    if (strncmp(path, "/data/app/", 10) != 0) {
        errno = EPERM;
        return -1;
    }
    char remaining[PATH_MAX];
    if (strlen(path) >= sizeof(remaining)) {
        errno = ENAMETOOLONG;
        return -1;
    }
    strcpy(remaining, path + 1);
    int parent = open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    struct stat metadata;
    if (parent < 0) return -1;
    if (fstat(parent, &metadata) < 0 || !protected_metadata(&metadata, 1, 0)) {
        close(parent);
        errno = EPERM;
        return -1;
    }
    char *component = remaining;
    unsigned depth = 0;
    for (;;) {
        depth++;
        char *separator = strchr(component, '/');
        if (separator) *separator = '\0';
        if (!*component || !strcmp(component, ".") || !strcmp(component, "..")) {
            close(parent);
            errno = EPERM;
            return -1;
        }
        int access = separator ? O_PATH | O_DIRECTORY : O_RDONLY | O_NONBLOCK;
        int child = openat(parent, component, access | O_NOFOLLOW | O_CLOEXEC);
        int saved_errno = errno;
        close(parent);
        if (child < 0) {
            errno = saved_errno;
            return -1;
        }
        if (fstat(child, &metadata) < 0 || !protected_metadata(&metadata, separator != NULL, depth >= 2)) {
            close(child);
            errno = EPERM;
            return -1;
        }
        if (!separator) return child;
        parent = child;
        component = separator + 1;
    }
}

static int application_identity(void)
{
    if (geteuid() < 10000 || getegid() == HNP_INSTALLER_UID) return 0;
    int count = getgroups(0, NULL);
    if (count < 0) return 0;
    gid_t *groups = calloc((size_t)count + 1, sizeof(gid_t));
    if (!groups) return 0;
    int actual = getgroups(count, groups);
    int valid = actual >= 0;
    for (int index = 0; index < actual; index++) {
        if (groups[index] == HNP_INSTALLER_UID) valid = 0;
    }
    free(groups);
    return valid;
}

int main(int argc, char **argv)
{
    if (argc < 1 || !application_identity()) {
        fputs("HNP alias requires an application process and argv[0]\n", stderr);
        return 126;
    }
    char executable[PATH_MAX];
    ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable) - 1);
    if (length < 0 || (size_t)length >= sizeof(executable) - 1) {
        fputs("HNP alias cannot resolve its installed executable\n", stderr);
        return 126;
    }
    executable[length] = '\0';
    char *basename = strrchr(executable, '/');
    const char *alias = NULL;
    const char *names[] = {"apply_patch", "applypatch", "codex-linux-sandbox", "codex-execve-wrapper"};
    for (size_t index = 0; basename && index < sizeof(names) / sizeof(names[0]); index++) {
        if (!strcmp(basename + 1, names[index])) alias = names[index];
    }
    if (!alias) {
        fputs("HNP alias has an unsupported installed name\n", stderr);
        return 126;
    }
    int launcher = protected_file(executable);
    if (launcher < 0) {
        perror("HNP alias installed path is not protected");
        return 126;
    }
    *basename = '\0';
    char *directory = strrchr(executable, '/');
    if (!directory || strcmp(directory + 1, "codex-path")) {
        close(launcher);
        fputs("HNP alias must be installed inside codex-path\n", stderr);
        return 126;
    }
    strcpy(directory + 1, "bin/codex"); /* Shorter than the removed codex-path/name. */
    int codex = protected_file(executable);
    if (codex < 0) {
        close(launcher);
        perror("HNP alias target is not a protected package executable");
        return 126;
    }
    argv[0] = (char *)alias;
    execv(executable, argv);
    int saved_errno = errno;
    close(codex);
    close(launcher);
    errno = saved_errno;
    perror("HNP alias could not execute Codex");
    return saved_errno == ENOENT ? 127 : 126;
}
