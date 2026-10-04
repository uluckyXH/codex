#include "host_layout.h"
#include "host_json.h"
#include "owned_processes.h"
#include "native_package.h"
#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#if defined(__OHOS__)
#include <AbilityKit/ability_runtime/application_context.h>
#endif

extern char **environ;
namespace codex_hnp {
Descriptor::~Descriptor() { reset(); }
Descriptor &Descriptor::operator=(Descriptor &&other) noexcept {
    if (this != &other) reset(other.release());
    return *this;
}
void Descriptor::reset(int fd) { if (fd_ >= 0) close(fd_); fd_ = fd; }
bool SameObject(const struct stat &a, const struct stat &b) { return a.st_dev == b.st_dev && a.st_ino == b.st_ino; }
bool PrivateDirectory(int fd) {
    struct stat st = {};
    return fstat(fd, &st) == 0 && S_ISDIR(st.st_mode) && st.st_uid == geteuid() &&
           (st.st_mode & 07777) == 0700;
}
bool ValidTerminalSize(int columns, int rows) {
    return columns > 0 && rows > 0 && columns <= 1000 && rows <= 1000 && columns * rows <= 100000;
}
std::string JsonString(const std::string &value) {
    std::string result = "\"";
    const char *hex = "0123456789abcdef";
    for (unsigned char byte : value) {
        if (byte == '\\' || byte == '"') { result += '\\'; result += byte; }
        else if (byte < 32) { result += "\\u00"; result += hex[byte >> 4]; result += hex[byte & 15]; }
        else result += byte;
    }
    return result + "\"";
}
std::string Base64(const char *bytes, size_t size) {
    static const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string output;
    for (size_t i = 0; i < size; i += 3) {
        unsigned value = static_cast<unsigned char>(bytes[i]) << 16;
        if (i + 1 < size) value |= static_cast<unsigned char>(bytes[i + 1]) << 8;
        if (i + 2 < size) value |= static_cast<unsigned char>(bytes[i + 2]);
        output += alphabet[(value >> 18) & 63]; output += alphabet[(value >> 12) & 63];
        output += i + 1 < size ? alphabet[(value >> 6) & 63] : '=';
        output += i + 2 < size ? alphabet[value & 63] : '=';
    }
    return output;
}
std::vector<char *> ArgumentPointers(std::vector<std::string> &values) {
    std::vector<char *> result;
    for (auto &value : values) result.push_back(value.data());
    result.push_back(nullptr); return result;
}
std::vector<std::string> HostEnvironment(const std::string &root, const std::string &term) {
    const char *keys[] = {"HOME=", "CODEX_HOME=", "TMPDIR=", "PATH=", "TERM=", "SHELL=", "LINES=", "COLUMNS="};
    std::vector<std::string> values;
    for (char **entry = environ; *entry; ++entry) {
        bool replace = false;
        for (const char *key : keys) if (strncmp(*entry, key, strlen(key)) == 0) replace = true;
        if (!replace) values.emplace_back(*entry);
    }
    values.push_back("HOME=" + root + "/host/home");
    values.push_back("CODEX_HOME=" + root + "/state");
    values.push_back("TMPDIR=" + root + "/tmp");
    values.push_back(std::string("PATH=") + CODEX_HNP_PACKAGE_PATH + "/bin:" + CODEX_HNP_PACKAGE_PATH +
                     "/codex-path:/system/bin:/system/xbin:/bin");
    values.push_back("TERM=" + term); values.push_back("SHELL=/system/bin/sh");
    return values;
}

static bool Fail(std::string &error, const char *stage, int number = 0) {
    if (number) errno = number;
    error = std::string(stage) + " errno=" + std::to_string(errno); return false;
}
static bool ValidPath(const std::string &path) {
    if (path.size() < 2 || path.size() >= PATH_MAX || path.front() != '/' || path.back() == '/') return false;
    size_t start = 1;
    while (start < path.size()) {
        size_t end = path.find('/', start);
        std::string part = path.substr(start, end == std::string::npos ? end : end - start);
        if (part.empty() || part == "." || part == "..") return false;
        for (unsigned char byte : part) if (byte < 32 || byte == 127) return false;
        if (end == std::string::npos) break;
        start = end + 1;
    }
    return true;
}
bool VerifyApplicationContext(const std::string &files, std::string &error) {
    if (!ValidPath(files)) return Fail(error, "context_path", EINVAL);
#if defined(__OHOS__)
    char buffer[PATH_MAX] = {}; int32_t length = 0;
    auto result = OH_AbilityRuntime_ApplicationContextGetFilesDir(buffer, sizeof(buffer), &length);
    if (result != ABILITY_RUNTIME_ERROR_CODE_NO_ERROR)
        return Fail(error, "native_context_unavailable", ENOTSUP);
    size_t actual = strnlen(buffer, sizeof(buffer));
    if (length <= 0 || actual >= sizeof(buffer) ||
        (static_cast<size_t>(length) != actual && static_cast<size_t>(length) != actual + 1) || files != buffer)
        return Fail(error, "context_source_mismatch", EACCES);
    return true;
#else
    return Fail(error, "native_context_unavailable", ENOTSUP);
#endif
}
static bool Metadata(int fd, bool privateDir) {
    struct stat st = {};
    if (fstat(fd, &st) != 0 || !S_ISDIR(st.st_mode)) { errno = EACCES; return false; }
    if (privateDir) return PrivateDirectory(fd) || (errno = EACCES, false);
    // Match the CLI strict ancestor policy. Root-owned system ancestors and
    // this app's ancestors are accepted only when nobody else can write them.
    if ((st.st_uid != 0 && st.st_uid != geteuid()) || (st.st_mode & 0022) || (st.st_mode & 07000)) {
        errno = EACCES; return false;
    }
    return true;
}
static int OpenEdge(int parent, const std::string &name, bool readAccess) {
    struct stat before = {}, held = {}, after = {};
    if (fstatat(parent, name.c_str(), &before, AT_SYMLINK_NOFOLLOW) != 0) return -1;
    int fd = openat(parent, name.c_str(), (readAccess ? O_RDONLY : O_PATH) | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return -1;
    if (fstat(fd, &held) != 0 || fstatat(parent, name.c_str(), &after, AT_SYMLINK_NOFOLLOW) != 0 ||
        !SameObject(before, held) || !SameObject(held, after)) {
        close(fd); errno = ESTALE; return -1;
    }
    return fd;
}
bool HostLayout::Append(int parent, const std::string &name, const std::string &relative,
                        bool privateDir, bool create, std::string &error) {
    if (create && mkdirat(edges_[parent].fd.get(), name.c_str(), 0700) != 0 && errno != EEXIST)
        return Fail(error, "host_mkdir");
    Descriptor fd(OpenEdge(edges_[parent].fd.get(), name, privateDir));
    if (fd.get() < 0) {
        error = "directory_open component=" + name + " errno=" + std::to_string(errno); return false;
    }
    if (!Metadata(fd.get(), privateDir)) {
        struct stat st = {};
        if (fstat(fd.get(), &st) == 0) {
            char detail[256];
            snprintf(detail, sizeof(detail), "directory_validate component=%s owner=%u group=%u mode=%04o euid=%u egid=%u required=%s errno=%d",
                name.c_str(), st.st_uid, st.st_gid, st.st_mode & 07777, geteuid(), getegid(), privateDir ? "private0700" : "protected-ancestor", EACCES);
            error = detail;
        } else error = "directory_stat component=" + name + " errno=" + std::to_string(errno);
        errno = EACCES; return false;
    }
    edges_.push_back({std::move(fd), name, relative, parent, privateDir}); return true;
}
bool HostLayout::Open(const std::string &files, bool initialize, std::string &error) {
    edges_.clear(); initializationUnverified_ = false; uid_ = geteuid(); gid_ = getegid();
    if (!VerifyApplicationContext(files, error)) return false;
    files_ = files; root_ = files + "/codex";
    Descriptor root(open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (root.get() < 0 || !Metadata(root.get(), false)) return Fail(error, "root_validate");
    edges_.push_back({std::move(root), "", "@root", -1, false});
    size_t start = 1; int parent = 0;
    while (start < files.size()) {
        size_t end = files.find('/', start); bool last = end == std::string::npos;
        std::string name = files.substr(start, last ? end : end - start);
        if (!Append(parent, name, last ? "@files" : "@ancestor", last, false, error)) return false;
        parent = static_cast<int>(edges_.size()) - 1;
        if (last) break;
        start = end + 1;
    }
    if (!Revalidate(error)) return false;
    // The explicit doctor entry initializes before config/arg0. No API key,
    // login, prompt, GUI marker or model request is involved.
    if (initialize && !InitializeCliDirectories(files, error, initializationUnverified_)) return false;
    if (!Append(parent, "codex", "", true, false, error)) return false;
    int data = static_cast<int>(edges_.size()) - 1;
    for (const char *name : {"state", "r", "tmp", "logs"})
        if (!Append(data, name, name, true, false, error)) return false;
    int runtime = -1;
    for (size_t i = 0; i < edges_.size(); ++i) if (edges_[i].relative == "r") runtime = static_cast<int>(i);
    for (const char *name : {"a", "s"})
        if (!Append(runtime, name, std::string("r/") + name, true, false, error)) return false;
    for (const char *name : {"workspace", "host"})
        if (!Append(data, name, name, true, initialize, error)) return false;
    int host = static_cast<int>(edges_.size()) - 1;
    for (const char *name : {"control", "handoff-private", "home"})
        if (!Append(host, name, std::string("host/") + name, true, initialize, error)) return false;
    return Revalidate(error);
}
int HostLayout::Fd(const std::string &relative) const {
    for (const auto &edge : edges_) if (edge.relative == relative) return edge.fd.get();
    errno = ENOENT; return -1;
}
bool HostLayout::Revalidate(std::string &error) const {
    if (uid_ != geteuid() || gid_ != getegid()) return Fail(error, "identity_changed", EACCES);
    std::vector<Descriptor> reopened;
    for (const auto &edge : edges_) {
        Descriptor fd(edge.parent < 0 ? open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC) :
                      OpenEdge(reopened[edge.parent].get(), edge.name, edge.privateDirectory));
        struct stat held = {}, named = {};
        if (fd.get() < 0 || !Metadata(fd.get(), edge.privateDirectory) || !Metadata(edge.fd.get(), edge.privateDirectory) ||
            fstat(fd.get(), &named) != 0 || fstat(edge.fd.get(), &held) != 0 || !SameObject(held, named))
            return Fail(error, "directory_replaced_or_unsafe", ESTALE);
        reopened.push_back(std::move(fd));
    }
    return true;
}

namespace {
bool CapturePipe(Descriptor &reader, Descriptor &writer) {
    int pair[2];
    if (pipe(pair) != 0) return false;
    reader.reset(pair[0]); writer.reset(pair[1]);
    int flags = fcntl(reader.get(), F_GETFL);
    return flags >= 0 && fcntl(reader.get(), F_SETFD, FD_CLOEXEC) == 0 &&
        fcntl(writer.get(), F_SETFD, FD_CLOEXEC) == 0 && fcntl(reader.get(), F_SETFL, flags | O_NONBLOCK) == 0;
}
void Drain(Descriptor &reader, std::string &contents, bool &truncated, int &readError) {
    if (reader.get() < 0) return;
    char bytes[4096];
    for (unsigned round = 0; round < 16; ++round) {
        ssize_t count = read(reader.get(), bytes, sizeof(bytes));
        if (count > 0) {
            size_t keep = std::min(static_cast<size_t>(count), 16384 - contents.size());
            contents.append(bytes, keep); if (keep < static_cast<size_t>(count)) truncated = true;
        } else if (count == 0) { reader.reset(); return; }
        else if (errno == EAGAIN) return;
        else if (errno != EINTR) { readError = errno; reader.reset(); return; }
    }
}
}
bool InitializeCliDirectories(const std::string &files, std::string &error, bool &cleanupUnknown) {
    cleanupUnknown = false;
    struct sigaction disposition = {};
    if (sigaction(SIGCHLD, nullptr, &disposition) != 0 || disposition.sa_handler == SIG_IGN ||
        (disposition.sa_flags & SA_NOCLDWAIT)) return Fail(error, "initialize_child_wait_unavailable", ECHILD);
    Descriptor stdoutRead, stdoutWrite, stderrRead, stderrWrite;
    if (!CapturePipe(stdoutRead, stdoutWrite) || !CapturePipe(stderrRead, stderrWrite)) return Fail(error, "initialize_pipe");
    std::vector<std::string> args = {std::string(CODEX_HNP_PACKAGE_PATH) + "/bin/codex", "doctor", "--initialize-data-directories", "--json"};
    auto values = HostEnvironment(files + "/codex", "dumb");
    auto argv = ArgumentPointers(args); auto envp = ArgumentPointers(values);
    posix_spawn_file_actions_t actions; posix_spawnattr_t attributes;
    int result = posix_spawn_file_actions_init(&actions);
    if (result) return Fail(error, "initialize_actions", result);
    result = posix_spawnattr_init(&attributes);
    if (result) { posix_spawn_file_actions_destroy(&actions); return Fail(error, "initialize_attributes", result); }
    result = posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, stdoutWrite.get(), STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, stderrWrite.get(), STDERR_FILENO);
    if (!result) result = posix_spawnattr_setpgroup(&attributes, 0);
    if (!result) result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETPGROUP);
    pid_t child = -1;
    if (!result) result = posix_spawn(&child, args[0].c_str(), &actions, &attributes, argv.data(), envp.data());
    posix_spawnattr_destroy(&attributes); posix_spawn_file_actions_destroy(&actions);
    stdoutWrite.reset(); stderrWrite.reset();
    if (result) return Fail(error, "initialize_spawn", result);
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(10);
    auto reapedDeadline = deadline; bool killed = false, reaped = false;
    int status = 0, readError = 0; std::string output, diagnostics; bool truncated = false;
    for (;;) {
        Drain(stdoutRead, output, truncated, readError); Drain(stderrRead, diagnostics, truncated, readError);
        if (!reaped) {
            pid_t waited = waitpid(child, &status, WNOHANG);
            if (waited == child) { reaped = true; reapedDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(1); }
            else if (waited < 0 && errno != EINTR) {
                cleanupUnknown = true; return Fail(error, "initialize_waitpid");
            }
        }
        if (reaped && stdoutRead.get() < 0 && stderrRead.get() < 0) break;
        auto now = std::chrono::steady_clock::now();
        if (reaped && now >= reapedDeadline) {
            cleanupUnknown = true; error = "initialize_pipe_still_owned_after_exit\n" + diagnostics; errno = ETIMEDOUT; return false;
        }
        if (!reaped && !killed && now >= deadline) {
            std::string failure;
            if (!SignalDirectChild(child, SIGKILL, failure)) {
                cleanupUnknown = true; error = "initialize_timeout " + failure + "\n" + diagnostics; return false;
            }
            killed = true;
        }
        if (!reaped && killed && now >= deadline + std::chrono::seconds(3)) {
            cleanupUnknown = true; error = "initialize_kill_unconfirmed\n" + diagnostics; errno = ETIMEDOUT; return false;
        }
        struct pollfd events[2] = {{stdoutRead.get(), POLLIN, 0}, {stderrRead.get(), POLLIN, 0}};
        int polled = poll(events, 2, 20);
        if (polled < 0 && errno != EINTR) readError = errno;
    }
    if (killed || !WIFEXITED(status) || WEXITSTATUS(status) != 0 || truncated || readError) {
        error = "cli_initialize_exit=" + std::to_string(WIFEXITED(status) ? WEXITSTATUS(status) : -1) +
                " signal=" + std::to_string(WIFSIGNALED(status) ? WTERMSIG(status) : 0) +
                " capture_errno=" + std::to_string(readError) + " truncated=" + (truncated ? "yes" : "no") + "\n" + diagnostics;
        errno = EIO; return false;
    }
    std::string root;
    if (!JsonPathString(output, {"paths", "root"}, root) || root != files + "/codex") {
        error = "cli_initialize_root_mismatch\n" + diagnostics + "\ninitializer_stdout=" + output; errno = EACCES; return false;
    }
    return true;
}

bool SecureApplicationContext(const std::string &files, std::string &evidence, std::string &error) {
    if (!VerifyApplicationContext(files, error)) return false;
    size_t slash = files.rfind('/'); std::string base = files.substr(0, slash);
    if (files.substr(slash + 1) != "files" || base.substr(base.rfind('/') + 1) != "base")
        return Fail(error, "secure_context_scope", EACCES);
    struct Pin { Descriptor fd; std::string name; bool pair; struct stat original; };
    std::vector<Pin> pins;
    Descriptor root(open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (root.get() < 0 || !Metadata(root.get(), false)) return Fail(error, "secure_root");
    struct stat rootStat = {}; if (fstat(root.get(), &rootStat) != 0) return Fail(error, "secure_root_stat");
    pins.push_back({std::move(root), "", false, rootStat});
    size_t start = 1;
    // Validate and pin the complete source before the first permission change.
    while (start < files.size()) {
        size_t end = files.find('/', start); bool last = end == std::string::npos;
        std::string part = files.substr(start, last ? end : end - start);
        bool pair = last || files.substr(0, end) == base;
        Descriptor next(OpenEdge(pins.back().fd.get(), part, pair)); struct stat st = {};
        if (next.get() < 0 || fstat(next.get(), &st) != 0) return Fail(error, "secure_context_open");
        if (pair) {
            if (st.st_uid != geteuid() ||
                ((st.st_mode & 07777) != 0777 && (st.st_mode & 07777) != 0700))
                return Fail(error, "secure_context_owner_or_mode", EACCES);
            char record[256];
            snprintf(record, sizeof(record), "context_%s_before_uid=%u gid=%u mode=%04o dev=%llu ino=%llu\n",
                part.c_str(), st.st_uid, st.st_gid, st.st_mode & 07777,
                static_cast<unsigned long long>(st.st_dev), static_cast<unsigned long long>(st.st_ino));
            evidence += record;
        } else if (!Metadata(next.get(), false)) return Fail(error, "secure_context_ancestor");
        pins.push_back({std::move(next), part, pair, st});
        if (last) break;
        start = end + 1;
    }
    auto recheck = [&]() {
        if (!VerifyApplicationContext(files, error)) return false;
        Descriptor current(open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
        for (size_t index = 0; index < pins.size(); ++index) {
            if (index) current.reset(OpenEdge(current.get(), pins[index].name, pins[index].pair));
            struct stat named = {}, held = {};
            if (current.get() < 0 || fstat(current.get(), &named) != 0 || fstat(pins[index].fd.get(), &held) != 0 ||
                !SameObject(named, pins[index].original) || !SameObject(named, held) ||
                named.st_uid != pins[index].original.st_uid || named.st_gid != pins[index].original.st_gid)
                return Fail(error, "secure_context_replaced", ESTALE);
            if (pins[index].pair) {
                if (named.st_uid != geteuid() ||
                    ((named.st_mode & 07777) != 0700 && (named.st_mode & 07777) != 0777))
                    return Fail(error, "secure_context_mode_changed", EACCES);
            } else if (!Metadata(current.get(), false)) return Fail(error, "secure_context_ancestor_changed");
        }
        return true;
    };
    for (auto &pin : pins) {
        if (!pin.pair) continue;
        if (!recheck()) return false;
        // Narrow explicit GUI exception: only this native Context's app-owned
        // files/base pair. CLI never performs this chmod, and no HOME/project,
        // system ancestor, foreign identity or guessed UID path is accepted.
        if (fchmod(pin.fd.get(), 0700) != 0 || !PrivateDirectory(pin.fd.get()))
            return Fail(error, "secure_context_chmod");
        evidence += "context_" + pin.name + "_after_mode=0700\n";
    }
    return recheck();
}
}
