#include "owned_processes.h"
#include <cerrno>
#include <cstdlib>
#include <dirent.h>
#include <fcntl.h>
#include <limits>
#include <poll.h>
#include <signal.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

namespace codex_hnp {
namespace {
int PidHandle(pid_t pid) {
#if defined(SYS_pidfd_open)
    return static_cast<int>(syscall(SYS_pidfd_open, pid, 0));
#else
    (void)pid; errno = ENOSYS; return -1;
#endif
}
int HandleSignal(int fd, int number) {
#if defined(SYS_pidfd_send_signal)
    return static_cast<int>(syscall(SYS_pidfd_send_signal, fd, number, nullptr, 0));
#else
    (void)fd; (void)number; errno = ENOSYS; return -1;
#endif
}
bool Failure(std::string &error, const char *stage) {
    error = std::string(stage) + " errno=" + std::to_string(errno); return false;
}
bool ReadStatus(int directory, ProcessIdentity &identity) {
    Descriptor input(openat(directory, "status", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
    if (input.get() < 0) return false;
    char contents[4096]; ssize_t count;
    do { count = read(input.get(), contents, sizeof(contents)); } while (count < 0 && errno == EINTR);
    if (count <= 0 || !ParseProcessUids(std::string(contents, count), identity)) { errno = EIO; return false; }
    return true;
}
bool ForeignIdentity(const ProcessIdentity &identity) {
    // Signal permission depends on real/saved/effective process UIDs, never
    // proc inode ownership (which can be root when dumpable is disabled).
    uid_t effective = geteuid(), real = getuid();
    return identity.realUid != effective && identity.effectiveUid != effective && identity.savedUid != effective &&
           identity.realUid != real && identity.effectiveUid != real && identity.savedUid != real;
}
bool ReadIdentity(int proc, pid_t pid, ProcessIdentity &identity, bool &provenForeign) {
    provenForeign = false;
    Descriptor directory(openat(proc, std::to_string(pid).c_str(), O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (directory.get() < 0) return false;
    ProcessIdentity status;
    bool statusRead = ReadStatus(directory.get(), status);
    if (statusRead) provenForeign = ForeignIdentity(status);
    Descriptor input(openat(directory.get(), "stat", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
    if (input.get() < 0) return false;
    char contents[4097]; ssize_t count;
    do { count = read(input.get(), contents, sizeof(contents)); } while (count < 0 && errno == EINTR);
    if (count <= 0 || count > 4096 || !ParseProcessStat(std::string(contents, count), identity) || identity.pid != pid) {
        errno = EIO; return false;
    }
    if (statusRead) {
        identity.realUid = status.realUid; identity.effectiveUid = status.effectiveUid;
        identity.savedUid = status.savedUid; identity.filesystemUid = status.filesystemUid;
    } else {
        // Membership can be inspected, but pinning must establish actual UID.
        errno = EACCES; return false;
    }
    return true;
}
bool SameProcess(const ProcessIdentity &a, const ProcessIdentity &b) { return a.pid == b.pid && a.start == b.start; }
}

bool StableProcessHandlesAvailable(std::string &error) {
    Descriptor handle(PidHandle(getpid()));
    if (handle.get() < 0 || HandleSignal(handle.get(), 0) != 0) return Failure(error, "pidfd_unavailable");
    return true;
}
bool SignalDirectChild(pid_t child, int number, std::string &error) {
    Descriptor handle(PidHandle(child));
    if (handle.get() < 0) return Failure(error, "direct_child_pidfd");
    siginfo_t information = {};
    // A reused non-child PID is rejected after pinning; an intervening reap
    // cannot redirect a signal sent through the already opened handle.
    if (waitid(P_PID, static_cast<id_t>(child), &information, WEXITED | WNOHANG | WNOWAIT) != 0)
        return Failure(error, "direct_child_ownership");
    if (HandleSignal(handle.get(), number) != 0 && errno != ESRCH) return Failure(error, "direct_child_signal");
    return true;
}
bool OwnedProcesses::Pin(const ProcessIdentity &identity, std::string &error) {
    auto previous = tracked_.find(identity.pid);
    if (previous != tracked_.end()) {
        if (SameProcess(previous->second.identity, identity)) return true;
        // Old handles cannot refer to the new occupant. Keep the old process
        // accounted for unless its stable handle confirms it has exited.
        struct pollfd event = {previous->second.handle.get(), POLLIN, 0};
        if (poll(&event, 1, 0) != 1 || !(event.revents & POLLIN)) { errno = ESTALE; return Failure(error, "pid_replaced_while_alive"); }
        tracked_.erase(previous);
    }
    if (tracked_.size() >= 512) { errno = EOVERFLOW; return Failure(error, "process_limit"); }
    Descriptor handle(PidHandle(identity.pid));
    if (handle.get() < 0) {
        if (errno == ESRCH || errno == ENOENT) return true;
        return Failure(error, "pidfd_open");
    }
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    ProcessIdentity confirmed; bool foreign = false;
    if (proc.get() < 0 || !ReadIdentity(proc.get(), identity.pid, confirmed, foreign)) {
        if (errno == ESRCH || errno == ENOENT) return true;
        return Failure(error, "pidfd_identity");
    }
    if (!SameProcess(identity, confirmed) || identity.session != confirmed.session || identity.parent != confirmed.parent ||
        identity.realUid != confirmed.realUid || identity.effectiveUid != confirmed.effectiveUid || identity.savedUid != confirmed.savedUid) {
        errno = ESTALE; return Failure(error, "pidfd_identity_changed");
    }
    tracked_.emplace(identity.pid, Tracked{confirmed, std::move(handle)}); return true;
}
bool OwnedProcesses::Begin(pid_t leader, std::string &error) {
    Clear(); leader_ = leader;
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    ProcessIdentity identity; bool foreign = false;
    if (proc.get() < 0 || !ReadIdentity(proc.get(), leader, identity, foreign)) return Failure(error, "leader_identity");
    if (identity.realUid != getuid() || identity.effectiveUid != geteuid()) { errno = EACCES; return Failure(error, "leader_uid"); }
    if (identity.session != leader || identity.group != leader) { errno = EACCES; return Failure(error, "leader_session"); }
    return Pin(identity, error);
}
bool OwnedProcesses::Observe(std::string &error) {
    if (leader_ <= 0) { errno = EINVAL; return Failure(error, "no_owned_session"); }
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (proc.get() < 0) return Failure(error, "proc_open");
    DIR *directory = fdopendir(dup(proc.get()));
    if (!directory) return Failure(error, "proc_enumerate");
    std::vector<ProcessIdentity> records;
    size_t entries = 0; int saved = 0;
    for (;;) {
        errno = 0; dirent *entry = readdir(directory);
        if (!entry) { saved = errno; break; }
        if (++entries > 16384) { saved = EOVERFLOW; break; }
        std::string name(entry->d_name);
        if (name.empty() || name.find_first_not_of("0123456789") != std::string::npos) continue;
        unsigned long value = strtoul(name.c_str(), nullptr, 10);
        if (!value || value > static_cast<unsigned long>(std::numeric_limits<pid_t>::max())) continue;
        ProcessIdentity identity; bool foreign = false; pid_t pid = static_cast<pid_t>(value);
        if (ReadIdentity(proc.get(), pid, identity, foreign)) records.push_back(identity);
        else if (errno != ENOENT && errno != ESRCH) {
            // Unknown unreadable entries are not silently considered dead.
            // A readable status proving a foreign identity may be excluded,
            // except when that exact PID is already tracked by a stable handle.
            if (!foreign || tracked_.find(pid) != tracked_.end()) { saved = errno; break; }
        }
    }
    closedir(directory);
    if (saved) { errno = saved; return Failure(error, "proc_scan_incomplete"); }
    auto leader = tracked_.find(leader_);
    bool anchor = false;
    if (leader != tracked_.end()) {
        for (const auto &record : records)
            if (record.pid == leader_ && SameProcess(record, leader->second.identity) && record.session == leader_) anchor = true;
    }
    siginfo_t wait = {};
    if (!anchor || waitid(P_PID, static_cast<id_t>(leader_), &wait, WEXITED | WNOHANG | WNOWAIT) != 0) {
        if (!anchor) errno = ESTALE;
        return Failure(error, "session_anchor_unverified");
    }
    std::map<pid_t, unsigned long long> original;
    for (const auto &item : tracked_) original[item.first] = item.second.identity.start;
    auto rollback = [&]() {
        for (auto item = tracked_.begin(); item != tracked_.end();) {
            auto old = original.find(item->first);
            if (old == original.end() || old->second != item->second.identity.start) item = tracked_.erase(item);
            else ++item;
        }
    };
    bool added = true;
    while (added) {
        added = false;
        for (const auto &identity : records) {
            bool owned = identity.session == leader_;
            auto parent = tracked_.find(identity.parent);
            if (!owned && parent != tracked_.end()) {
                // Confirm the parent record has the pinned start time before
                // extending ownership across a descendant's setsid().
                for (const auto &record : records)
                    if (record.pid == identity.parent && SameProcess(record, parent->second.identity)) owned = true;
            }
            if (!owned) continue;
            size_t before = tracked_.size();
            if (!Pin(identity, error)) { rollback(); return false; }
            if (tracked_.size() > before) added = true;
        }
    }
    ProcessIdentity confirmed; bool foreign = false;
    leader = tracked_.find(leader_);
    if (leader == tracked_.end() || !ReadIdentity(proc.get(), leader_, confirmed, foreign) ||
        !SameProcess(leader->second.identity, confirmed) || confirmed.session != leader_ ||
        waitid(P_PID, static_cast<id_t>(leader_), &wait, WEXITED | WNOHANG | WNOWAIT) != 0) {
        rollback(); errno = ESTALE; return Failure(error, "session_anchor_changed");
    }
    return true;
}
bool OwnedProcesses::Signal(int number, std::string &error, int &signalErrno) {
    bool success = true; signalErrno = 0;
    for (auto &item : tracked_) {
        if (HandleSignal(item.second.handle.get(), number) != 0 && errno != ESRCH) {
            if (!signalErrno) { signalErrno = errno; Failure(error, "pidfd_send_signal"); }
            success = false;
        }
    }
    return success;
}
int OwnedProcesses::Alive() const {
    int alive = 0;
    for (const auto &item : tracked_) {
        struct pollfd event = {item.second.handle.get(), POLLIN, 0};
        int result = poll(&event, 1, 0);
        // poll errors are unknown, never a claim that the process is gone.
        if (result != 1 || !(event.revents & POLLIN)) ++alive;
    }
    return alive;
}
void OwnedProcesses::Clear() { tracked_.clear(); leader_ = -1; }
}
