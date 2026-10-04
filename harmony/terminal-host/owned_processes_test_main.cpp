// Compiles the real OwnedProcesses implementation against deterministic POSIX
// boundaries. No process is spawned/signalled and no device is accessed.
// Link only this file and entry/src/main/cpp/process_identity.cpp.
#include "owned_processes.h"
#include <algorithm>
#include <cerrno>
#include <cstdarg>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <dirent.h>
#include <fcntl.h>
#include <functional>
#include <limits>
#include <poll.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

#ifndef O_PATH
#define O_PATH 0
#endif
#ifndef SYS_pidfd_open
#define SYS_pidfd_open 0x10001
#endif
#ifndef SYS_pidfd_send_signal
#define SYS_pidfd_send_signal 0x10002
#endif

namespace {
constexpr pid_t Leader = 100, Child = 101, Foreign = 200;
constexpr uid_t HostUid = 1001;
struct Process {
    pid_t pid, parent, group, session;
    unsigned long long start;
    uid_t uid = HostUid;
    char state = 'S';
    bool exited = false, waitOwned = false, enumerated = true;
    int statError = 0, statusError = 0;
};
enum class FdKind { Proc, Directory, Status, Stat, Pidfd };
struct File {
    FdKind kind; pid_t pid = -1; unsigned long long start = 0; std::string contents;
};
struct Enumeration { int fd; std::vector<pid_t> entries; size_t next = 0; dirent entry = {}; };
struct Sent { pid_t pid; unsigned long long start; int number; };
std::map<pid_t, Process> processes;
std::map<int, File> files;
std::vector<Sent> signals;
int nextFd = 10, procError = 0, enumerationError = 0, pollError = 0;
short pollEvents = 0;
unsigned openedPidfds = 0, statReads = 0, hideLeaderAtStatRead = 0, changeLeaderAtStatRead = 0;
bool reuseDuringWait = false, waitOptionsValid = true;
int AddFile(File file) { int fd = nextFd++; files.emplace(fd, std::move(file)); return fd; }
int TestClose(int fd) { files.erase(fd); return 0; }
int TestOpen(const char *path, int, ...) {
    if (procError) { errno = procError; return -1; }
    if (std::string(path) != "/proc") { errno = ENOENT; return -1; }
    return AddFile({FdKind::Proc, -1, 0, {}});
}
int TestOpenAt(int fd, const char *path, int, ...) {
    auto file = files.find(fd);
    if (file == files.end()) { errno = EBADF; return -1; }
    if (file->second.kind == FdKind::Proc) {
        pid_t pid = static_cast<pid_t>(strtol(path, nullptr, 10));
        if (!processes.count(pid)) { errno = ENOENT; return -1; }
        return AddFile({FdKind::Directory, pid, 0, {}});
    }
    if (file->second.kind != FdKind::Directory) { errno = ENOTDIR; return -1; }
    auto found = processes.find(file->second.pid);
    if (found == processes.end()) { errno = ENOENT; return -1; }
    Process &process = found->second;
    if (std::string(path) == "status") {
        if (process.statusError) { errno = process.statusError; return -1; }
        std::string uid = std::to_string(process.uid);
        return AddFile({FdKind::Status, process.pid, 0, "Name:\tfixture\nUid:\t" + uid + "\t" + uid + "\t" + uid + "\t" + uid + "\n"});
    }
    if (std::string(path) != "stat") { errno = ENOENT; return -1; }
    if (process.pid == Leader) {
        ++statReads;
        if (hideLeaderAtStatRead == statReads) process.statError = ENOENT;
        if (changeLeaderAtStatRead == statReads) ++process.start;
    }
    if (process.statError) { errno = process.statError; return -1; }
    std::string record = std::to_string(process.pid) + " (fixture) " + process.state + " " +
        std::to_string(process.parent) + " " + std::to_string(process.group) + " " + std::to_string(process.session) +
        " 0 -1 4194304 1 2 3 4 5 6 7 8 9 10 1 0 " + std::to_string(process.start) + " 4096 0\n";
    return AddFile({FdKind::Stat, process.pid, 0, record});
}
ssize_t TestRead(int fd, void *buffer, size_t size) {
    auto file = files.find(fd);
    if (file == files.end()) { errno = EBADF; return -1; }
    size_t count = std::min(size, file->second.contents.size());
    memcpy(buffer, file->second.contents.data(), count);
    file->second.contents.erase(0, count); return static_cast<ssize_t>(count);
}
int TestDup(int fd) {
    auto file = files.find(fd);
    if (file == files.end()) { errno = EBADF; return -1; }
    return AddFile(file->second);
}
DIR *TestFdopendir(int fd) {
    auto enumeration = new Enumeration{fd, {}, 0, {}};
    for (const auto &item : processes) if (item.second.enumerated) enumeration->entries.push_back(item.first);
    return reinterpret_cast<DIR *>(enumeration);
}
dirent *TestReaddir(DIR *opaque) {
    if (enumerationError) { errno = enumerationError; return nullptr; }
    auto enumeration = reinterpret_cast<Enumeration *>(opaque);
    if (enumeration->next == enumeration->entries.size()) { errno = 0; return nullptr; }
    snprintf(enumeration->entry.d_name, sizeof(enumeration->entry.d_name), "%d", enumeration->entries[enumeration->next++]);
    return &enumeration->entry;
}
int TestClosedir(DIR *opaque) {
    auto enumeration = reinterpret_cast<Enumeration *>(opaque);
    TestClose(enumeration->fd); delete enumeration; return 0;
}
int TestPoll(pollfd *events, nfds_t count, int timeout) {
    if (pollError) { errno = pollError; return -1; }
    if (count != 1 || timeout != 0 || events[0].events != POLLIN) { errno = EINVAL; return -1; }
    auto file = files.find(events[0].fd);
    if (file == files.end() || file->second.kind != FdKind::Pidfd) { events[0].revents = POLLNVAL; return 1; }
    if (pollEvents) { events[0].revents = pollEvents; return 1; }
    auto process = processes.find(file->second.pid);
    bool gone = process == processes.end() || process->second.start != file->second.start || process->second.exited;
    events[0].revents = gone ? POLLIN : 0; return gone ? 1 : 0;
}
int TestWaitid(idtype_t type, id_t id, siginfo_t *information, int options) {
    if (type != P_PID || options != (WEXITED | WNOHANG | WNOWAIT)) { waitOptionsValid = false; errno = EINVAL; return -1; }
    auto process = processes.find(static_cast<pid_t>(id));
    if (process == processes.end() || !process->second.waitOwned) { errno = ECHILD; return -1; }
    memset(information, 0, sizeof(*information));
    if (process->second.exited) { information->si_pid = process->first; information->si_code = CLD_EXITED; information->si_status = 7; }
    if (reuseDuringWait) { ++process->second.start; reuseDuringWait = false; }
    return 0;
}
long TestSyscall(long number, ...) {
    va_list args; va_start(args, number);
    if (number == SYS_pidfd_open) {
        pid_t pid = va_arg(args, int); va_end(args); ++openedPidfds;
        auto process = processes.find(pid);
        if (process == processes.end()) { errno = ESRCH; return -1; }
        return AddFile({FdKind::Pidfd, pid, process->second.start, {}});
    }
    if (number == SYS_pidfd_send_signal) {
        int fd = va_arg(args, int), signal = va_arg(args, int); va_end(args);
        auto file = files.find(fd);
        if (file == files.end() || file->second.kind != FdKind::Pidfd) { errno = EBADF; return -1; }
        auto process = processes.find(file->second.pid);
        if (process == processes.end() || process->second.start != file->second.start || process->second.exited) { errno = ESRCH; return -1; }
        signals.push_back({file->second.pid, file->second.start, signal}); return 0;
    }
    va_end(args); errno = ENOSYS; return -1;
}
uid_t TestUid() { return HostUid; }
void Reset() {
    processes.clear(); files.clear(); signals.clear();
    processes.emplace(Leader, Process{Leader, 900, Leader, Leader, 42, HostUid, 'S', false, true});
    procError = enumerationError = pollError = 0; pollEvents = 0;
    openedPidfds = statReads = hideLeaderAtStatRead = changeLeaderAtStatRead = 0;
    reuseDuringWait = false; waitOptionsValid = true;
}
void Require(bool value, const char *message) {
    if (!value) { fprintf(stderr, "FAIL %s errno=%d\n", message, errno); exit(1); }
}
unsigned checks = 0;
void Run(const char *name, const std::function<void(codex_hnp::OwnedProcesses &, std::string &)> &body) {
    Reset(); codex_hnp::OwnedProcesses owned; std::string error;
    Require(owned.Begin(Leader, error), "fixture leader pinned before hardening");
    body(owned, error); Require(waitOptionsValid, "wait never consumes exit or stopped events");
    owned.Clear(); Require(files.empty(), "fixture descriptors released");
    ++checks; printf("PASS %s\n", name);
}
}

namespace codex_hnp {
Descriptor::~Descriptor() { reset(); }
void Descriptor::reset(int fd) { if (fd_ >= 0) TestClose(fd_); fd_ = fd; }
Descriptor &Descriptor::operator=(Descriptor &&other) noexcept { if (this != &other) reset(other.release()); return *this; }
}

// Interpose only system boundaries inside the production translation unit.
#define open TestOpen
#define openat TestOpenAt
#define read TestRead
#define dup TestDup
#define fdopendir TestFdopendir
#define readdir TestReaddir
#define closedir TestClosedir
#define poll TestPoll
#define waitid TestWaitid
#define syscall TestSyscall
#define getuid TestUid
#define geteuid TestUid
#include "entry/src/main/cpp/owned_processes.cpp"
#undef open
#undef openat
#undef read
#undef dup
#undef fdopendir
#undef readdir
#undef closedir
#undef poll
#undef waitid
#undef syscall
#undef getuid
#undef geteuid

int main() {
    Run("visible live leader has complete scan", [](auto &owned, auto &error) {
        bool complete = false; Require(owned.ObserveRunning(error, complete) && complete, "complete live observation");
    });
    for (int hiddenError : {ENOENT, EACCES, EPERM}) {
        Run("hidden live leader uses original pidfd and WNOWAIT", [hiddenError](auto &owned, auto &error) {
            processes.at(Leader).statError = hiddenError; unsigned pins = openedPidfds; bool complete = true;
            Require(owned.ObserveRunning(error, complete) && !complete && errno == hiddenError, "hidden live child retained");
            Require(openedPidfds == pins && owned.Alive() == 1 && error.find("pidfd_alive=1 wait_owned=1") != std::string::npos, "no numeric PID repin");
            Require(!owned.Observe(error), "same partial evidence cannot authorize cleanup");
        });
    }
    Run("stopped and continued hidden leader stays live", [](auto &owned, auto &error) {
        processes.at(Leader).statError = ENOENT; bool complete = true;
        processes.at(Leader).state = 'T'; Require(owned.ObserveRunning(error, complete) && !complete, "stopped child is live");
        processes.at(Leader).state = 'S'; Require(owned.ObserveRunning(error, complete) && !complete, "continued child is live");
    });
    Run("filtered readdir is diagnostic while leader is live", [](auto &owned, auto &error) {
        processes.at(Leader).enumerated = false; bool complete = true;
        Require(owned.ObserveRunning(error, complete) && !complete, "direct live anchor survives hidden enumeration");
        Require(!owned.Observe(error), "filtered enumeration is not cleanup proof");
    });
    Run("proc open and enumeration failures preserve verified live leader", [](auto &owned, auto &error) {
        bool complete = true; procError = EACCES;
        Require(owned.ObserveRunning(error, complete) && !complete, "unreadable proc root is diagnostic");
        procError = 0; enumerationError = EIO;
        Require(owned.ObserveRunning(error, complete) && !complete, "failed readdir is diagnostic");
    });
    Run("lost wait ownership cannot be downgraded", [](auto &owned, auto &error) {
        processes.at(Leader).statError = ENOENT; processes.at(Leader).waitOwned = false; bool complete = true;
        Require(!owned.ObserveRunning(error, complete) && !complete && errno == ECHILD, "pidfd alone does not prove child ownership");
    });
    Run("poll errors and invalid handles do not prove liveness or death", [](auto &owned, auto &error) {
        processes.at(Leader).statError = ENOENT; bool complete = true; pollError = EIO;
        Require(!owned.ObserveRunning(error, complete) && owned.Alive() == 1, "poll error remains unknown");
        pollError = 0; pollEvents = POLLNVAL;
        Require(!owned.ObserveRunning(error, complete) && owned.Alive() == 1, "invalid handle remains unknown");
        pollEvents = POLLIN | POLLERR;
        Require(!owned.ObserveRunning(error, complete) && owned.Alive() == 1, "error event cannot prove cleanup");
    });
    Run("exit during hidden view never certifies cleanup", [](auto &owned, auto &error) {
        processes.at(Leader).statError = ENOENT; processes.at(Leader).exited = true; bool complete = true;
        Require(!owned.ObserveRunning(error, complete) && !complete && owned.Alive() == 0, "exit cannot be running fallback");
        Require(!owned.Observe(error), "zero pinned survivors does not prove descendants gone");
    });
    Run("PID reuse between wait and second poll is rejected", [](auto &owned, auto &error) {
        processes.at(Leader).statError = ENOENT; reuseDuringWait = true; bool complete = true;
        Require(!owned.ObserveRunning(error, complete) && !complete, "old stable handle rejects new numeric child");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.empty(), "recycled PID never signalled");
    });
    Run("changed visible identity is not a visibility warning", [](auto &owned, auto &error) {
        ++processes.at(Leader).start; bool complete = true;
        Require(!owned.ObserveRunning(error, complete) && errno == ESTALE, "changed start remains identity failure");
    });
    Run("hidden live descendant is not treated as ENOENT death", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43}); processes.at(Child).statError = ENOENT;
        bool complete = true; Require(owned.ObserveRunning(error, complete) && !complete, "unknown live descendant makes view incomplete");
        Require(!owned.Observe(error), "hidden descendant blocks cleanup");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError), "known handles remain signalable");
        Require(signals.size() == 1 && signals[0].pid == Leader, "probe handle is never an owned signal target");
    });
    Run("vanished enumerated process is safely skipped", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43}); processes.at(Child).statError = ENOENT; processes.at(Child).exited = true;
        bool complete = false; Require(owned.ObserveRunning(error, complete) && complete, "pidfd confirms actual disappearance");
    });
    Run("foreign unreadable process is excluded by actual UID", [](auto &owned, auto &error) {
        processes.emplace(Foreign, Process{Foreign, 900, Foreign, Foreign, 60, HostUid + 1}); processes.at(Foreign).statError = ENOENT;
        Require(owned.Observe(error), "proven foreign process does not block scan");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.size() == 1 && signals[0].pid == Leader, "foreign process never signalled");
    });
    Run("already pinned hidden descendant remains a stable signal target", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43});
        Require(owned.Observe(error) && owned.Alive() == 2, "descendant pinned while visible");
        processes.at(Leader).statError = ENOENT; processes.at(Child).statError = ENOENT; bool complete = true;
        Require(owned.ObserveRunning(error, complete) && !complete, "both hidden processes retain running anchor");
        int signalError = 0; Require(owned.Signal(SIGKILL, error, signalError) && signals.size() == 2, "signals use previously verified pins");
        Require(signals[0].pid == Leader && signals[1].pid == Child && signals[1].start == 43, "no replacement handle is used");
    });
    Run("old exited pin cannot hide a live recycled descendant PID", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43}); Require(owned.Observe(error), "old descendant pinned");
        ++processes.at(Child).start; processes.at(Child).statError = ENOENT;
        bool complete = true; Require(owned.ObserveRunning(error, complete) && !complete, "current live occupant makes scan incomplete");
        Require(!owned.Observe(error), "old handle death is not current occupant death");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.size() == 1 && signals[0].pid == Leader, "recycled descendant PID is never signalled");
    });
    Run("restored exit scan discovers survivor before cleanup", [](auto &owned, auto &error) {
        processes.at(Leader).exited = true; processes.at(Leader).state = 'Z';
        processes.emplace(Child, Process{Child, 1, Leader, Leader, 43});
        Require(owned.Observe(error) && owned.Alive() == 1, "post-exit enumeration pins reparented same-session survivor");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.size() == 1 && signals[0].pid == Child, "only survivor is signalled");
        processes.at(Child).exited = true;
        Require(owned.Observe(error) && owned.Alive() == 0, "fresh final scan and stable handles both confirm gone");
    });
    Run("filtered exit scan remains incomplete even with zero pinned survivors", [](auto &owned, auto &error) {
        processes.at(Leader).exited = true; processes.at(Leader).state = 'Z'; processes.at(Leader).enumerated = false;
        Require(!owned.Observe(error) && owned.Alive() == 0, "zombie direct anchor cannot replace enumeration");
    });
    Run("leader metadata lost after new pins rolls them back", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43});
        statReads = 0; hideLeaderAtStatRead = 3; bool complete = true;
        Require(owned.ObserveRunning(error, complete) && !complete && owned.Alive() == 1, "partial scan does not retain provisional descendant pin");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.size() == 1 && signals[0].pid == Leader, "rollback preserves original signal authority");
    });
    Run("anchor changes after new pins remains fatal and rolls back", [](auto &owned, auto &error) {
        processes.emplace(Child, Process{Child, Leader, Leader, Leader, 43});
        statReads = 0; changeLeaderAtStatRead = 3; bool complete = true;
        Require(!owned.ObserveRunning(error, complete) && errno == ESTALE, "changed final anchor cannot be downgraded");
        int signalError = 0; Require(owned.Signal(SIGTERM, error, signalError) && signals.empty(), "unverified descendant and recycled leader never signalled");
    });
    Reset(); {
        codex_hnp::OwnedProcesses owned; std::string error; hideLeaderAtStatRead = 2;
        Require(!owned.Begin(Leader, error) && owned.Alive() == 0, "startup cannot succeed without pinned leader");
        Require(error.find("stat_open") != std::string::npos, "startup preserves missing metadata cause");
    }
    Require(files.empty(), "failed-start descriptors released"); ++checks; printf("PASS startup pin race is rejected\n");
    printf("%u checks passed\n", checks); return 0;
}
