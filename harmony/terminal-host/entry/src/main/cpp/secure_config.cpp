#include "secure_config.h"
#include <cerrno>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

namespace {
constexpr off_t kMaxConfigBytes = 64 * 1024;

bool PrivateDirectory(int fd) {
    struct stat st = {};
    return fstat(fd, &st) == 0 && S_ISDIR(st.st_mode) &&
        st.st_uid == geteuid() && (st.st_mode & 07777) == 0700;
}

int OpenPrivateDirectory(int parent, const char *name) {
    if (mkdirat(parent, name, 0700) != 0 && errno != EEXIST) return -1;
    int fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd >= 0 && !PrivateDirectory(fd)) {
        close(fd);
        errno = EACCES;
        return -1;
    }
    return fd;
}

bool ConfigFile(int fd, bool incoming, struct stat *st) {
    if (fstat(fd, st) != 0) return false;
    mode_t mode = st->st_mode & 07777;
    return S_ISREG(st->st_mode) && st->st_uid == geteuid() && st->st_nlink == 1 &&
        st->st_size > 0 && st->st_size <= kMaxConfigBytes &&
        (mode == 0600 || (incoming && mode == 0660));
}

int Result(FILE *report, const char *stage, int result, int error = 0) {
    if (report) {
        fprintf(report, "config_import_stage=%s result=%d errno=%d\n", stage, result, error);
        fflush(report);
    }
    return result;
}
}

int ImportPrivateConfig(int files_fd, FILE *report) {
    if (!PrivateDirectory(files_fd)) return Result(report, "application-directory", -1, EACCES);
    int inbox = OpenPrivateDirectory(files_fd, "handoff-private");
    if (inbox < 0) return Result(report, "inbox-directory", -1, errno);
    int state = OpenPrivateDirectory(files_fd, "state");
    if (state < 0) { int error = errno; close(inbox); return Result(report, "state-directory", -1, error); }
    int input = openat(inbox, "incoming-config.toml", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (input < 0) {
        int error = errno;
        close(inbox);
        if (error != ENOENT) { close(state); return Result(report, "source-open", -1, error); }
        int existing = openat(state, "config.toml", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
        error = errno;
        close(state);
        if (existing < 0) return Result(report, "existing-open", error == ENOENT ? 0 : -1, error);
        struct stat st = {};
        bool valid = ConfigFile(existing, false, &st);
        close(existing);
        return Result(report, "existing-validate", valid ? 1 : -1, valid ? 0 : EACCES);
    }
    struct stat initial = {};
    if (!ConfigFile(input, true, &initial)) {
        close(input); close(inbox); close(state);
        return Result(report, "source-validate", -1, EACCES);
    }
    char temporary[64];
    snprintf(temporary, sizeof(temporary), ".config-import-%ld.tmp", static_cast<long>(getpid()));
    int output = openat(state, temporary, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (output < 0) {
        int error = errno; close(input); close(inbox); close(state);
        return Result(report, "temporary-open", -1, error);
    }
    int error = 0;
    off_t total = 0;
    char buffer[4096];
    for (;;) {
        ssize_t count = read(input, buffer, sizeof(buffer));
        if (count < 0 && errno == EINTR) continue;
        if (count < 0) { error = errno; break; }
        if (count == 0) break;
        total += count;
        if (total > initial.st_size || total > kMaxConfigBytes) { error = EFBIG; break; }
        for (ssize_t offset = 0; offset < count;) {
            ssize_t sent = write(output, buffer + offset, count - offset);
            if (sent < 0 && errno == EINTR) continue;
            if (sent <= 0) { error = sent < 0 ? errno : EIO; break; }
            offset += sent;
        }
        if (error) break;
    }
    if (!error && total != initial.st_size) error = EIO;
    if (!error && fchmod(output, 0600) != 0) error = errno;
    if (!error && fsync(output) != 0) error = errno;
    struct stat saved = {};
    if (!error && !ConfigFile(output, false, &saved)) error = EACCES;
    if (close(output) != 0 && !error) error = errno;
    close(input);
    if (!error && renameat(state, temporary, state, "config.toml") != 0) error = errno;
    if (!error && fsync(state) != 0) error = errno;
    if (error) {
        unlinkat(state, temporary, 0);
        close(inbox); close(state);
        return Result(report, "atomic-save", -1, error);
    }
    if (unlinkat(inbox, "incoming-config.toml", 0) != 0) error = errno;
    if (!error && fsync(inbox) != 0) error = errno;
    close(inbox); close(state);
    return Result(report, "complete", error ? -1 : 2, error);
}
