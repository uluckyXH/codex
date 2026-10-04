#include "pty_session.h"
#include "native_package.h"
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <fcntl.h>
#include <mutex>
#include <pty.h>
#include <signal.h>
#include <spawn.h>
#include <string>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <vector>

extern char **environ;
namespace codex_hnp {
namespace {
constexpr const char *kFiles = "/data/storage/el2/base/files";
constexpr unsigned kUid = 20020059;
using Clock = std::chrono::steady_clock;

class Descriptor {
public:
    explicit Descriptor(int value = -1) : value_(value) {}
    ~Descriptor() { if (value_ >= 0) close(value_); }
    Descriptor(const Descriptor &) = delete;
    Descriptor &operator=(const Descriptor &) = delete;
    int get() const { return value_; }
    int release() { int value = value_; value_ = -1; return value; }
    void reset(int value) { if (value_ >= 0) close(value_); value_ = value; }
private:
    int value_;
};

bool ValidSize(int columns, int rows) {
    return columns > 0 && rows > 0 && columns <= 1000 && rows <= 1000 && columns * rows <= 100000;
}

bool PrivateDirectory(int descriptor) {
    struct stat metadata = {};
    return fstat(descriptor, &metadata) == 0 && S_ISDIR(metadata.st_mode) &&
        metadata.st_uid == kUid && metadata.st_gid == kUid && (metadata.st_mode & 07777) == 0700;
}

// Inspect existing application directories only. This component never chmods,
// creates, repairs, removes, imports, or reads configuration file contents.
bool PreparedDirectories() {
    if (geteuid() != kUid || getegid() != kUid) { errno = EACCES; return false; }
    Descriptor current(open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (current.get() < 0) return false;
    const char *components[] = {"data", "storage", "el2", "base", "files"};
    for (unsigned index = 0; index < 5; ++index) {
        Descriptor next(openat(current.get(), components[index], O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
        if (next.get() < 0) return false;
        struct stat metadata = {};
        if (fstat(next.get(), &metadata) != 0) return false;
        if (!S_ISDIR(metadata.st_mode) ||
            (index < 3 && (metadata.st_uid != 0 || (metadata.st_mode & 07022) != 0)) ||
            (index >= 3 && !PrivateDirectory(next.get()))) { errno = EACCES; return false; }
        current.reset(next.release());
    }
    for (const char *name : {"home", "state", "workspace", "tmp", "logs", "r"}) {
        Descriptor child(openat(current.get(), name, O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
        if (child.get() < 0) return false;
        if (!PrivateDirectory(child.get())) { errno = EACCES; return false; }
    }
    return true;
}

std::vector<char *> Pointers(std::vector<std::string> &values) {
    std::vector<char *> result;
    for (std::string &value : values) result.push_back(value.data());
    result.push_back(nullptr);
    return result;
}

std::string Base64(const char *bytes, size_t size) {
    static const char alphabet[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    std::string output;
    output.reserve((size + 2) / 3 * 4);
    for (size_t index = 0; index < size; index += 3) {
        unsigned value = static_cast<unsigned char>(bytes[index]) << 16;
        if (index + 1 < size) value |= static_cast<unsigned char>(bytes[index + 1]) << 8;
        if (index + 2 < size) value |= static_cast<unsigned char>(bytes[index + 2]);
        output += alphabet[(value >> 18) & 63];
        output += alphabet[(value >> 12) & 63];
        output += index + 1 < size ? alphabet[(value >> 6) & 63] : '=';
        output += index + 2 < size ? alphabet[value & 63] : '=';
    }
    return output;
}

class Session {
public:
    Session() : watcher_([this] { Watch(); }) {}
    ~Session() {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            shutdown_ = true;
            RequestStop();
        }
        changed_.notify_one();
        watcher_.join();
    }

    std::string Start(int columns, int rows) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (child_ > 0) return Result(false, "already_running");
        if (!ValidSize(columns, rows)) return Result(false, "invalid_dimensions");
        if (!PreparedDirectories()) return Result(false, "private_directories_not_prepared", errno);
        master_.reset(-1); observer_.reset(-1);
        eof_ = true; stopping_ = false; exitCode_ = -1; signal_ = 0; error_.clear(); input_.clear();
        int master = -1, slave = -1;
        char name[128] = {};
        if (openpty(&master, &slave, name, nullptr, nullptr) != 0) return Result(false, "openpty", errno);
        Descriptor masterGuard(master), slaveGuard(slave);
        if (fcntl(master, F_SETFD, FD_CLOEXEC) != 0 || fcntl(slave, F_SETFD, FD_CLOEXEC) != 0 ||
            fcntl(master, F_SETFL, fcntl(master, F_GETFL) | O_NONBLOCK) != 0) return Result(false, "pty_flags", errno);
        struct winsize size = {}, observed = {};
        size.ws_col = static_cast<unsigned short>(columns); size.ws_row = static_cast<unsigned short>(rows);
        if (ioctl(slave, TIOCSWINSZ, &size) != 0 || ioctl(slave, TIOCGWINSZ, &observed) != 0 ||
            observed.ws_col != columns || observed.ws_row != rows) return Result(false, "pty_dimensions", errno);

        std::string package = CODEX_HNP_PACKAGE_PATH;
        std::string workspace = std::string(kFiles) + "/workspace";
        // The shell script is fixed. No user input or credential is interpolated.
        // The terminal only exposes the fixed Codex process after this exec.
        std::vector<std::string> args = {"/system/bin/sh", "-c", "umask 077; cd \"$1\" || exit 125; shift; exec \"$@\"", "codex-hnp-terminal", workspace,
            package + "/bin/codex", "--model", "gpt-5.6-terra", "--sandbox", "read-only", "--ask-for-approval", "on-request", "--cd", workspace,
            "-c", "log_dir=\"/data/storage/el2/base/files/state/log\""};
        std::vector<std::string> env;
        const char *replaced[] = {"HOME=", "CODEX_HOME=", "TMPDIR=", "PATH=", "TERM=", "SHELL=", "LINES=", "COLUMNS="};
        for (char **entry = environ; *entry; ++entry) {
            bool replace = false;
            for (const char *key : replaced) if (strncmp(*entry, key, strlen(key)) == 0) replace = true;
            if (!replace) env.emplace_back(*entry);
        }
        env.push_back(std::string("HOME=") + kFiles + "/home");
        env.push_back(std::string("CODEX_HOME=") + kFiles + "/state");
        env.push_back(std::string("TMPDIR=") + kFiles + "/tmp");
        env.push_back("PATH=" + package + "/codex-path:/system/bin:/system/xbin:/bin");
        env.push_back("TERM=xterm-256color"); env.push_back("SHELL=/system/bin/sh");
        env.push_back("COLUMNS=" + std::to_string(columns)); env.push_back("LINES=" + std::to_string(rows));
        auto argv = Pointers(args); auto envp = Pointers(env);
        posix_spawnattr_t attributes;
        posix_spawn_file_actions_t actions;
        int result = posix_spawnattr_init(&attributes);
        if (result) return Result(false, "spawn_attributes", result);
        result = posix_spawn_file_actions_init(&actions);
        if (result) { posix_spawnattr_destroy(&attributes); return Result(false, "spawn_actions", result); }
        sigset_t empty, defaults; sigemptyset(&empty); sigemptyset(&defaults);
        for (int number : {SIGINT, SIGTERM, SIGHUP, SIGWINCH, SIGPIPE}) sigaddset(&defaults, number);
        result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSID | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF);
        if (!result) result = posix_spawnattr_setsigmask(&attributes, &empty);
        if (!result) result = posix_spawnattr_setsigdefault(&attributes, &defaults);
        if (!result) result = posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, name, O_RDWR, 0);
        if (!result) result = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDOUT_FILENO);
        if (!result) result = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDERR_FILENO);
        pid_t child = -1;
        if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
        posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attributes);
        if (result) return Result(false, "posix_spawn", result);
        child_ = child; master_.reset(masterGuard.release());
        // HarmonyOS permits reading size from the slave, but denies the same
        // ioctl on the master. Keep our original CLOEXEC slave for queries.
        observer_.reset(slaveGuard.release()); eof_ = false; ++generation_;
        changed_.notify_one();
        return Result(true);
    }

    std::string Write(const std::string &bytes) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (bytes.empty() || bytes.size() > 16384) return Result(false, "input_size");
        if (child_ <= 0 || stopping_ || master_.get() < 0) return Result(false, "not_running");
        if (input_.size() + bytes.size() > 65536) return Result(false, "input_queue_full");
        // The entire call is accepted before any bytes are sent. The watcher
        // retains partial-write remainders and retries only those bytes.
        input_.append(bytes);
        changed_.notify_one();
        return Result(true, "", 0, "", static_cast<int>(bytes.size()));
    }

    std::string Resize(int columns, int rows) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!ValidSize(columns, rows)) return Result(false, "invalid_dimensions");
        if (child_ <= 0 || master_.get() < 0 || observer_.get() < 0) return Result(false, "not_running");
        struct winsize size = {}, observed = {};
        size.ws_col = static_cast<unsigned short>(columns); size.ws_row = static_cast<unsigned short>(rows);
        if (ioctl(master_.get(), TIOCSWINSZ, &size) != 0) return Result(false, "resize_set", errno);
        if (ioctl(observer_.get(), TIOCGWINSZ, &observed) != 0) return Result(false, "resize_get_slave", errno);
        if (observed.ws_col != columns || observed.ws_row != rows) return Result(false, "resize_mismatch", EIO);
        return Result(true);
    }

    std::string Read() {
        std::lock_guard<std::mutex> lock(mutex_);
        if (master_.get() < 0) return Result(true);
        char bytes[16384];
        ssize_t count = read(master_.get(), bytes, sizeof(bytes));
        if (count > 0) return Result(true, "", 0, Base64(bytes, static_cast<size_t>(count)));
        if (count == 0 || (count < 0 && errno == EIO) ||
            (count < 0 && errno == EAGAIN && child_ <= 0)) {
            eof_ = true; master_.reset(-1); return Result(true);
        }
        if (count < 0 && (errno == EAGAIN || errno == EINTR)) return Result(true);
        return Result(false, "read", errno);
    }

    std::string Stop() {
        std::lock_guard<std::mutex> lock(mutex_);
        RequestStop(); changed_.notify_one(); return Result(true);
    }

    std::string Status() {
        std::lock_guard<std::mutex> lock(mutex_); return Result(true);
    }

private:
    std::string Result(bool ok, const char *error = "", int number = 0, const std::string &data = "", int accepted = 0) {
        std::string status = child_ > 0 ? (stopping_ ? "stopping" : "running") : (generation_ ? "exited" : "idle");
        std::string failure = *error ? error : error_;
        return std::string("{\"ok\":") + (ok ? "true" : "false") +
            ",\"status\":\"" + status + "\",\"running\":" + (child_ > 0 ? "true" : "false") +
            ",\"eof\":" + (eof_ ? "true" : "false") + ",\"exitCode\":" + (exitCode_ >= 0 ? std::to_string(exitCode_) : "null") +
            ",\"signal\":" + std::to_string(signal_) + ",\"generation\":" + std::to_string(generation_) +
            ",\"accepted\":" + std::to_string(accepted) + ",\"dataBase64\":\"" + data +
            "\",\"error\":\"" + failure + "\",\"errno\":" + std::to_string(number) + "}";
    }

    void SignalOwned(int number) {
        if (child_ <= 0) return;
        // Until waitpid reaps this exact child, its PID cannot be recycled.
        if (getpgid(child_) == child_) kill(-child_, number);
        else kill(child_, number);
    }

    void RequestStop() {
        if (child_ <= 0 || stopping_) return;
        input_.clear(); stopping_ = true; stopDeadline_ = Clock::now() + std::chrono::seconds(1);
        SignalOwned(SIGTERM);
    }

    void Watch() {
        std::unique_lock<std::mutex> lock(mutex_);
        for (;;) {
            if (child_ > 0) {
                if (stopping_ && Clock::now() >= stopDeadline_) SignalOwned(SIGKILL);
                int status = 0;
                pid_t result = waitpid(child_, &status, WNOHANG);
                if (result == child_) {
                    exitCode_ = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
                    signal_ = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
                    child_ = -1; stopping_ = false; observer_.reset(-1);
                } else if (result < 0 && errno != EINTR) {
                    error_ = "waitpid"; child_ = -1; stopping_ = false; observer_.reset(-1);
                }
            }
            if (child_ > 0 && !stopping_ && !input_.empty()) {
                ssize_t count = write(master_.get(), input_.data(), input_.size());
                if (count > 0) input_.erase(0, static_cast<size_t>(count));
                else if (count < 0 && errno != EAGAIN && errno != EINTR) {
                    error_ = "queued_write"; RequestStop();
                }
            }
            if (shutdown_ && child_ <= 0) return;
            changed_.wait_for(lock, std::chrono::milliseconds(50));
        }
    }

    std::mutex mutex_;
    std::condition_variable changed_;
    Descriptor master_;
    Descriptor observer_;
    pid_t child_ = -1;
    int exitCode_ = -1;
    int signal_ = 0;
    unsigned generation_ = 0;
    bool eof_ = true;
    bool stopping_ = false;
    bool shutdown_ = false;
    std::string error_;
    std::string input_;
    Clock::time_point stopDeadline_;
    std::thread watcher_;
};

Session &Current() { static Session session; return session; }
}
std::string PtyStart(int columns, int rows) { return Current().Start(columns, rows); }
std::string PtyWrite(const std::string &bytes) { return Current().Write(bytes); }
std::string PtyResize(int columns, int rows) { return Current().Resize(columns, rows); }
std::string PtyRead() { return Current().Read(); }
std::string PtyStop() { return Current().Stop(); }
std::string PtyStatus() { return Current().Status(); }
}
