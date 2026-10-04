#include "pty_session.h"
#include "host_layout.h"
#include "native_package.h"
#include "owned_processes.h"
#include <algorithm>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <mutex>
#include <poll.h>
#include <pty.h>
#include <signal.h>
#include <spawn.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <termios.h>
#include <thread>
#include <unistd.h>
#include <vector>

namespace codex_hnp {
namespace {
using Clock = std::chrono::steady_clock;

bool Token(std::string &token) {
    Descriptor input(open("/dev/urandom", O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
    unsigned char bytes[16]; size_t offset = 0;
    while (input.get() >= 0 && offset < sizeof(bytes)) {
        ssize_t count = read(input.get(), bytes + offset, sizeof(bytes) - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) return false;
        offset += static_cast<size_t>(count);
    }
    if (offset != sizeof(bytes)) return false;
    token.clear(); const char *hex = "0123456789abcdef";
    for (unsigned char byte : bytes) { token += hex[byte >> 4]; token += hex[byte & 15]; }
    return true;
}

class ShellStartup {
public:
    ~ShellStartup() { (void)Remove(); }
    bool Prepare(int directory, const std::string &token) {
        directory_ = directory; name_ = ".shell-startup-" + token;
        file_.reset(openat(directory_, name_.c_str(), O_RDWR | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
        if (file_.get() < 0) { name_.clear(); return false; }
        struct stat metadata = {};
        if (fstat(file_.get(), &metadata) != 0) return false;
        if (!S_ISREG(metadata.st_mode) || metadata.st_uid != geteuid() || metadata.st_nlink != 1 ||
            (metadata.st_mode & 07777) != 0600) { errno = EACCES; return false; }
        // Only builtins, in the already interactive shell. cwd is a quoted
        // dedicated environment value, available before positional setup,
        // never interpolated into executable text. No second shell exec.
        const char script[] = "umask 077 || exit 125\n"
            "[ -n \"$CODEX_TERMINAL_CWD\" ] || exit 125\n"
            "cd \"$CODEX_TERMINAL_CWD\" || exit 125\nunset CODEX_TERMINAL_CWD ENV\n"
            "printf 'ready\\n' >&3\nexec 3>&-\n";
        size_t offset = 0;
        while (offset < sizeof(script) - 1) {
            ssize_t count = write(file_.get(), script + offset, sizeof(script) - 1 - offset);
            if (count < 0 && errno == EINTR) continue;
            if (count <= 0) { if (!count) errno = EIO; return false; }
            offset += static_cast<size_t>(count);
        }
        if (fsync(file_.get()) != 0) return false;
        int descriptors[2];
        if (pipe(descriptors) != 0) return false;
        Descriptor reader(descriptors[0]), writer(descriptors[1]);
        // Reserve child fd 3 without relying on the parent's descriptor layout.
        reader_.reset(fcntl(reader.get(), F_DUPFD_CLOEXEC, 10));
        if (reader_.get() < 0) return false;
        writer_.reset(fcntl(writer.get(), F_DUPFD_CLOEXEC, 10));
        if (writer_.get() < 0) return false;
        int flags = fcntl(reader_.get(), F_GETFL);
        return flags >= 0 && fcntl(reader_.get(), F_SETFL, flags | O_NONBLOCK) == 0;
    }
    std::string Environment(const std::string &root) const { return "ENV=" + root + "/host/control/" + name_; }
    int AddActions(posix_spawn_file_actions_t &actions) const {
        int result = posix_spawn_file_actions_adddup2(&actions, writer_.get(), 3);
        if (!result && reader_.get() != 3) result = posix_spawn_file_actions_addclose(&actions, reader_.get());
        if (!result) result = posix_spawn_file_actions_addclose(&actions, writer_.get());
        return result;
    }
    void Spawned() { writer_.reset(); }
    bool Ready() {
        auto deadline = Clock::now() + std::chrono::seconds(3); std::string received;
        while (received.size() < 6) {
            int remaining = static_cast<int>(std::chrono::duration_cast<std::chrono::milliseconds>(deadline - Clock::now()).count());
            if (remaining <= 0) { errno = ETIMEDOUT; return false; }
            struct pollfd event = {reader_.get(), POLLIN, 0};
            int result = poll(&event, 1, remaining);
            if (result < 0 && errno == EINTR) continue;
            if (result < 0) return false;
            if (!result) { errno = ETIMEDOUT; return false; }
            char bytes[7]; ssize_t count = read(reader_.get(), bytes, sizeof(bytes));
            if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
            if (count <= 0) { if (!count) errno = EPIPE; return false; }
            received.append(bytes, static_cast<size_t>(count));
        }
        reader_.reset();
        if (received != "ready\n") { errno = EPROTO; return false; }
        return true;
    }
    bool Remove() {
        if (name_.empty()) return true;
        struct stat pinned = {}, named = {};
        if (fstat(file_.get(), &pinned) != 0) return false;
        if (fstatat(directory_, name_.c_str(), &named, AT_SYMLINK_NOFOLLOW) != 0) {
            if (errno != ENOENT) return false;
        } else {
            if (!SameObject(pinned, named)) { errno = ESTALE; return false; }
            if (unlinkat(directory_, name_.c_str(), 0) != 0) return false;
        }
        name_.clear(); file_.reset(); return true;
    }
private:
    int directory_ = -1; std::string name_;
    Descriptor file_, reader_, writer_;
};

class Session {
public:
    explicit Session(std::string kind) : kind_(std::move(kind)), watcher_([this] { Watch(); }) {}
    ~Session() {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            shutdown_ = true; shutdownDeadline_ = Clock::now() + std::chrono::seconds(6);
            RequestStop("host_shutdown", true);
        }
        changed_.notify_one(); watcher_.join();
    }

    std::string Start(const std::string &files, const std::string &cwd, const std::string &policy, int columns, int rows) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (child_ > 0 || cleanupUnknown_) return Result(false, "already_running_or_cleanup_unverified");
        if (!ValidTerminalSize(columns, rows)) return Result(false, "invalid_dimensions");
        if (policy != "read-only" && policy != "workspace-write" && policy != "danger-full-access")
            return Result(false, "invalid_policy");
        auto layout = std::make_unique<HostLayout>(); std::string failure;
        if (!layout->Open(files, false, failure)) return Result(false, "directory_preparation_required", errno);
        if (!StableProcessHandlesAvailable(failure)) return Result(false, "stable_process_handles_unavailable", errno);
        struct sigaction disposition = {};
        if (sigaction(SIGCHLD, nullptr, &disposition) != 0 || disposition.sa_handler == SIG_IGN ||
            (disposition.sa_flags & SA_NOCLDWAIT)) return Result(false, "child_wait_unavailable", ECHILD);
        std::string working = cwd.empty() ? layout->Root() + "/workspace" : cwd;
        if (working.front() != '/' || working.size() > 4095 || working.find('\0') != std::string::npos)
            return Result(false, "invalid_working_directory", EINVAL);
        Descriptor project(open(working.c_str(), O_PATH | O_DIRECTORY | O_CLOEXEC));
        if (project.get() < 0 || faccessat(project.get(), ".", X_OK, AT_EACCESS) != 0)
            return Result(false, "working_directory_access", errno);
        std::string nextId;
        if (!Token(nextId)) return Result(false, "session_token", errno);
        int master = -1, slave = -1; char name[128] = {};
        if (openpty(&master, &slave, name, nullptr, nullptr) != 0) return Result(false, "openpty", errno);
        Descriptor masterGuard(master), slaveGuard(slave);
        int flags = fcntl(master, F_GETFL);
        if (flags < 0 || fcntl(master, F_SETFD, FD_CLOEXEC) != 0 || fcntl(slave, F_SETFD, FD_CLOEXEC) != 0 ||
            fcntl(master, F_SETFL, flags | O_NONBLOCK) != 0) return Result(false, "pty_flags", errno);
        struct winsize size = {}, observed = {};
        size.ws_col = static_cast<unsigned short>(columns); size.ws_row = static_cast<unsigned short>(rows);
        if (ioctl(slave, TIOCSWINSZ, &size) != 0 || ioctl(slave, TIOCGWINSZ, &observed) != 0 ||
            observed.ws_col != columns || observed.ws_row != rows) return Result(false, "pty_dimensions", errno);
        struct termios initialSettings = {}, launchSettings = {};
        if (tcgetattr(slave, &initialSettings) != 0) return Result(false, "pty_initial_attributes", errno);
        launchSettings = initialSettings;
        if (kind_ == "shell") {
            // Only this freshly allocated slave is changed. An interactive sh
            // must not inherit a platform default of noncanonical VMIN=0.
            launchSettings.c_iflag = (launchSettings.c_iflag & ~(IGNCR | INLCR)) | ICRNL;
            launchSettings.c_oflag |= OPOST | ONLCR;
            launchSettings.c_lflag |= ICANON | ISIG | IEXTEN | ECHO | ECHOE | ECHOK;
            launchSettings.c_cflag |= CREAD;
            launchSettings.c_cc[VINTR] = 3; launchSettings.c_cc[VQUIT] = 28;
            launchSettings.c_cc[VERASE] = 127; launchSettings.c_cc[VKILL] = 21;
            launchSettings.c_cc[VEOF] = 4; launchSettings.c_cc[VSTART] = 17;
            launchSettings.c_cc[VSTOP] = 19; launchSettings.c_cc[VSUSP] = 26;
            launchSettings.c_cc[VMIN] = 1; launchSettings.c_cc[VTIME] = 0;
            if (tcsetattr(slave, TCSANOW, &launchSettings) != 0 || tcgetattr(slave, &launchSettings) != 0)
                return Result(false, "pty_shell_attributes", errno);
            if ((launchSettings.c_lflag & (ICANON | ISIG | ECHO)) != (ICANON | ISIG | ECHO) ||
                launchSettings.c_cc[VMIN] != 1 || launchSettings.c_cc[VTIME] != 0 || launchSettings.c_cc[VEOF] != 4)
                return Result(false, "pty_shell_attributes_mismatch", EIO);
        }
        std::string package = CODEX_HNP_PACKAGE_PATH;
        std::unique_ptr<ShellStartup> shellStartup;
        std::vector<std::string> args;
        if (kind_ == "shell") {
            // SDK exposes no spawn chdir/umask actions. ENV initializes this
            // shell before input; its private acknowledgement proves both ran.
            if (getuid() != geteuid() || getgid() != getegid()) return Result(false, "shell_startup_identity", EACCES);
            shellStartup = std::make_unique<ShellStartup>();
            if (!shellStartup->Prepare(layout->Fd("host/control"), nextId)) return Result(false, "shell_startup_prepare", errno);
            args = {"/system/bin/sh", "-i", "-s"};
        } else {
            args = {"/system/bin/sh", "-c", "umask 077; cd \"$1\" || exit 125; shift; exec \"$@\"", "codex-terminal", working,
                    package + "/bin/codex", "--model", "gpt-5.6-terra", "--sandbox", policy,
                    "--ask-for-approval", "on-request", "--cd", working};
        }
        auto values = HostEnvironment(layout->Root(), "xterm-256color");
        if (shellStartup) {
            values.erase(std::remove_if(values.begin(), values.end(), [](const std::string &value) {
                return value.compare(0, 4, "ENV=") == 0 || value.compare(0, 9, "BASH_ENV=") == 0 ||
                    value.compare(0, 19, "CODEX_TERMINAL_CWD=") == 0;
            }), values.end());
            values.push_back(shellStartup->Environment(layout->Root()));
            values.push_back("CODEX_TERMINAL_CWD=" + working);
        }
        values.push_back("COLUMNS=" + std::to_string(columns)); values.push_back("LINES=" + std::to_string(rows));
        auto argv = ArgumentPointers(args); auto envp = ArgumentPointers(values);
        std::string reportName = "terminal-" + kind_ + "-" + nextId + ".status.txt";
        Descriptor nextReport(openat(layout->Fd("logs"), reportName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
        if (nextReport.get() < 0) return Result(false, "diagnostic_open", errno);
        posix_spawnattr_t attributes; posix_spawn_file_actions_t actions;
        int result = posix_spawnattr_init(&attributes);
        if (result) return Result(false, "spawn_attributes", result);
        result = posix_spawn_file_actions_init(&actions);
        if (result) { posix_spawnattr_destroy(&attributes); return Result(false, "spawn_actions", result); }
        sigset_t empty, defaults; sigemptyset(&empty); sigemptyset(&defaults);
        for (int number : {SIGINT, SIGTERM, SIGHUP, SIGWINCH, SIGPIPE, SIGTSTP, SIGTTIN, SIGTTOU, SIGCHLD})
            sigaddset(&defaults, number);
        result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSID | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF);
        if (!result) result = posix_spawnattr_setsigmask(&attributes, &empty);
        if (!result) result = posix_spawnattr_setsigdefault(&attributes, &defaults);
        // Opening the slave after setsid creates this session's controlling TTY.
        if (!result) result = posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, name, O_RDWR, 0);
        if (!result) result = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDOUT_FILENO);
        if (!result) result = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDERR_FILENO);
        if (!result && shellStartup) result = shellStartup->AddActions(actions);
        if (!result && !layout->Revalidate(failure)) result = errno;
        pid_t child = -1;
        if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
        posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attributes);
        if (shellStartup) shellStartup->Spawned();
        if (result) return Result(false, "posix_spawn", result);
        master_.reset(masterGuard.release()); observer_.reset(slaveGuard.release());
        report_ = std::move(nextReport);
        shellStartup_.reset();
        layout_ = std::move(layout); child_ = child; id_ = nextId; cwd_ = working; policy_ = policy;
        shellStartup_ = std::move(shellStartup);
        eof_ = false; exitCode_ = -1; signal_ = 0; rawStatus_ = -1; waitErrno_ = 0; signalErrno_ = 0;
        error_.clear(); errorDetail_.clear(); lastAnchorDetail_.clear(); lastVisibilityDetail_.clear(); errorNumber_ = 0;
        input_.clear(); finalOutput_.clear(); stopping_ = false; leaderExited_ = false;
        cleanupVerified_ = false; cleanupUnknown_ = false; cleanupPaused_ = false; nextObserve_ = Clock::now();
        procVisibilityUnknown_ = false; procScanErrno_ = 0;
        started_ = Clock::now(); reportRecords_ = 0; waitObserved_ = false; reaped_ = false; waitCode_ = 0; waitStatus_ = 0;
        firstRead_ = false; firstReadEof_ = false; firstReadEio_ = false; firstOutput_ = false;
        readCount_ = 0; readErrno_ = 0;
        group_ = child_; session_ = child_; foreground_ = -1;
        Log("spawn", 0);
        Log(kind_ == "shell" ? "shell_direct_spawn" : "codex_bootstrap_spawn", 0);
        LogTerminal("pty_initial_attributes", initialSettings);
        LogTerminal(kind_ == "shell" ? "pty_shell_attributes" : "pty_launch_attributes", launchSettings);
        LogConnection();
        bool pinned = owned_.Begin(child_, failure);
        int beginError = pinned ? 0 : errno;
        std::string beginFailure = failure;
        if (!pinned) {
            SetError("process_identity", beginError, beginFailure); cleanupUnknown_ = true;
            // Retain child/session ownership on failure; never report it stopped.
            // Rejecting startup permits one guarded direct-child pidfd kill.
            // Later retries only use already pinned handles, never reopen a PID.
            std::string signalFailure;
            bool sent = SignalDirectChild(child_, SIGKILL, signalFailure);
            signalErrno_ = sent ? 0 : errno;
            Log("start_failed_direct_child_kill", signalErrno_, signalFailure);
            RequestStop("start_failed");
            SetError("process_identity", beginError, beginFailure);
        }
        bool ready = true; int startupError = 0;
        if (pinned && shellStartup_) {
            ready = shellStartup_->Ready(); startupError = ready ? 0 : errno;
            Log("shell_initialization", startupError);
            if (!ready) {
                RequestStop("shell_initialization_failed"); SetError("shell_initialization", startupError);
            } else if (!shellStartup_->Remove()) Log("shell_startup_remove", errno);
            else shellStartup_.reset();
        }
        changed_.notify_one();
        return Result(pinned && ready, !pinned ? "process_identity" : !ready ? "shell_initialization" : "", !pinned ? beginError : startupError);
    }

    std::string Write(const std::string &id, const std::string &bytes) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        if (bytes.empty() || bytes.size() > 16384) return Result(false, "input_size");
        if (child_ <= 0 || stopping_ || leaderExited_ || master_.get() < 0) return Result(false, "not_running");
        if (input_.size() + bytes.size() > 65536) return Result(false, "input_queue_full");
        input_.append(bytes); changed_.notify_one(); return Result(true, "", 0, "", static_cast<int>(bytes.size()));
    }
    std::string Resize(const std::string &id, int columns, int rows) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        if (!ValidTerminalSize(columns, rows)) return Result(false, "invalid_dimensions");
        if (child_ <= 0 || observer_.get() < 0 || master_.get() < 0) return Result(false, "not_running");
        struct winsize size = {}, observed = {};
        size.ws_col = static_cast<unsigned short>(columns); size.ws_row = static_cast<unsigned short>(rows);
        if (ioctl(master_.get(), TIOCSWINSZ, &size) != 0) return Result(false, "resize_set", errno);
        if (ioctl(observer_.get(), TIOCGWINSZ, &observed) != 0) return Result(false, "resize_get", errno);
        if (observed.ws_col != columns || observed.ws_row != rows) return Result(false, "resize_mismatch", EIO);
        return Result(true);
    }
    std::string Read(const std::string &id) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        if (!finalOutput_.empty()) {
            std::string bytes = finalOutput_.substr(0, 16384); finalOutput_.erase(0, bytes.size());
            eof_ = finalOutput_.empty(); return Result(true, "", 0, Base64(bytes.data(), bytes.size()));
        }
        if (master_.get() < 0) return Result(true);
        char bytes[16384]; ssize_t count = read(master_.get(), bytes, sizeof(bytes));
        int readError = count < 0 ? errno : 0;
        readCount_ = count; readErrno_ = readError;
        if (!firstRead_) { firstRead_ = true; Log("pty_first_read", readError); }
        if (count > 0) {
            if (!firstOutput_) { firstOutput_ = true; Log("pty_first_output", 0); }
            return Result(true, "", 0, Base64(bytes, static_cast<size_t>(count)));
        }
        if (!count && !firstReadEof_) { firstReadEof_ = true; Log("pty_read_zero", 0); LogCurrentTerminal("pty_read_zero_attributes"); }
        if (count < 0 && readError == EIO && !firstReadEio_) {
            firstReadEio_ = true; Log("pty_read_eio", readError); LogCurrentTerminal("pty_read_eio_attributes");
        }
        // A transient EOF/EIO is not child exit or cleanup proof. Closing the
        // master here can itself hang up a still-starting interactive shell.
        // Only verified cleanup releases it; retain terminal output and token.
        if (!count || (count < 0 && (readError == EIO || readError == EAGAIN || readError == EINTR))) return Result(true);
        SetError("pty_read", readError); return Result(false, "pty_read", readError);
    }
    std::string Control(const std::string &id, const std::string &operation) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        if (operation == "interrupt") {
            if (child_ <= 0 || stopping_ || leaderExited_ || observer_.get() < 0) return Result(false, "not_running");
            errno = 0; foreground_ = tcgetpgrp(master_.get());
            int queryErrno = foreground_ < 0 ? errno : 0; Log("interrupt_foreground", queryErrno);
            if (foreground_ < 0) {
                errno = 0; foreground_ = tcgetpgrp(observer_.get());
                Log("interrupt_foreground_slave", foreground_ < 0 ? errno : 0);
            }
            // The PTY's line discipline (or Codex raw-mode handler) routes the
            // control byte. A denied group query is recorded, never replaced
            // by signalling a guessed/cached numeric foreground PID.
            struct termios settings = {};
            if (tcgetattr(observer_.get(), &settings) != 0) return Result(false, "terminal_attributes", errno);
            unsigned char control = (settings.c_lflag & ISIG) ? settings.c_cc[VINTR] : 3;
            if (control == _POSIX_VDISABLE) return Result(false, "interrupt_disabled");
            if (input_.size() >= 65536) return Result(false, "input_queue_full");
            input_.push_back(static_cast<char>(control)); changed_.notify_one();
            return Result(true, "", 0, "", 1);
        }
        if (operation != "close" && operation != "exit-codex") return Result(false, "invalid_operation");
        if (operation == "exit-codex" && kind_ != "codex") return Result(false, "not_codex_session");
        if (cleanupVerified_) return Result(true);
        if (child_ <= 0) return Result(false, "ownership_unverified", waitErrno_);
        RequestStop(operation.c_str(), true); changed_.notify_one();
        return Result(error_.empty());
    }
    std::string Status(const std::string &id) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        return Result(true);
    }
private:
    bool Matches(const std::string &id) const { return id == id_ && (!id.empty() || child_ <= 0); }
    void Log(const char *stage, int error, const std::string &detail = "") {
        if (report_.get() < 0) return;
        // Repeated failures and explicit retries cannot grow a report forever.
        if (reportRecords_ >= 256) {
            if (reportRecords_ == 256) {
                ++reportRecords_;
                const char limit[] = "stage=diagnostic_record_limit records=256\n";
                ssize_t ignored = write(report_.get(), limit, sizeof(limit) - 1); (void)ignored;
            }
            return;
        }
        ++reportRecords_;
        char record[1024];
        auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(Clock::now() - started_).count();
        int count = snprintf(record, sizeof(record),
            "stage=%s kind=%s session=%s elapsed_ms=%lld child=%d group=%d sid=%d foreground=%d errno=%d wait_errno=%d signal_errno=%d wait_observed=%d wait_code=%d wait_status=%d exit_code=%d signal=%d reaped=%d raw_wait=%d alive=%d cleanup_paused=%d read_count=%lld read_errno=%d detail=%s\n",
            stage, kind_.c_str(), id_.c_str(), static_cast<long long>(elapsed), child_, group_, session_, foreground_, error,
            waitErrno_, signalErrno_, waitObserved_ ? 1 : 0, waitCode_, waitStatus_, exitCode_, signal_, reaped_ ? 1 : 0,
            rawStatus_, owned_.Alive(), cleanupPaused_ ? 1 : 0, static_cast<long long>(readCount_), readErrno_, detail.substr(0, 256).c_str());
        if (count > 0 && count < static_cast<int>(sizeof(record))) {
            ssize_t ignored = write(report_.get(), record, static_cast<size_t>(count)); (void)ignored;
        }
    }
    void LogTerminal(const char *stage, const struct termios &settings) {
        Log(stage, 0, "icanon=" + std::to_string((settings.c_lflag & ICANON) != 0) +
            " isig=" + std::to_string((settings.c_lflag & ISIG) != 0) + " echo=" + std::to_string((settings.c_lflag & ECHO) != 0) +
            " vmin=" + std::to_string(settings.c_cc[VMIN]) + " vtime=" + std::to_string(settings.c_cc[VTIME]) +
            " veof=" + std::to_string(settings.c_cc[VEOF]) + " iflag=" + std::to_string(settings.c_iflag) +
            " oflag=" + std::to_string(settings.c_oflag) + " cflag=" + std::to_string(settings.c_cflag) +
            " lflag=" + std::to_string(settings.c_lflag));
    }
    void LogCurrentTerminal(const char *stage) {
        struct termios settings = {};
        if (observer_.get() < 0) { Log(stage, EBADF); return; }
        if (tcgetattr(observer_.get(), &settings) != 0) { Log(stage, errno); return; }
        LogTerminal(stage, settings);
    }
    void LogConnection() {
        struct stat masterStat = {}, slaveStat = {};
        int masterResult = fstat(master_.get(), &masterStat), masterError = masterResult == 0 ? 0 : errno;
        int slaveResult = fstat(observer_.get(), &slaveStat), slaveError = slaveResult == 0 ? 0 : errno;
        int masterFlags = fcntl(master_.get(), F_GETFL), masterFlagError = masterFlags < 0 ? errno : 0;
        int slaveFlags = fcntl(observer_.get(), F_GETFL), slaveFlagError = slaveFlags < 0 ? errno : 0;
        Log("pty_connection", masterError ? masterError : slaveError,
            "stdio=spawn_open_slave_dup_0_to_1_2 master_isatty=" + std::to_string(isatty(master_.get())) +
            " slave_isatty=" + std::to_string(isatty(observer_.get())) +
            " master_rdev=" + std::to_string(static_cast<unsigned long long>(masterStat.st_rdev)) +
            " slave_rdev=" + std::to_string(static_cast<unsigned long long>(slaveStat.st_rdev)) +
            " master_flags=" + std::to_string(masterFlags) + " slave_flags=" + std::to_string(slaveFlags) +
            " master_flags_errno=" + std::to_string(masterFlagError) + " slave_flags_errno=" + std::to_string(slaveFlagError));
        for (int index = STDIN_FILENO; index <= STDERR_FILENO; ++index) {
            struct stat connected = {};
            std::string fd = "/proc/" + std::to_string(child_) + "/fd/" + std::to_string(index);
            int result = stat(fd.c_str(), &connected), metadataError = result == 0 ? 0 : errno;
            Log("pty_child_stdio", metadataError, "fd=" + std::to_string(index) +
                " is_character=" + std::to_string(result == 0 && S_ISCHR(connected.st_mode)) +
                " same_slave_rdev=" + std::to_string(result == 0 && slaveResult == 0 && connected.st_rdev == slaveStat.st_rdev));
        }
        errno = 0; foreground_ = tcgetpgrp(master_.get()); Log("pty_initial_foreground", foreground_ < 0 ? errno : 0);
        errno = 0; pid_t ttySession = tcgetsid(observer_.get());
        Log("pty_initial_session", ttySession < 0 ? errno : 0, "tty_sid=" + std::to_string(ttySession));
    }
    void SetError(const char *error, int number, const std::string &detail = "") {
        bool changed = error_ != error || errorNumber_ != number || errorDetail_ != detail;
        error_ = error; errorNumber_ = number; errorDetail_ = detail;
        if (changed) Log(error, number, detail);
    }
    std::string Result(bool ok, const char *error = "", int number = 0, const std::string &data = "", int accepted = 0) const {
        std::string state = "idle";
        if (!id_.empty()) {
            if (cleanupVerified_) state = "exited";
            else if (cleanupUnknown_ || !error_.empty()) state = "stop_failed";
            else state = stopping_ ? "stopping" : "running";
        }
        std::string failure = *error ? error : error_;
        return std::string("{\"ok\":") + (ok ? "true" : "false") + ",\"kind\":" + JsonString(kind_) +
            ",\"sessionId\":" + JsonString(id_) + ",\"status\":" + JsonString(state) +
            ",\"policy\":" + JsonString(policy_) +
            ",\"running\":" + (child_ > 0 && !leaderExited_ ? "true" : "false") +
            ",\"eof\":" + (eof_ ? "true" : "false") +
            ",\"cleanupVerified\":" + (cleanupVerified_ ? "true" : "false") +
            ",\"cleanupPaused\":" + (cleanupPaused_ ? "true" : "false") +
            ",\"procVisibilityUnknown\":" + (procVisibilityUnknown_ ? "true" : "false") +
            ",\"procScanErrno\":" + std::to_string(procScanErrno_) +
            ",\"waitObserved\":" + (waitObserved_ ? "true" : "false") + ",\"reaped\":" + (reaped_ ? "true" : "false") +
            ",\"exitCode\":" + (exitCode_ >= 0 ? std::to_string(exitCode_) : "null") +
            ",\"signal\":" + std::to_string(signal_) + ",\"waitErrno\":" + std::to_string(waitErrno_) +
            ",\"signalErrno\":" + std::to_string(signalErrno_) + ",\"survivors\":" + std::to_string(owned_.Alive()) +
            ",\"accepted\":" + std::to_string(accepted) + ",\"dataBase64\":" + JsonString(data) +
            ",\"error\":" + JsonString(failure) + ",\"errno\":" + std::to_string(*error ? number : errorNumber_) +
            ",\"detail\":" + JsonString(errorDetail_) + "}";
    }
    bool Observe() {
        std::string failure; bool complete = false;
        // Runtime health and final cleanup require different evidence. A
        // pinned live child remains usable through proc hardening; once stop
        // starts (including natural exit), only the strict scan is accepted.
        bool observed = !stopping_ && !leaderExited_ ? owned_.ObserveRunning(failure, complete) :
            (complete = owned_.Observe(failure));
        procVisibilityUnknown_ = !complete; procScanErrno_ = complete ? 0 : errno;
        if (!observed) { cleanupUnknown_ = true; SetError("process_scan", procScanErrno_, failure); return false; }
        if (complete) {
            if (lastAnchorDetail_ != failure || !lastVisibilityDetail_.empty()) {
                lastAnchorDetail_ = failure; Log("process_scan_anchor", 0, failure);
            }
            if (!lastVisibilityDetail_.empty()) { lastVisibilityDetail_.clear(); Log("process_scan_recovered", 0); }
        } else if (lastVisibilityDetail_ != failure) {
            lastVisibilityDetail_ = failure; Log("process_scan_unverified_running", procScanErrno_, failure);
        }
        if (error_ == "process_scan") {
            error_.clear(); errorDetail_.clear(); errorNumber_ = 0;
            Log(complete ? "process_scan_recovered" : "leader_running_confirmed", 0);
        }
        cleanupUnknown_ = false;
        // Keep incomplete-view polling at the slower cadence. This false
        // means scan incomplete, not failed terminal health; no input is lost.
        return complete;
    }
    bool SignalOwned(int number) {
        std::string failure;
        errno = 0; group_ = getpgid(child_); int groupError = group_ < 0 ? errno : 0;
        Log("getpgid", groupError);
        errno = 0; session_ = getsid(child_); int sessionError = session_ < 0 ? errno : 0;
        Log("getsid", sessionError);
        // Never fall back to a numeric PID/group after a failed query or wait.
        // The stable handles are the signal authority, including other job groups.
        bool observed = Observe();
        bool sent = true; signalErrno_ = 0;
        int knownAlive = owned_.Alive(); bool attempted = knownAlive > 0;
        if (attempted) sent = owned_.Signal(number, failure, signalErrno_);
        Log(number == SIGTERM ? "signal_term" : "signal_kill", signalErrno_,
            "attempted=" + std::to_string(attempted) + " known_alive=" + std::to_string(knownAlive));
        if (!sent) SetError("signal_failed", signalErrno_, failure);
        return observed && sent;
    }
    void RequestStop(const char *reason, bool retry = false) {
        if (child_ <= 0 || cleanupVerified_) return;
        input_.clear();
        if (stopping_ && !retry) return;
        bool wasStopping = stopping_;
        stopping_ = true; cleanupPaused_ = false; stopStarted_ = Clock::now(); nextObserve_ = stopStarted_;
        nextSignal_ = stopStarted_ + std::chrono::seconds(1);
        error_.clear(); errorDetail_.clear(); errorNumber_ = 0;
        if (wasStopping) Log("cleanup_explicit_retry", 0);
        Log(reason, 0); SignalOwned(SIGTERM);
    }
    void ReleasePty() {
        if (shellStartup_) {
            if (!shellStartup_->Remove()) Log("shell_startup_remove", errno);
            else shellStartup_.reset();
        }
        observer_.reset();
        char bytes[4096]; bool truncated = false;
        for (unsigned round = 0; master_.get() >= 0 && round < 64; ++round) {
            ssize_t count = read(master_.get(), bytes, sizeof(bytes));
            if (count > 0) {
                size_t remaining = 65536 - finalOutput_.size();
                size_t keep = std::min(remaining, static_cast<size_t>(count));
                finalOutput_.append(bytes, keep); if (keep < static_cast<size_t>(count)) truncated = true;
            } else if (count < 0 && errno == EINTR) continue;
            else break;
        }
        master_.reset(); eof_ = finalOutput_.empty();
        Log(truncated ? "pty_released_tail_truncated" : "pty_released", 0);
    }
    void Watch() {
        std::unique_lock<std::mutex> lock(mutex_);
        for (;;) {
            if (child_ > 0 && !cleanupPaused_) {
                siginfo_t info = {}; int waited = waitid(P_PID, static_cast<id_t>(child_), &info, WEXITED | WNOHANG | WNOWAIT);
                if (waited != 0 && errno != EINTR) {
                    waitErrno_ = errno; cleanupUnknown_ = true; cleanupPaused_ = true; SetError("waitid_ownership", waitErrno_);
                    std::string failure;
                    if (!owned_.Signal(SIGKILL, failure, signalErrno_)) SetError("signal_failed", signalErrno_, failure);
                    // Lost wait ownership is not exit confirmation. Do not ever
                    // act on this numeric PID again; restart remains blocked.
                    child_ = -1;
                } else if (waited == 0 && info.si_pid == child_) {
                    leaderExited_ = true;
                    if (!waitObserved_) {
                        waitObserved_ = true; waitCode_ = info.si_code; waitStatus_ = info.si_status;
                        exitCode_ = info.si_code == CLD_EXITED ? info.si_status : -1;
                        signal_ = info.si_code == CLD_KILLED || info.si_code == CLD_DUMPED ? info.si_status : 0;
                        Log("waitid_leader_exit", 0); LogCurrentTerminal("pty_leader_exit_attributes");
                    }
                    if (!stopping_) RequestStop("leader_exit_cleanup");
                }
                if (child_ > 0 && Clock::now() >= nextObserve_) {
                    bool observed = Observe();
                    nextObserve_ = Clock::now() + std::chrono::milliseconds(observed ? 100 : 500);
                }
                if (child_ > 0 && leaderExited_ && stopping_ && !cleanupUnknown_ && owned_.Alive() == 0) {
                    // Final fresh enumeration while the leader is still unreaped
                    // pins its session number and detects ordinary background jobs.
                    if (Observe() && owned_.Alive() == 0) {
                        int status = 0; pid_t reaped = waitpid(child_, &status, WNOHANG);
                        if (reaped == child_) {
                            reaped_ = true; rawStatus_ = status; exitCode_ = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
                            signal_ = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
                            Log("waitpid_reaped", 0); child_ = -1; ReleasePty(); input_.clear();
                            stopping_ = false; cleanupPaused_ = false; cleanupVerified_ = true;
                            procVisibilityUnknown_ = false; procScanErrno_ = 0;
                            error_.clear(); errorDetail_.clear(); errorNumber_ = 0;
                            owned_.Clear(); Log("cleanup_verified", 0);
                        } else if (reaped < 0 && errno != EINTR) {
                            waitErrno_ = errno; cleanupUnknown_ = true; cleanupPaused_ = true;
                            SetError("waitpid_failed", waitErrno_); child_ = -1;
                        }
                    }
                }
                if (child_ > 0 && stopping_) {
                    auto now = Clock::now();
                    if (now - stopStarted_ >= std::chrono::seconds(5)) {
                        // Alive()==0 only describes pinned handles. A failed
                        // enumeration still cannot prove all jobs are gone.
                        cleanupUnknown_ = true; cleanupPaused_ = true;
                        if (error_.empty()) SetError("stop_timeout", ETIMEDOUT);
                        Log("cleanup_paused", errorNumber_, errorDetail_);
                    } else if (now >= nextSignal_) {
                        SignalOwned(SIGKILL); nextSignal_ = Clock::now() + std::chrono::seconds(1);
                    }
                }
            }
            if (child_ > 0 && !stopping_ && !leaderExited_ && !input_.empty()) {
                ssize_t sent = write(master_.get(), input_.data(), input_.size());
                if (sent > 0) input_.erase(0, static_cast<size_t>(sent));
                else if (sent < 0 && errno != EAGAIN && errno != EINTR) {
                    SetError("queued_write", errno); RequestStop("input_failure");
                }
            }
            if (shutdown_ && (child_ <= 0 || Clock::now() >= shutdownDeadline_)) return;
            changed_.wait_for(lock, std::chrono::milliseconds(50));
        }
    }
    std::string kind_, id_, cwd_, policy_, error_, errorDetail_, lastAnchorDetail_, lastVisibilityDetail_, input_, finalOutput_;
    std::mutex mutex_; std::condition_variable changed_;
    Descriptor master_, observer_, report_;
    std::unique_ptr<HostLayout> layout_;
    std::unique_ptr<ShellStartup> shellStartup_;
    OwnedProcesses owned_;
    pid_t child_ = -1, group_ = -1, session_ = -1, foreground_ = -1;
    int exitCode_ = -1, signal_ = 0, rawStatus_ = -1, errorNumber_ = 0, waitErrno_ = 0, signalErrno_ = 0;
    int waitCode_ = 0, waitStatus_ = 0, readErrno_ = 0;
    int procScanErrno_ = 0;
    ssize_t readCount_ = 0; unsigned reportRecords_ = 0;
    bool eof_ = true, stopping_ = false, shutdown_ = false, leaderExited_ = false;
    bool cleanupVerified_ = false, cleanupUnknown_ = false, cleanupPaused_ = false;
    bool procVisibilityUnknown_ = false;
    bool waitObserved_ = false, reaped_ = false, firstRead_ = false, firstReadEof_ = false, firstReadEio_ = false, firstOutput_ = false;
    Clock::time_point started_, stopStarted_, nextSignal_, nextObserve_, shutdownDeadline_;
    std::thread watcher_;
};
Session *Current(const std::string &kind) {
    static Session shell("shell"), codex("codex");
    if (kind == "shell") return &shell;
    if (kind == "codex") return &codex;
    return nullptr;
}
std::string InvalidKind() { return "{\"ok\":false,\"error\":\"invalid_session_kind\"}"; }
}
std::string PtyStart(const std::string &files, const std::string &kind, const std::string &cwd,
                     const std::string &policy, int columns, int rows) {
    auto session = Current(kind); return session ? session->Start(files, cwd, policy, columns, rows) : InvalidKind();
}
std::string PtyWrite(const std::string &kind, const std::string &id, const std::string &bytes) {
    auto session = Current(kind); return session ? session->Write(id, bytes) : InvalidKind();
}
std::string PtyResize(const std::string &kind, const std::string &id, int columns, int rows) {
    auto session = Current(kind); return session ? session->Resize(id, columns, rows) : InvalidKind();
}
std::string PtyRead(const std::string &kind, const std::string &id) {
    auto session = Current(kind); return session ? session->Read(id) : InvalidKind();
}
std::string PtyControl(const std::string &kind, const std::string &id, const std::string &operation) {
    auto session = Current(kind); return session ? session->Control(id, operation) : InvalidKind();
}
std::string PtyStatus(const std::string &kind, const std::string &id) {
    auto session = Current(kind); return session ? session->Status(id) : InvalidKind();
}
}
