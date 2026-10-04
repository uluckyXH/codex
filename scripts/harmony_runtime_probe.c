/* Standalone, offline diagnostics. No Codex startup, config, dotenv or shell.
 * Directory observations are evidence, never approval of a runtime root.
 * Build: C11, -Wall -Wextra -Werror; OHOS also links -ldl.
 */
#define _GNU_SOURCE 1
#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/un.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#if defined(__APPLE__)
#include <mach-o/dyld.h>
#include <sys/mount.h>
#else
#include <sys/vfs.h>
#endif
#if defined(__OHOS__)
#include "harmony_runtime_probe_context.h"
#endif

#ifndef CODEX_HARMONY_BUILD_ID
#define CODEX_HARMONY_BUILD_ID "unknown"
#endif
#ifndef CODEX_HARMONY_VERSION
#define CODEX_HARMONY_VERSION "unknown"
#endif
#ifndef PROBE_ITEM_TIMEOUT_MS
#define PROBE_ITEM_TIMEOUT_MS 1500
#endif
#ifndef PROBE_TOTAL_TIMEOUT_MS
#define PROBE_TOTAL_TIMEOUT_MS 15000
#endif
#define MAX_PATHS 12
#define PATH_BYTES 1024
#define MAX_ANCESTORS 32
#define MAX_GROUPS 256
#define CAPTURE_BYTES 16384
#define ERROR_BYTES 2048
#define TOTAL_CAPTURE_BYTES 65536
#define MOUNT_BYTES (1024 * 1024)
#define RESULT_FD 3

#if defined(CODEX_OHOS_PLATFORM_DATA) && defined(CODEX_OHOS_RUNTIME_BASE)
#error "platform data observation cannot use a build-bound runtime root"
#endif

/* Ancestors only need searchable path handles. O_RDONLY would additionally
 * require listing permission, rejecting a trusted root-owned 0711 ancestor.
 * New test directories below these handles still use ordinary readable FDs.
 * No O_RDONLY fallback on OHOS: unsupported operations remain diagnostics.
 */
#if defined(__OHOS__)
#define ANCESTOR_OPEN_FLAGS (O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
#else
#define ANCESTOR_OPEN_FLAGS (O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
#endif

extern char **environ;
static FILE *result;

/* Preserve UTF-8; escape control bytes and invalid UTF-8 as visible \\xNN. */
static void json_string(FILE *out, const char *value) {
    const unsigned char *s = (const unsigned char *)(value ? value : "");
    fputc('"', out);
    while (*s) {
        unsigned char c = *s++;
        if (c == '"' || c == '\\') {
            fputc('\\', out);
            fputc(c, out);
        } else if (c < 32) {
            fprintf(out, "\\u%04x", c);
        } else if (c < 128) {
            fputc(c, out);
        } else {
            unsigned n = c >= 0xc2 && c <= 0xdf   ? 1
                         : c >= 0xe0 && c <= 0xef ? 2
                         : c >= 0xf0 && c <= 0xf4 ? 3
                                                  : 0;
            bool valid = n != 0;
            for (unsigned i = 0; valid && i < n; i++)
                valid = s[i] && (s[i] & 0xc0) == 0x80;
            if (valid && ((c == 0xe0 && s[0] < 0xa0) || (c == 0xed && s[0] >= 0xa0) ||
                          (c == 0xf0 && s[0] < 0x90) || (c == 0xf4 && s[0] >= 0x90)))
                valid = false;
            if (valid) {
                fputc(c, out);
                fwrite(s, 1, n, out);
                s += n;
            } else {
                fprintf(out, "\\\\x%02x", c);
            }
        }
    }
    fputc('"', out);
}

static void stage(const char *name) {
    fprintf(stderr, "stage=%s\n", name);
    fflush(stderr);
    if (result)
        fflush(result);
}

static void error_fields(int error) {
    fprintf(result, "\"errno\":%d,\"error\":", error);
    json_string(result, strerror(error));
}

static bool valid_path(const char *path) {
    if (!path || path[0] != '/' || strlen(path) >= PATH_BYTES)
        return false;
    const char *p = path + 1;
    while (*p) {
        const char *end = strchr(p, '/');
        size_t n = end ? (size_t)(end - p) : strlen(p);
        if ((n == 1 && p[0] == '.') || (n == 2 && p[0] == '.' && p[1] == '.'))
            return false;
        if (!end)
            break;
        p = end + 1;
    }
    return true;
}

static void metadata(const struct stat *st) {
    fprintf(result,
            "\"uid\":%ju,\"gid\":%ju,\"mode\":\"%04o\","
            "\"dev\":%ju,\"ino\":%ju,\"directory\":%s,\"symlink\":%s",
            (uintmax_t)st->st_uid, (uintmax_t)st->st_gid, (unsigned)(st->st_mode & 07777),
            (uintmax_t)st->st_dev, (uintmax_t)st->st_ino, S_ISDIR(st->st_mode) ? "true" : "false",
            S_ISLNK(st->st_mode) ? "true" : "false");
}

static bool safe_ancestor(const struct stat *st) {
    bool trusted_owner = st->st_uid == 0 || st->st_uid == geteuid();
    bool sticky_root = st->st_uid == 0 && (st->st_mode & S_ISVTX);
    return S_ISDIR(st->st_mode) && trusted_owner && (!(st->st_mode & 0022) || sticky_root);
}

/* Walk held directory FDs, recording each component without following links.
 * A successful policy check only permits a disposable test, not production use.
 */
static int inspect_chain(const char *path, bool *compatible) {
    *compatible = true;
    int fd = open("/", ANCESTOR_OPEN_FLAGS);
    fprintf(result, "[");
    struct stat st;
    if (fd < 0 || fstat(fd, &st)) {
        int error = errno;
        fprintf(result, "{\"path\":\"/\",");
        error_fields(error);
        fprintf(result, "}]");
        if (fd >= 0)
            close(fd);
        *compatible = false;
        return -1;
    }
    fprintf(result, "{\"path\":\"/\",");
    metadata(&st);
    fprintf(result, "}");
    *compatible = safe_ancestor(&st);
    char copy[PATH_BYTES], prefix[PATH_BYTES] = "";
    snprintf(copy, sizeof(copy), "%s", path);
    char *save = NULL;
    unsigned count = 1;
    for (char *part = strtok_r(copy, "/", &save); part; part = strtok_r(NULL, "/", &save)) {
        if (++count > MAX_ANCESTORS) {
            fprintf(result, ",{\"status\":\"ancestor_limit\"}");
            *compatible = false;
            close(fd);
            fd = -1;
            break;
        }
        size_t used = strlen(prefix);
        snprintf(prefix + used, sizeof(prefix) - used, "/%s", part);
        fprintf(result, ",{\"path\":");
        json_string(result, prefix);
        stage("fstatat-nofollow");
        if (fstatat(fd, part, &st, AT_SYMLINK_NOFOLLOW)) {
            int error = errno;
            fprintf(result, ",");
            error_fields(error);
            fprintf(result, "}");
            *compatible = false;
            close(fd);
            fd = -1;
            break;
        }
        fprintf(result, ",");
        metadata(&st);
        *compatible = *compatible && safe_ancestor(&st);
        if (S_ISLNK(st.st_mode)) {
            char target[PATH_BYTES];
            stage("readlinkat");
            ssize_t size = readlinkat(fd, part, target, sizeof(target) - 1);
            if (size >= 0) {
                target[size] = 0;
                fprintf(result, ",\"link_target\":");
                json_string(result, target);
                fprintf(result, ",\"link_target_may_be_truncated\":%s",
                        size == (ssize_t)sizeof(target) - 1 ? "true" : "false");
            } else {
                fprintf(result, ",");
                error_fields(errno);
            }
        }
        fprintf(result, "}");
        if (!S_ISDIR(st.st_mode) || S_ISLNK(st.st_mode)) {
            *compatible = false;
            close(fd);
            fd = -1;
            break;
        }
        stage("openat-nofollow");
        int next = openat(fd, part, ANCESTOR_OPEN_FLAGS);
        int open_error = errno;
        close(fd);
        fd = next;
        struct stat opened;
        if (fd < 0 || fstat(fd, &opened) || opened.st_dev != st.st_dev ||
            opened.st_ino != st.st_ino || !safe_ancestor(&opened)) {
            *compatible = false;
            if (fd < 0) {
                fprintf(result, ",{\"stage\":\"openat\",");
                error_fields(open_error);
                fprintf(result, "}");
                break;
            }
        }
    }
    fprintf(result, "]");
    return fd;
}

static bool covers(const char *mount, const char *path) {
    size_t n = strlen(mount);
    return !strcmp(mount, "/") || (!strncmp(mount, path, n) && (path[n] == 0 || path[n] == '/'));
}

static void decode_mount(char *text) {
    char *dest = text;
    for (char *p = text; *p; p++) {
        if (p[0] == '\\' && p[1] >= '0' && p[1] <= '7' && p[2] >= '0' && p[2] <= '7' &&
            p[3] >= '0' && p[3] <= '7') {
            *dest++ = (char)((p[1] - '0') * 64 + (p[2] - '0') * 8 + p[3] - '0');
            p += 3;
        } else {
            *dest++ = *p;
        }
    }
    *dest = 0;
}

static void mounts(const char *path, int fd) {
    fprintf(result, ",\"filesystem\":{");
    struct statfs fs;
    stage("fstatfs");
    if (fd >= 0 && !fstatfs(fd, &fs)) {
#if defined(__APPLE__)
        fprintf(result, "\"type_name\":");
        json_string(result, fs.f_fstypename);
#else
        fprintf(result, "\"type\":%ju", (uintmax_t)fs.f_type);
#endif
        fprintf(result, ",\"flags\":%ju", (uintmax_t)fs.f_flags);
    } else {
        error_fields(fd < 0 ? EBADF : errno);
    }
    fprintf(result,
            "},\"mountinfo\":{\"selection\":\"covering_candidate_path\","
            "\"complete_mount_table\":false,\"entry_limit\":8,\"input_byte_limit\":%d,",
            MOUNT_BYTES);
    stage("mountinfo");
    FILE *input = fopen("/proc/self/mountinfo", "r");
    if (!input) {
        error_fields(errno);
        fprintf(result, "}");
        return;
    }
    char line[4096];
    size_t bytes = 0;
    unsigned matched = 0;
    bool truncated = false;
    fprintf(result, "\"entries\":[");
    while (fgets(line, sizeof(line), input)) {
        bytes += strlen(line);
        if (bytes > MOUNT_BYTES || matched >= 8) {
            truncated = true;
            break;
        }
        if (!strchr(line, '\n')) {
            truncated = true;
            break;
        }
        unsigned id, parent;
        char device[64], root[1024], point[1024], options[256], type[64];
        int used = 0;
        if (sscanf(line, "%u %u %63s %1023s %1023s %255s %n", &id, &parent, device, root, point,
                   options, &used) != 6)
            continue;
        char *separator = strstr(line + used, "- ");
        if (!separator || sscanf(separator + 2, "%63s", type) != 1)
            continue;
        decode_mount(point);
        decode_mount(root);
        if (!covers(point, path))
            continue;
        if (matched++)
            fprintf(result, ",");
        fprintf(result, "{\"mount_id\":%u,\"parent_id\":%u,\"device\":", id, parent);
        json_string(result, device);
        fprintf(result, ",\"root\":");
        json_string(result, root);
        fprintf(result, ",\"mount_point\":");
        json_string(result, point);
        fprintf(result, ",\"type\":");
        json_string(result, type);
        char wrapped[260];
        snprintf(wrapped, sizeof(wrapped), ",%s,", options);
        fprintf(result, ",\"read_only\":%s,\"noexec\":%s,\"nosuid\":%s,\"nodev\":%s}",
                strstr(wrapped, ",ro,") ? "true" : "false",
                strstr(wrapped, ",noexec,") ? "true" : "false",
                strstr(wrapped, ",nosuid,") ? "true" : "false",
                strstr(wrapped, ",nodev,") ? "true" : "false");
    }
    fprintf(result, "],\"truncated\":%s}", truncated ? "true" : "false");
    fclose(input);
    if (fd >= 0) {
        char info_path[64];
        snprintf(info_path, sizeof(info_path), "/proc/self/fdinfo/%d", fd);
        stage("fdinfo");
        input = fopen(info_path, "r");
        fprintf(result, ",\"fdinfo\":{");
        if (!input) {
            error_fields(errno);
        } else {
            unsigned long long mount_id = 0;
            bool found = false;
            for (unsigned i = 0; i < 32 && fgets(line, sizeof(line), input); i++)
                if (sscanf(line, "mnt_id: %llu", &mount_id) == 1) {
                    found = true;
                    break;
                }
            if (found)
                fprintf(result, "\"mount_id\":%llu", mount_id);
            else
                fprintf(result, "\"status\":\"mount_id_unavailable\"");
            fclose(input);
        }
        fprintf(result, "}");
    }
}

static void path_observation(const char *path) {
    fprintf(result, "{\"path\":");
    json_string(result, path);
    fprintf(result, ",\"ancestors\":");
    bool compatible = false;
    int fd = inspect_chain(path, &compatible);
    fprintf(result, ",\"posix_ancestor_checks\":%s,\"runtime_root_approved\":false",
            compatible && fd >= 0 ? "true" : "false");
    stage("realpath");
    char *canonical = realpath(path, NULL);
    fprintf(result, ",\"canonical\":{");
    if (canonical && strlen(canonical) < PATH_BYTES) {
        fprintf(result, "\"path\":");
        json_string(result, canonical);
    } else {
        error_fields(canonical ? ENAMETOOLONG : errno);
    }
    fprintf(result, "}");
    if (canonical && strlen(canonical) < PATH_BYTES && strcmp(path, canonical)) {
        fprintf(result, ",\"canonical_ancestors\":");
        bool canonical_compatible;
        int resolved_fd = inspect_chain(canonical, &canonical_compatible);
        if (fd >= 0)
            close(fd);
        fd = resolved_fd;
        fprintf(result, ",\"canonical_posix_ancestor_checks\":%s",
                canonical_compatible && fd >= 0 ? "true" : "false");
    }
    mounts(canonical && strlen(canonical) < PATH_BYTES ? canonical : path, fd);
    if (fd >= 0)
        close(fd);
    free(canonical);
    fprintf(result, "}");
}

static void identity(void) {
    stage("identity");
    fprintf(result,
            "{\"pid\":%jd,\"ppid\":%jd,\"uid\":%ju,\"euid\":%ju,"
            "\"gid\":%ju,\"egid\":%ju,\"groups\":[",
            (intmax_t)getpid(), (intmax_t)getppid(), (uintmax_t)getuid(), (uintmax_t)geteuid(),
            (uintmax_t)getgid(), (uintmax_t)getegid());
    gid_t groups[MAX_GROUPS];
    int count = getgroups(MAX_GROUPS, groups);
    int group_error = count < 0 ? errno : 0;
    for (int i = 0; i < count; i++)
        fprintf(result, "%s%ju", i ? "," : "", (uintmax_t)groups[i]);
    fprintf(result, "],\"groups_errno\":%d,\"proc_status\":{", group_error);
    stage("proc-status");
    FILE *input = fopen("/proc/self/status", "r");
    if (!input) {
        error_fields(errno);
    } else {
        char line[1024];
        bool comma = false;
        for (unsigned i = 0; i < 128 && fgets(line, sizeof(line), input); i++) {
            const char *keys[] = {"Uid:", "Gid:", "Groups:", "NoNewPrivs:", "Seccomp:"};
            for (unsigned k = 0; k < sizeof(keys) / sizeof(keys[0]); k++) {
                if (strncmp(line, keys[k], strlen(keys[k])))
                    continue;
                if (comma)
                    fprintf(result, ",");
                comma = true;
                json_string(result, keys[k]);
                fprintf(result, ":");
                line[strcspn(line, "\n")] = 0;
                json_string(result, line + strlen(keys[k]));
            }
        }
        fclose(input);
    }
    fprintf(result, "}");
    char cwd[PATH_BYTES];
    stage("getcwd");
    if (getcwd(cwd, sizeof(cwd))) {
        fprintf(result, ",\"cwd\":");
        json_string(result, cwd);
    } else {
        fprintf(result, ",\"cwd_errno\":%d", errno);
    }
    fprintf(result, "}");
}

static void native_context(const char *symbol) {
    fprintf(result, "{\"symbol\":");
    json_string(result, symbol);
    if (strcmp(symbol, "OH_AbilityRuntime_ApplicationContextGetFilesDir") &&
        strcmp(symbol, "OH_AbilityRuntime_ApplicationContextGetCacheDir") &&
        strcmp(symbol, "OH_AbilityRuntime_ApplicationContextGetTempDir")) {
        fprintf(result, ",\"status\":\"invalid_symbol\"}");
        return;
    }
#if defined(__OHOS__) || defined(PROBE_TEST_CONTEXT_LIBRARY)
#if defined(PROBE_TEST_CONTEXT_LIBRARY)
    const char *library = PROBE_TEST_CONTEXT_LIBRARY;
    typedef int (*ContextFunction)(char *, int32_t, int32_t *);
#else
    const char *library = "libability_runtime.so";
    typedef HarmonyContextFunction ContextFunction;
#endif
    stage("native-dlopen");
    void *handle = dlopen(library, RTLD_NOW | RTLD_LOCAL);
    if (!handle) {
        fprintf(result, ",\"status\":\"library_unavailable\",\"loader_error\":");
        json_string(result, dlerror());
        fprintf(result, "}");
        return;
    }
    /* A fresh process performs exactly one query. Keep its library reference
     * until native_worker_exit() terminates the process without exit handlers.
     * A marker before dlclose in an older report does not prove dlclose itself
     * crashed; neither unloading nor process-exit destructors are needed here.
     */
    fprintf(result, ",\"library_lifetime\":\"retained_until_worker_exit\"");
    stage("native-library-retained");
    dlerror();
    stage("native-dlsym");
    void *address = dlsym(handle, symbol);
    const char *error = dlerror();
    if (error || !address) {
        fprintf(result, ",\"status\":\"symbol_unavailable\",\"loader_error\":");
        json_string(result, error);
        fprintf(result, "}");
        return;
    }
    ContextFunction function;
    _Static_assert(sizeof(function) == sizeof(address), "dlsym pointer ABI");
    memcpy(&function, &address, sizeof(function));
    char path[PATH_BYTES];
    memset(path, 0xff, sizeof(path));
    int32_t length = -1;
    stage("native-context-call");
    int code = function(path, sizeof(path), &length);
    stage("native-context-returned");
    fprintf(result, ",\"return_code\":%d,\"write_length\":%d,\"status\":", code, length);
    if (code != 0) {
        json_string(result, code == 16000011 ? "context_not_exist" : "api_error");
    } else if (length <= 0 || length >= (int32_t)sizeof(path) || !memchr(path, 0, sizeof(path)) ||
               !valid_path(path)) {
        json_string(result, "invalid_api_path");
    } else {
        json_string(result, "ok");
        fprintf(result, ",\"observation\":");
        path_observation(path);
    }
    fprintf(result, "}");
#else
    fprintf(result, ",\"status\":\"unsupported_platform\"}");
#endif
}

static bool same_entry(int parent, const char *name, const struct stat *expected) {
    struct stat current;
    return !fstatat(parent, name, &current, AT_SYMLINK_NOFOLLOW) &&
           current.st_dev == expected->st_dev && current.st_ino == expected->st_ino &&
           (current.st_mode & S_IFMT) == (expected->st_mode & S_IFMT);
}

static int create_test(const char *path) {
    fprintf(result, "{\"parent\":");
    json_string(result, path);
    fprintf(result, ",\"ancestors\":");
    bool compatible = false;
    int parent = inspect_chain(path, &compatible);
    if (parent < 0 || !compatible || strspn(path, "/") == strlen(path)) {
        if (parent >= 0)
            close(parent);
        fprintf(result, ",\"status\":\"parent_rejected\",\"created\":false,"
                        "\"runtime_root_approved\":false}");
        return 1;
    }
    stage("random-name");
    unsigned char random[16];
    int entropy = open("/dev/urandom", O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    ssize_t got = entropy >= 0 ? read(entropy, random, sizeof(random)) : -1;
    int random_error = errno;
    if (entropy >= 0)
        close(entropy);
    if (got != (ssize_t)sizeof(random)) {
        fprintf(result, ",\"status\":\"random_failed\",");
        error_fields(got < 0 ? random_error : EIO);
        fprintf(result, "}");
        close(parent);
        return 1;
    }
    char name[64] = ".codex-probe-";
    for (unsigned i = 0; i < sizeof(random); i++)
        snprintf(name + 13 + i * 2, 3, "%02x", random[i]);
    /* The prefix has 13 bytes including its trailing '-'. */
    fprintf(result, ",\"name\":");
    json_string(result, name);
    stage("mkdirat-0700");
    if (mkdirat(parent, name, 0700)) {
        fprintf(result, ",\"status\":\"mkdir_failed\",\"created\":false,");
        error_fields(errno);
        fprintf(result, "}");
        close(parent);
        return 1;
    }
    fprintf(result, ",\"created\":true");
    stage("open-created-directory");
    int directory = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    struct stat created;
    if (directory < 0 || fstat(directory, &created)) {
        fprintf(result, ",\"status\":\"open_created_failed\",\"cleanup\":\"unknown\",");
        error_fields(errno);
        fprintf(result, "}");
        if (directory >= 0)
            close(directory);
        close(parent);
        return 1;
    }
    fprintf(result, ",\"created_metadata\":{");
    metadata(&created);
    fprintf(result, "}");
    bool private = S_ISDIR(created.st_mode) && created.st_uid == geteuid() &&
                   (created.st_mode & 07777) == 0700 && same_entry(parent, name, &created);
    bool passed = private;
    bool file_created = false;
    struct stat file_identity;
    if (private) {
        stage("openat-exclusive-file");
        int file =
            openat(directory, "sample", O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
        if (file < 0) {
            passed = false;
            fprintf(result, ",\"file_errno\":%d", errno);
        } else {
            static const char sample[] = "Codex native path probe\n";
            char readback[sizeof(sample)] = {0};
            file_created = !fstat(file, &file_identity);
            int file_error = file_created ? 0 : errno;
            passed = file_created && file_identity.st_uid == geteuid() &&
                     (file_identity.st_mode & 07777) == 0600;
            if (file_created) {
                fprintf(result, ",\"file_metadata\":{");
                metadata(&file_identity);
                fprintf(result, "}");
            }
            stage("file-write-readback");
            if (passed) {
                ssize_t written = write(file, sample, sizeof(sample));
                if (written != (ssize_t)sizeof(sample)) {
                    passed = false;
                    file_error = written < 0 ? errno : EIO;
                }
            }
            if (passed && lseek(file, 0, SEEK_SET) != 0) {
                passed = false;
                file_error = errno;
            }
            if (passed) {
                ssize_t read_size = read(file, readback, sizeof(readback));
                if (read_size != (ssize_t)sizeof(readback)) {
                    passed = false;
                    file_error = read_size < 0 ? errno : EIO;
                }
            }
            if (passed && memcmp(sample, readback, sizeof(sample))) {
                passed = false;
                file_error = EIO;
            }
            fprintf(result, ",\"file_roundtrip\":%s", passed ? "true" : "false");
            fprintf(result, ",\"file_errno\":%d", file_error);
            close(file);
        }
    }
    bool socket_created = false;
    struct stat socket_identity;
    if (private) {
        stage("unix-socket-bind");
        struct sockaddr_un address = {0};
        address.sun_family = AF_UNIX;
        snprintf(address.sun_path, sizeof(address.sun_path), "probe.sock");
        int socket_fd = socket(AF_UNIX, SOCK_STREAM, 0);
        int socket_error = socket_fd < 0 ? errno : 0;
        mode_t old_umask = umask(0177);
        if (socket_fd >= 0 && !fchdir(directory) &&
            !bind(socket_fd, (struct sockaddr *)&address,
                  (socklen_t)(offsetof(struct sockaddr_un, sun_path) + strlen(address.sun_path) +
                              1))) {
            socket_created =
                !fstatat(directory, "probe.sock", &socket_identity, AT_SYMLINK_NOFOLLOW);
            if (!socket_created)
                socket_error = errno;
            else if (socket_identity.st_uid != geteuid() ||
                     (socket_identity.st_mode & 07777) != 0600)
                socket_error = EPERM;
        } else if (socket_fd >= 0)
            socket_error = errno;
        umask(old_umask);
        if (socket_fd >= 0)
            close(socket_fd);
        fprintf(result, ",\"socket_bind\":%s,\"socket_errno\":%d",
                socket_created && !socket_error ? "true" : "false", socket_error);
        passed = passed && socket_created && !socket_error;
    }
    stage("cleanup-owned-objects");
    bool clean = true;
    if (file_created)
        clean =
            same_entry(directory, "sample", &file_identity) && !unlinkat(directory, "sample", 0);
    if (socket_created) {
        bool removed = same_entry(directory, "probe.sock", &socket_identity) &&
                       !unlinkat(directory, "probe.sock", 0);
        clean = clean && removed;
    }
    /* No recursive removal and no deletion when a name now identifies another object. */
    clean = clean && same_entry(parent, name, &created) && !unlinkat(parent, name, AT_REMOVEDIR);
    fprintf(result,
            ",\"private_leaf\":%s,\"cleanup\":\"%s\",\"status\":\"%s\","
            "\"runtime_root_approved\":false}",
            private ? "true" : "false", clean ? "removed" : "incomplete",
            passed && clean ? "ok" : "failed");
    close(directory);
    close(parent);
    return passed && clean ? 0 : 1;
}

static int worker(const char *operation, const char *argument) {
    result = fdopen(RESULT_FD, "w");
    if (!result) {
        fprintf(stderr, "stage=worker-result-open-failed errno=%d\n", errno);
        return 1;
    }
    setvbuf(result, NULL, _IONBF, 0);
    stage(operation);
    int code = 0;
    if (!strcmp(operation, "identity"))
        identity();
    else if (!strcmp(operation, "path") && valid_path(argument))
        path_observation(argument);
    else if (!strcmp(operation, "native"))
        native_context(argument);
    else if (!strcmp(operation, "create") && valid_path(argument))
        code = create_test(argument);
    else
        code = 2;
    fprintf(result, "\n");
    /* _Exit does not flush stdio. Preserve earlier unbuffered write failures
     * as well as failures from these explicit final flush/close operations.
     */
    int flush_error = ferror(result) ? EIO : 0;
    if (fflush(result))
        flush_error = errno ? errno : EIO;
    fprintf(stderr, "stage=worker-result-flushed errno=%d\n", flush_error);
    int close_error = fclose(result) ? (errno ? errno : EIO) : 0;
    result = NULL;
    fprintf(stderr, "stage=worker-result-closed errno=%d\n", close_error);
    fflush(stderr);
    return flush_error || close_error ? 1 : code;
}

static _Noreturn void native_worker_exit(int code) {
    /* This applies only to the dedicated native-query worker, never the
     * supervisor. No signal handler or success substitution is involved.
     */
    fprintf(stderr, "stage=native-worker-_Exit code=%d\n", code);
    bool log_ok = !ferror(stderr);
    if (fflush(stderr))
        log_ok = false;
    if (fclose(stderr))
        log_ok = false;
    _Exit(log_ok ? code : 1);
}

static int64_t milliseconds(void) {
    struct timespec time;
    if (clock_gettime(CLOCK_MONOTONIC, &time))
        return -1;
    return (int64_t)time.tv_sec * 1000 + time.tv_nsec / 1000000;
}

static int high_pipe(int descriptors[2]) {
    int original[2];
    if (pipe(original))
        return -1;
    descriptors[0] = fcntl(original[0], F_DUPFD_CLOEXEC, 10);
    descriptors[1] = fcntl(original[1], F_DUPFD_CLOEXEC, 10);
    close(original[0]);
    close(original[1]);
    if (descriptors[0] < 0 || descriptors[1] < 0) {
        if (descriptors[0] >= 0)
            close(descriptors[0]);
        if (descriptors[1] >= 0)
            close(descriptors[1]);
        return -1;
    }
    return 0;
}

static void drain(int fd, char *buffer, size_t limit, size_t *size, bool *overflow) {
    char chunk[1024];
    for (unsigned i = 0; i < 32; i++) {
        ssize_t count = read(fd, chunk, sizeof(chunk));
        if (count <= 0)
            return;
        size_t keep = (size_t)count;
        if (keep > limit - *size) {
            keep = limit - *size;
            *overflow = true;
        }
        memcpy(buffer + *size, chunk, keep);
        *size += keep;
        buffer[*size] = 0;
    }
}

static int supervise(const char *executable, const char *operation, const char *argument,
                     int64_t deadline, size_t *remaining) {
    fprintf(stdout, "{\"stage\":");
    json_string(stdout, operation);
    fprintf(stdout, ",\"candidate\":");
    json_string(stdout, argument);
    fflush(stdout);
    int64_t now = milliseconds();
    if (now < 0 || now >= deadline || !*remaining) {
        fprintf(stdout, ",\"status\":\"budget_exhausted\"}");
        return 1;
    }
    int data[2], errors[2];
    if (high_pipe(data)) {
        fprintf(stdout, ",\"status\":\"pipe_failed\",\"errno\":%d}", errno);
        return 1;
    }
    if (high_pipe(errors)) {
        int error = errno;
        close(data[0]);
        close(data[1]);
        fprintf(stdout, ",\"status\":\"pipe_failed\",\"errno\":%d}", error);
        return 1;
    }
    posix_spawn_file_actions_t actions;
    int error = posix_spawn_file_actions_init(&actions);
    bool initialized = !error;
#define ACTION(call)                                                                               \
    do {                                                                                           \
        if (!error)                                                                                \
            error = (call);                                                                        \
    } while (0)
    ACTION(posix_spawn_file_actions_addopen(&actions, 0, "/dev/null", O_RDONLY, 0));
    ACTION(posix_spawn_file_actions_addopen(&actions, 1, "/dev/null", O_WRONLY, 0));
    ACTION(posix_spawn_file_actions_adddup2(&actions, data[1], RESULT_FD));
    ACTION(posix_spawn_file_actions_adddup2(&actions, errors[1], 2));
    ACTION(posix_spawn_file_actions_addclose(&actions, data[0]));
    ACTION(posix_spawn_file_actions_addclose(&actions, data[1]));
    ACTION(posix_spawn_file_actions_addclose(&actions, errors[0]));
    ACTION(posix_spawn_file_actions_addclose(&actions, errors[1]));
#undef ACTION
    char *arguments[] = {(char *)executable, "--probe-worker", (char *)operation, (char *)argument,
                         NULL};
    pid_t pid = -1;
    if (!error)
        error = posix_spawn(&pid, executable, &actions, NULL, arguments, environ);
    if (initialized)
        posix_spawn_file_actions_destroy(&actions);
    close(data[1]);
    close(errors[1]);
    if (error) {
        close(data[0]);
        close(errors[0]);
        fprintf(stdout, ",\"status\":\"spawn_failed\",\"errno\":%d}", error);
        return 1;
    }
    if (fcntl(data[0], F_SETFL, O_NONBLOCK) < 0 || fcntl(errors[0], F_SETFL, O_NONBLOCK) < 0) {
        int nonblock_error = errno;
        kill(pid, SIGKILL);
        close(data[0]);
        close(errors[0]);
        fprintf(stdout, ",\"status\":\"nonblocking_setup_failed\",\"errno\":%d}", nonblock_error);
        return 1;
    }
    char output[CAPTURE_BYTES + 1] = "", stderr_text[ERROR_BYTES + 1] = "";
    size_t output_size = 0, stderr_size = 0;
    size_t capacity = *remaining < CAPTURE_BYTES ? *remaining : CAPTURE_BYTES;
    bool overflow = false, stderr_overflow = false, done = false, timed_out = false;
    int status = 0;
    int64_t item_deadline = now + PROBE_ITEM_TIMEOUT_MS;
    if (item_deadline > deadline)
        item_deadline = deadline;
    while (!done) {
        drain(data[0], output, capacity, &output_size, &overflow);
        drain(errors[0], stderr_text, ERROR_BYTES, &stderr_size, &stderr_overflow);
        pid_t waited = waitpid(pid, &status, WNOHANG);
        if (waited == pid) {
            done = true;
            break;
        }
        if (waited < 0 && errno != EINTR)
            break;
        now = milliseconds();
        if (now < 0 || now >= item_deadline || overflow) {
            timed_out = !overflow;
            kill(pid, SIGKILL);
            /* Bounded reap: an uninterruptible filesystem call may outlive SIGKILL. */
            for (unsigned i = 0; i < 10; i++) {
                if (waitpid(pid, &status, WNOHANG) == pid) {
                    done = true;
                    break;
                }
                struct timespec delay = {0, 1000000};
                nanosleep(&delay, NULL);
            }
            break;
        }
        struct pollfd fds[] = {{data[0], POLLIN, 0}, {errors[0], POLLIN, 0}};
        poll(fds, 2, 10);
    }
    drain(data[0], output, capacity, &output_size, &overflow);
    drain(errors[0], stderr_text, ERROR_BYTES, &stderr_size, &stderr_overflow);
    close(data[0]);
    close(errors[0]);
    *remaining -= output_size;
    bool complete = done && !timed_out && !overflow && WIFEXITED(status) &&
                    (WEXITSTATUS(status) == 0 || WEXITSTATUS(status) == 1) && output_size >= 3 &&
                    output[0] == '{' && output[output_size - 2] == '}';
    bool ok = complete && WEXITSTATUS(status) == 0;
    fprintf(stdout, ",\"status\":\"%s\",\"pid\":%jd,\"timed_out\":%s,\"output_truncated\":%s,",
            ok ? "collected" : "incomplete", (intmax_t)pid, timed_out ? "true" : "false",
            overflow ? "true" : "false");
    if (done) {
        fprintf(stdout, "\"raw_wait_status\":%d,\"exit_code\":", status);
        if (WIFEXITED(status))
            fprintf(stdout, "%d", WEXITSTATUS(status));
        else
            fprintf(stdout, "null");
        fprintf(stdout, ",\"signal\":");
        if (WIFSIGNALED(status))
            fprintf(stdout, "%d", WTERMSIG(status));
        else
            fprintf(stdout, "null");
    } else
        fprintf(stdout, "\"raw_wait_status\":null,\"exit_code\":null,\"signal\":null");
    fprintf(stdout, ",\"stderr_truncated\":%s,\"stderr\":", stderr_overflow ? "true" : "false");
    json_string(stdout, stderr_text);
    fprintf(stdout, complete ? ",\"result\":%s" : ",\"partial_output\":", complete ? output : "");
    if (!complete)
        json_string(stdout, output);
    fprintf(stdout, "}");
    return ok ? 0 : 1;
}

static int self_path(char *path) {
#if defined(__APPLE__)
    uint32_t size = PATH_BYTES;
    return _NSGetExecutablePath(path, &size);
#else
    ssize_t size = readlink("/proc/self/exe", path, PATH_BYTES - 1);
    if (size <= 0 || size >= PATH_BYTES - 1)
        return -1;
    path[size] = 0;
    return 0;
#endif
}

int main(int argc, char **argv) {
    if (argc == 4 && !strcmp(argv[1], "--probe-worker")) {
        int code = worker(argv[2], argv[3]);
        if (!strcmp(argv[2], "native"))
            native_worker_exit(code);
        return code;
    }
    const char *paths[MAX_PATHS], *create = NULL;
    size_t count = 0;
    for (int i = 1; i < argc; i++) {
        if (!strcmp(argv[i], "--help")) {
            puts("Usage: harmony-runtime-probe [--json] [--path ABSOLUTE_DIR]... [--create-test "
                 "ABSOLUTE_PARENT]\n"
                 "Default: read-only JSON; at most 12 explicit paths replace default candidates.\n"
                 "Platform builds observe files/codex/r/{a,s} namespace candidates.\n"
                 "Fixed-root diagnostic builds observe base/cEUID/{a,s} instead.\n"
                 "Candidates are read-only observations, not the CLI's validated selection.\n"
                 "Create-test only touches new disposable objects under a checked parent.\n"
                 "Exit 0: observations collected (not a safe-root approval); 1: incomplete/create "
                 "failure; 2: invalid arguments.");
            return 0;
        }
        if (!strcmp(argv[i], "--json"))
            continue;
        if (!strcmp(argv[i], "--path") && i + 1 < argc && count < MAX_PATHS &&
            valid_path(argv[i + 1])) {
            paths[count++] = argv[++i];
            continue;
        }
        if (!strcmp(argv[i], "--create-test") && !create && i + 1 < argc &&
            valid_path(argv[i + 1])) {
            create = argv[++i];
            continue;
        }
        fputs("Invalid arguments; use --help. Paths must be absolute, shorter than 1024 bytes, "
              "without dot components.\n",
              stderr);
        return 2;
    }
    bool explicit_paths = count != 0;
    char codex_home[PATH_BYTES];
    const char *runtime_selection = "not_configured";
#if defined(CODEX_OHOS_PLATFORM_DATA)
    runtime_selection = "explicit_paths_override";
    if (!explicit_paths) {
        /* These are the platform namespace candidates, not a claimed Context
         * result or an approved root. Native Context observations below run in
         * isolated workers, where a bad platform call cannot hang this process.
         * Keep paths independent of environment variables and numeric UIDs.
         */
        paths[count++] = "/data/storage/el2/base/files/codex/r/a";
        paths[count++] = "/data/storage/el2/base/files/codex/r/s";
        runtime_selection = "platform_namespace_candidates";
    }
#elif defined(CODEX_OHOS_RUNTIME_BASE)
    char runtime_paths[2][PATH_BYTES];
    const char *base = CODEX_OHOS_RUNTIME_BASE;
    runtime_selection = "explicit_paths_override";
    if (!explicit_paths) {
        size_t length = strlen(base);
        while (length && base[length - 1] == '/')
            length--;
        if (!length || !valid_path(base)) {
            runtime_selection = "invalid_base";
        } else {
            /* Match ProtectedRuntimeDirectory's c{euid:08x}/{a,s} names.
             * Use only the build contract and native identity; never a shell,
             * environment override, directory creation, or root approval.
             */
            int a = snprintf(runtime_paths[0], PATH_BYTES, "%.*s/c%08jx/a", (int)length, base,
                             (uintmax_t)geteuid());
            int s = snprintf(runtime_paths[1], PATH_BYTES, "%.*s/c%08jx/s", (int)length, base,
                             (uintmax_t)geteuid());
            if (a < 0 || s < 0 || a >= PATH_BYTES || s >= PATH_BYTES) {
                runtime_selection = "path_too_long";
            } else {
                paths[count++] = runtime_paths[0];
                paths[count++] = runtime_paths[1];
                runtime_selection = "selected";
            }
        }
    }
#endif
    if (!explicit_paths) {
        const char *keys[] = {"HOME", "CODEX_HOME", "TMPDIR"};
        for (unsigned k = 0; k < 3; k++) {
            const char *value = getenv(keys[k]);
            if (valid_path(value))
                paths[count++] = value;
        }
        const char *home = getenv("HOME");
        if (!getenv("CODEX_HOME") && valid_path(home) && strlen(home) + 8 < sizeof(codex_home)) {
            snprintf(codex_home, sizeof(codex_home), "%s/.codex", home);
            paths[count++] = codex_home;
        }
        const char *fixed[] = {"/storage/Users/currentUser", "/data/storage/el2/base/files",
                               "/data/storage/el2/base/cache", "/data/storage/el2/base/temp",
                               "/dev/shm"};
        _Static_assert(sizeof(fixed) / sizeof(fixed[0]) + 3 + 2 <= MAX_PATHS,
                       "default candidates including runtime a/s fit MAX_PATHS");
        for (unsigned i = 0; i < sizeof(fixed) / sizeof(fixed[0]); i++)
            paths[count++] = fixed[i];
    }
    char executable[PATH_BYTES];
    if (self_path(executable)) {
        puts("{\"schema_version\":1,\"status\":\"self_path_unavailable\",\"runtime_root_approved\":"
             "false}");
        return 1;
    }
    size_t remaining = TOTAL_CAPTURE_BYTES;
    int64_t started = milliseconds(), deadline = started + PROBE_TOTAL_TIMEOUT_MS;
    fprintf(stdout, "{\"schema_version\":1,\"build_id\":");
    json_string(stdout, CODEX_HARMONY_BUILD_ID);
    fprintf(stdout, ",\"version\":");
    json_string(stdout, CODEX_HARMONY_VERSION);
    fprintf(stdout,
            ",\"read_only\":%s,\"runtime_root_approved\":false,\"supervisor_pid\":%jd,"
            "\"supervisor_euid\":%ju,\"path_selection\":{\"mode\":\"%s\","
            "\"selected_count\":%zu,\"limit\":%d,\"runtime_paths\":",
            create ? "false" : "true", (intmax_t)getpid(), (uintmax_t)geteuid(),
            explicit_paths ? "explicit" : "defaults", count, MAX_PATHS);
    json_string(stdout, runtime_selection);
    fprintf(stdout, "},\"checks\":[");
    int failures = supervise(executable, "identity", "", deadline, &remaining);
    const char *symbols[] = {"OH_AbilityRuntime_ApplicationContextGetFilesDir",
                             "OH_AbilityRuntime_ApplicationContextGetCacheDir",
                             "OH_AbilityRuntime_ApplicationContextGetTempDir"};
    for (unsigned i = 0; i < 3; i++) {
        fprintf(stdout, ",");
        failures += supervise(executable, "native", symbols[i], deadline, &remaining);
    }
    for (size_t i = 0; i < count; i++) {
        fprintf(stdout, ",");
        failures += supervise(executable, "path", paths[i], deadline, &remaining);
    }
    if (create) {
        fprintf(stdout, ",");
        failures += supervise(executable, "create", create, deadline, &remaining);
    }
    fprintf(stdout, "],\"incomplete_checks\":%d,\"status\":\"%s\"}\n", failures,
            failures ? "incomplete" : "collected");
    return failures ? 1 : 0;
}
