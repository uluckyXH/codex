#include "pty_session.h"
#include "host_layout.h"
#include "native_package.h"
#include "owned_processes.h"
#include <algorithm>
#include <cerrno>
#include <chrono>
#include <condition_variable>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <mutex>
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

class Session {
public:
    explicit Session(std::string kind) : kind_(std::move(kind)), watcher_([this] { Watch(); }) {}
    ~Session() {
        {
            std::lock_guard<std::mutex> lock(mutex_);
            shutdown_ = true; shutdownDeadline_ = Clock::now() + std::chrono::seconds(6);
            RequestStop("host_shutdown");
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
        std::string package = CODEX_HNP_PACKAGE_PATH;
        // Fixed bootstrap, structured argv. Quotes/spaces/Chinese in cwd are
        // ordinary argument bytes and can never become executable shell text.
        std::vector<std::string> args = {"/system/bin/sh", "-c",
            "umask 077; cd \"$1\" || exit 125; shift; exec \"$@\"", "codex-terminal", working};
        if (kind_ == "shell") args.insert(args.end(), {"/system/bin/sh", "-i"});
        else args.insert(args.end(), {package + "/bin/codex", "--model", "gpt-5.6-terra", "--sandbox", policy,
                                      "--ask-for-approval", "on-request", "--cd", working});
        auto values = HostEnvironment(layout->Root(), "xterm-256color");
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
        if (!result && !layout->Revalidate(failure)) result = errno;
        pid_t child = -1;
        if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
        posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attributes);
        if (result) return Result(false, "posix_spawn", result);
        master_.reset(masterGuard.release()); observer_.reset(slaveGuard.release());
        report_ = std::move(nextReport);
        layout_ = std::move(layout); child_ = child; id_ = nextId; cwd_ = working; policy_ = policy;
        eof_ = false; exitCode_ = -1; signal_ = 0; rawStatus_ = 0; waitErrno_ = 0; signalErrno_ = 0;
        error_.clear(); errorNumber_ = 0; input_.clear(); finalOutput_.clear(); stopping_ = false; leaderExited_ = false;
        cleanupVerified_ = false; cleanupUnknown_ = false; nextObserve_ = Clock::now();
        group_ = child_; session_ = child_; foreground_ = -1;
        Log("spawn", 0);
        bool pinned = owned_.Begin(child_, failure);
        if (!pinned) {
            SetError("process_identity", errno); cleanupUnknown_ = true;
            // Retain child/session ownership on failure; never report it stopped.
            SignalDirectChild(child_, SIGTERM, failure); RequestStop("start_failed");
        }
        changed_.notify_one(); return Result(pinned, pinned ? "" : "process_identity", pinned ? 0 : errorNumber_);
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
        if (count > 0) return Result(true, "", 0, Base64(bytes, static_cast<size_t>(count)));
        if (count == 0 || (count < 0 && errno == EIO) || (count < 0 && errno == EAGAIN && cleanupVerified_)) {
            eof_ = true; master_.reset(); return Result(true);
        }
        if (count < 0 && (errno == EAGAIN || errno == EINTR)) return Result(true);
        return Result(false, "pty_read", errno);
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
        RequestStop(operation.c_str()); changed_.notify_one();
        return Result(error_.empty());
    }
    std::string Status(const std::string &id) {
        std::lock_guard<std::mutex> lock(mutex_);
        if (!Matches(id)) return Result(false, "stale_session");
        return Result(true);
    }
private:
    bool Matches(const std::string &id) const { return id == id_ && (!id.empty() || child_ <= 0); }
    void Log(const char *stage, int error) {
        if (report_.get() < 0) return;
        char record[512];
        int count = snprintf(record, sizeof(record),
            "stage=%s kind=%s session=%s child=%d group=%d sid=%d foreground=%d errno=%d wait_errno=%d signal_errno=%d raw_wait=%d alive=%d\n",
            stage, kind_.c_str(), id_.c_str(), child_, group_, session_, foreground_, error,
            waitErrno_, signalErrno_, rawStatus_, owned_.Alive());
        if (count > 0 && count < static_cast<int>(sizeof(record))) {
            ssize_t ignored = write(report_.get(), record, static_cast<size_t>(count)); (void)ignored;
        }
    }
    void SetError(const char *error, int number) { error_ = error; errorNumber_ = number; Log(error, number); }
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
            ",\"exitCode\":" + (exitCode_ >= 0 ? std::to_string(exitCode_) : "null") +
            ",\"signal\":" + std::to_string(signal_) + ",\"waitErrno\":" + std::to_string(waitErrno_) +
            ",\"signalErrno\":" + std::to_string(signalErrno_) + ",\"survivors\":" + std::to_string(owned_.Alive()) +
            ",\"accepted\":" + std::to_string(accepted) + ",\"dataBase64\":" + JsonString(data) +
            ",\"error\":" + JsonString(failure) + ",\"errno\":" + std::to_string(*error ? number : errorNumber_) + "}";
    }
    bool Observe() {
        std::string failure;
        if (!owned_.Observe(failure)) { cleanupUnknown_ = true; SetError("process_scan", errno); return false; }
        cleanupUnknown_ = false; return true;
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
        bool sent = owned_.Signal(number, failure, signalErrno_);
        Log(number == SIGTERM ? "signal_term" : "signal_kill", signalErrno_);
        if (!sent) SetError("signal_failed", signalErrno_);
        return observed && sent;
    }
    void RequestStop(const char *reason) {
        if (child_ <= 0 || cleanupVerified_) return;
        input_.clear();
        if (!stopping_) {
            stopping_ = true; stopStarted_ = Clock::now(); nextSignal_ = stopStarted_ + std::chrono::seconds(1);
            SignalOwned(SIGTERM); Log(reason, signalErrno_);
        } else if (!error_.empty()) {
            error_.clear(); errorNumber_ = 0; nextSignal_ = Clock::now();
        }
    }
    void ReleasePty() {
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
            if (child_ > 0) {
                siginfo_t info = {}; int waited = waitid(P_PID, static_cast<id_t>(child_), &info, WEXITED | WNOHANG | WNOWAIT);
                if (waited != 0 && errno != EINTR) {
                    waitErrno_ = errno; cleanupUnknown_ = true; SetError("waitid_ownership", waitErrno_);
                    std::string failure; owned_.Signal(SIGKILL, failure, signalErrno_);
                    // Lost wait ownership is not exit confirmation. Do not ever
                    // act on this numeric PID again; restart remains blocked.
                    child_ = -1;
                } else if (waited == 0 && info.si_pid == child_) {
                    leaderExited_ = true;
                    if (!stopping_) RequestStop("leader_exit_cleanup");
                }
                if (child_ > 0 && Clock::now() >= nextObserve_) {
                    Observe(); nextObserve_ = Clock::now() + std::chrono::milliseconds(100);
                }
                if (child_ > 0 && stopping_ && Clock::now() >= nextSignal_) {
                    SignalOwned(SIGKILL); nextSignal_ = Clock::now() + std::chrono::milliseconds(250);
                    if (Clock::now() - stopStarted_ > std::chrono::seconds(5) && owned_.Alive() > 0)
                        SetError("stop_timeout", ETIMEDOUT);
                }
                if (child_ > 0 && leaderExited_ && stopping_ && !cleanupUnknown_ && owned_.Alive() == 0) {
                    // Final fresh enumeration while the leader is still unreaped
                    // pins its session number and detects ordinary background jobs.
                    if (Observe() && owned_.Alive() == 0) {
                        int status = 0; pid_t reaped = waitpid(child_, &status, WNOHANG);
                        if (reaped == child_) {
                            rawStatus_ = status; exitCode_ = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
                            signal_ = WIFSIGNALED(status) ? WTERMSIG(status) : 0;
                            Log("waitpid_reaped", 0); child_ = -1; ReleasePty(); input_.clear();
                            stopping_ = false; cleanupVerified_ = true; error_.clear(); errorNumber_ = 0;
                            owned_.Clear(); Log("cleanup_verified", 0);
                        } else if (reaped < 0 && errno != EINTR) {
                            waitErrno_ = errno; cleanupUnknown_ = true; SetError("waitpid_failed", waitErrno_); child_ = -1;
                        }
                    }
                }
            }
            if (child_ > 0 && !stopping_ && !input_.empty()) {
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
    std::string kind_, id_, cwd_, policy_, error_, input_, finalOutput_;
    std::mutex mutex_; std::condition_variable changed_;
    Descriptor master_, observer_, report_;
    std::unique_ptr<HostLayout> layout_;
    OwnedProcesses owned_;
    pid_t child_ = -1, group_ = -1, session_ = -1, foreground_ = -1;
    int exitCode_ = -1, signal_ = 0, rawStatus_ = 0, errorNumber_ = 0, waitErrno_ = 0, signalErrno_ = 0;
    bool eof_ = true, stopping_ = false, shutdown_ = false, leaderExited_ = false;
    bool cleanupVerified_ = false, cleanupUnknown_ = false;
    Clock::time_point stopStarted_, nextSignal_, nextObserve_, shutdownDeadline_;
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
