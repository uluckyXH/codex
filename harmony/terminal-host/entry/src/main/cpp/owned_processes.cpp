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
bool HandleExited(int fd) {
    struct pollfd event = {fd, POLLIN, 0};
    return poll(&event, 1, 0) == 1 && (event.revents & POLLIN) && !(event.revents & (POLLERR | POLLNVAL));
}
bool Failure(std::string &error, const char *stage, pid_t pid = -1) {
    int saved = errno;
    error = std::string(stage);
    if (pid > 0) error += " pid=" + std::to_string(pid);
    error += " errno=" + std::to_string(saved); errno = saved; return false;
}
bool ReadStatus(int directory, pid_t pid, ProcessIdentity &identity, std::string &error) {
    Descriptor input(openat(directory, "status", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
    if (input.get() < 0) return Failure(error, "status_open", pid);
    char contents[4096]; ssize_t count;
    do { count = read(input.get(), contents, sizeof(contents)); } while (count < 0 && errno == EINTR);
    if (count < 0) return Failure(error, "status_read", pid);
    if (!count || !ParseProcessUids(std::string(contents, count), identity)) {
        errno = EIO; return Failure(error, "status_parse", pid);
    }
    return true;
}
bool ForeignIdentity(const ProcessIdentity &identity) {
    // Signal permission depends on real/saved/effective process UIDs, never
    // proc inode ownership (which can be root when dumpable is disabled).
    uid_t effective = geteuid(), real = getuid();
    return identity.realUid != effective && identity.effectiveUid != effective && identity.savedUid != effective &&
           identity.realUid != real && identity.effectiveUid != real && identity.savedUid != real;
}
bool ReadIdentity(int proc, pid_t pid, ProcessIdentity &identity, bool &provenForeign, std::string &error) {
    provenForeign = false;
    Descriptor directory(openat(proc, std::to_string(pid).c_str(), O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (directory.get() < 0) return Failure(error, "pid_directory_open", pid);
    ProcessIdentity status;
    std::string statusFailure;
    bool statusRead = ReadStatus(directory.get(), pid, status, statusFailure);
    int statusError = statusRead ? 0 : errno;
    if (statusRead) provenForeign = ForeignIdentity(status);
    Descriptor input(openat(directory.get(), "stat", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
    if (input.get() < 0) return Failure(error, "stat_open", pid);
    char contents[4097]; ssize_t count;
    do { count = read(input.get(), contents, sizeof(contents)); } while (count < 0 && errno == EINTR);
    if (count < 0) return Failure(error, "stat_read", pid);
    if (!count || count > 4096 || !ParseProcessStat(std::string(contents, count), identity) || identity.pid != pid) {
        errno = EIO; return Failure(error, "stat_parse", pid);
    }
    if (statusRead) {
        identity.realUid = status.realUid; identity.effectiveUid = status.effectiveUid;
        identity.savedUid = status.savedUid; identity.filesystemUid = status.filesystemUid;
    } else {
        // Membership can be inspected, but pinning must establish actual UID.
        error = statusFailure; errno = statusError; return false;
    }
    return true;
}
bool SameProcess(const ProcessIdentity &a, const ProcessIdentity &b) { return a.pid == b.pid && a.start == b.start; }
bool SameAnchor(const ProcessIdentity &expected, const ProcessIdentity &actual) {
    return SameProcess(expected, actual) && actual.session == expected.pid &&
        expected.realUid == actual.realUid && expected.effectiveUid == actual.effectiveUid && expected.savedUid == actual.savedUid;
}
std::string AnchorDetail(const ProcessIdentity &expected, const ProcessIdentity &actual, bool enumerated) {
    return "pid=" + std::to_string(actual.pid) + " enumerated=" + std::to_string(enumerated) +
        " expected_start=" + std::to_string(expected.start) + " actual_start=" + std::to_string(actual.start) +
        " group=" + std::to_string(actual.group) + " sid=" + std::to_string(actual.session) +
        " real_uid=" + std::to_string(actual.realUid) + " effective_uid=" + std::to_string(actual.effectiveUid) +
        " saved_uid=" + std::to_string(actual.savedUid);
}
}

bool StableProcessHandlesAvailable(std::string &error) {
    Descriptor handle(PidHandle(getpid()));
    if (handle.get() < 0 || HandleSignal(handle.get(), 0) != 0) return Failure(error, "pidfd_unavailable", getpid());
    return true;
}
bool SignalDirectChild(pid_t child, int number, std::string &error) {
    Descriptor handle(PidHandle(child));
    if (handle.get() < 0) return Failure(error, "direct_child_pidfd", child);
    siginfo_t information = {};
    // A reused non-child PID is rejected after pinning; an intervening reap
    // cannot redirect a signal sent through the already opened handle.
    if (waitid(P_PID, static_cast<id_t>(child), &information, WEXITED | WNOHANG | WNOWAIT) != 0)
        return Failure(error, "direct_child_ownership", child);
    if (HandleSignal(handle.get(), number) != 0 && errno != ESRCH) return Failure(error, "direct_child_signal", child);
    return true;
}
bool OwnedProcesses::Pin(const ProcessIdentity &identity, std::string &error) {
    auto previous = tracked_.find(identity.pid);
    if (previous != tracked_.end()) {
        if (SameProcess(previous->second.identity, identity)) return true;
        // Old handles cannot refer to the new occupant. Keep the old process
        // accounted for unless its stable handle confirms it has exited.
        if (!HandleExited(previous->second.handle.get())) { errno = ESTALE; return Failure(error, "pid_replaced_while_alive", identity.pid); }
        tracked_.erase(previous);
    }
    if (tracked_.size() >= 512) { errno = EOVERFLOW; return Failure(error, "process_limit", identity.pid); }
    Descriptor handle(PidHandle(identity.pid));
    if (handle.get() < 0) {
        if (errno == ESRCH || errno == ENOENT) return true;
        return Failure(error, "pidfd_open", identity.pid);
    }
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    ProcessIdentity confirmed; bool foreign = false;
    if (proc.get() < 0) return Failure(error, "pin_proc_open", identity.pid);
    if (!ReadIdentity(proc.get(), identity.pid, confirmed, foreign, error)) {
        int identityError = errno;
        // ENOENT can mean a hardened proc view, even after pidfd_open succeeds.
        // Only that stable handle becoming readable proves this process gone.
        if ((identityError == ESRCH || identityError == ENOENT) && HandleExited(handle.get())) return true;
        errno = identityError;
        return false;
    }
    if (!SameProcess(identity, confirmed) || identity.session != confirmed.session || identity.parent != confirmed.parent ||
        identity.realUid != confirmed.realUid || identity.effectiveUid != confirmed.effectiveUid || identity.savedUid != confirmed.savedUid) {
        errno = ESTALE; return Failure(error, "pidfd_identity_changed", identity.pid);
    }
    tracked_.emplace(identity.pid, Tracked{confirmed, std::move(handle)}); return true;
}
bool OwnedProcesses::Begin(pid_t leader, std::string &error) {
    Clear(); leader_ = leader;
    if (leader <= 0) { errno = EINVAL; return Failure(error, "invalid_leader"); }
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    ProcessIdentity identity; bool foreign = false;
    if (proc.get() < 0) return Failure(error, "leader_proc_open", leader);
    if (!ReadIdentity(proc.get(), leader, identity, foreign, error)) return false;
    if (identity.realUid != getuid() || identity.effectiveUid != geteuid()) { errno = EACCES; return Failure(error, "leader_uid", leader); }
    if (identity.session != leader || identity.group != leader) { errno = EACCES; return Failure(error, "leader_session", leader); }
    if (!Pin(identity, error)) return false;
    // Pin may ignore a process that disappeared between metadata and pidfd
    // reads. Startup must not mistake that case for a pinned leader.
    if (tracked_.find(leader_) == tracked_.end()) { errno = ESTALE; return Failure(error, "leader_not_pinned", leader_); }
    siginfo_t information = {};
    if (waitid(P_PID, static_cast<id_t>(leader_), &information, WEXITED | WNOHANG | WNOWAIT) != 0)
        return Failure(error, "leader_wait_ownership", leader_);
    return true;
}
bool OwnedProcesses::LeaderAlive(std::string &error) const {
    auto leader = tracked_.find(leader_);
    if (leader_ <= 0 || leader == tracked_.end()) { errno = ESTALE; return Failure(error, "session_anchor_missing", leader_); }
    auto liveHandle = [&]() {
        struct pollfd event = {leader->second.handle.get(), POLLIN, 0};
        int result;
        do { result = poll(&event, 1, 0); } while (result < 0 && errno == EINTR);
        if (result < 0) return Failure(error, "session_anchor_pidfd_poll", leader_);
        if (result != 0 || event.revents) {
            errno = event.revents & POLLNVAL ? EBADF : event.revents & POLLIN ? ESRCH : EIO;
            return Failure(error, "session_anchor_pidfd_not_live", leader_);
        }
        return true;
    };
    if (!liveHandle()) return false;
    siginfo_t information = {};
    int result;
    do { result = waitid(P_PID, static_cast<id_t>(leader_), &information, WEXITED | WNOHANG | WNOWAIT); }
    while (result < 0 && errno == EINTR);
    if (result != 0) return Failure(error, "session_anchor_wait_ownership", leader_);
    if (information.si_pid != 0) { errno = ESRCH; return Failure(error, "session_anchor_exit_pending", leader_); }
    // Bracket the numeric wait with the same stable handle. A reap/reuse race
    // cannot turn another child into proof that this pinned process is alive.
    // WEXITED deliberately leaves stopped/continued events untouched: a
    // suspended child remains live and input stays in the bounded PTY queue.
    return liveHandle();
}
bool OwnedProcesses::Observe(std::string &error) {
    bool visibilityUnknown = false;
    return Scan(error, visibilityUnknown);
}
bool OwnedProcesses::ObserveRunning(std::string &diagnostic, bool &complete) {
    bool visibilityUnknown = false;
    complete = Scan(diagnostic, visibilityUnknown);
    if (complete) return true;
    int scanError = errno;
    if (!visibilityUnknown) return false;
    std::string scanFailure = diagnostic, liveFailure;
    if (!LeaderAlive(liveFailure)) {
        int liveError = errno;
        diagnostic = liveFailure + " proc_cause=" + scanFailure; errno = liveError; return false;
    }
    diagnostic = "cause=" + scanFailure + " pid=" + std::to_string(leader_) +
        " pidfd_alive=1 wait_owned=1 proc_complete=0";
    errno = scanError; return true;
}
bool OwnedProcesses::Scan(std::string &error, bool &visibilityUnknown) {
    visibilityUnknown = false;
    if (leader_ <= 0) { errno = EINVAL; return Failure(error, "no_owned_session"); }
    auto leader = tracked_.find(leader_);
    if (leader == tracked_.end()) { errno = ESTALE; return Failure(error, "session_anchor_missing", leader_); }
    Descriptor proc(open("/proc", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC));
    if (proc.get() < 0) { visibilityUnknown = true; return Failure(error, "proc_open"); }
    ProcessIdentity direct; bool directForeign = false;
    if (!ReadIdentity(proc.get(), leader_, direct, directForeign, error)) { visibilityUnknown = true; return false; }
    if (!SameAnchor(leader->second.identity, direct)) {
        errno = ESTALE; Failure(error, "session_anchor_direct_changed", leader_);
        error += " " + AnchorDetail(leader->second.identity, direct, false); return false;
    }
    siginfo_t wait = {};
    if (waitid(P_PID, static_cast<id_t>(leader_), &wait, WEXITED | WNOHANG | WNOWAIT) != 0)
        return Failure(error, "session_anchor_direct_wait", leader_);
    Descriptor enumeration(dup(proc.get()));
    if (enumeration.get() < 0) { visibilityUnknown = true; return Failure(error, "proc_duplicate"); }
    DIR *directory = fdopendir(enumeration.get());
    if (!directory) { visibilityUnknown = true; return Failure(error, "proc_enumerate"); }
    enumeration.release();
    std::vector<ProcessIdentity> records;
    size_t entries = 0; int saved = 0; std::string savedFailure;
    for (;;) {
        errno = 0; dirent *entry = readdir(directory);
        if (!entry) { saved = errno; if (saved) Failure(savedFailure, "proc_readdir"); break; }
        if (++entries > 16384) { saved = EOVERFLOW; errno = saved; Failure(savedFailure, "proc_entry_limit"); break; }
        std::string name(entry->d_name);
        if (name.empty() || name.find_first_not_of("0123456789") != std::string::npos) continue;
        unsigned long value = strtoul(name.c_str(), nullptr, 10);
        if (!value || value > static_cast<unsigned long>(std::numeric_limits<pid_t>::max())) continue;
        ProcessIdentity identity; bool foreign = false; pid_t pid = static_cast<pid_t>(value);
        std::string failure;
        if (ReadIdentity(proc.get(), pid, identity, foreign, failure)) records.push_back(identity);
        else {
            int identityError = errno;
            // Unknown unreadable entries are not silently considered dead.
            // A readable status proving a foreign identity may be excluded,
            // except when that exact PID is already tracked by a stable handle.
            auto known = tracked_.find(pid);
            if (foreign && known == tracked_.end()) continue;
            if (identityError == ENOENT || identityError == ESRCH) {
                if (known == tracked_.end() || HandleExited(known->second.handle.get())) {
                    // This temporary handle is only a disappearance probe. It
                    // confers no ownership and is never stored or signalled.
                    // An old dead pin does not prove that a newly enumerated
                    // occupant of its numeric PID has also disappeared.
                    Descriptor probe(PidHandle(pid));
                    if ((probe.get() < 0 && errno == ESRCH) ||
                        (probe.get() >= 0 && HandleExited(probe.get()))) continue;
                }
            }
            saved = identityError; savedFailure = failure; break;
        }
    }
    closedir(directory);
    if (saved) { visibilityUnknown = true; error = "proc_scan_incomplete cause=" + savedFailure; errno = saved; return false; }
    bool enumerated = false;
    for (const auto &record : records) {
        if (record.pid != leader_) continue;
        enumerated = true;
        if (!SameAnchor(direct, record)) {
            errno = ESTALE; Failure(error, "session_anchor_enumerated_changed", leader_);
            error += " " + AnchorDetail(direct, record, true); return false;
        }
    }
    // readdir visibility is not the identity authority. A directly verified,
    // still-waitable child may be absent after hardening; keep that anchor in
    // the parent/session graph without treating missing jobs as dead.
    if (!enumerated) records.push_back(direct);
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
            if (!Pin(identity, error)) {
                int savedPin = errno; rollback();
                visibilityUnknown = savedPin == ENOENT || savedPin == ESRCH || savedPin == EACCES || savedPin == EPERM || savedPin == EIO;
                errno = savedPin; return false;
            }
            if (tracked_.size() > before) added = true;
        }
    }
    ProcessIdentity confirmed; bool foreign = false;
    leader = tracked_.find(leader_);
    if (leader == tracked_.end()) {
        rollback(); errno = ESTALE; return Failure(error, "session_anchor_missing", leader_);
    }
    if (!ReadIdentity(proc.get(), leader_, confirmed, foreign, error)) {
        int savedIdentity = errno; rollback(); visibilityUnknown = true; errno = savedIdentity; return false;
    }
    if (!SameAnchor(leader->second.identity, confirmed)) {
        std::string detail = AnchorDetail(leader->second.identity, confirmed, enumerated);
        rollback(); errno = ESTALE; Failure(error, "session_anchor_changed", leader_); error += " " + detail; return false;
    }
    if (waitid(P_PID, static_cast<id_t>(leader_), &wait, WEXITED | WNOHANG | WNOWAIT) != 0) {
        int savedWait = errno; rollback(); errno = savedWait; return Failure(error, "session_anchor_wait", leader_);
    }
    error = "session_anchor_direct " + AnchorDetail(leader->second.identity, confirmed, enumerated) + " wait_owned=1";
    if (!enumerated) {
        // A demonstrably filtered enumeration is never complete. Running
        // callers may separately prove leader liveness, but cleanup callers
        // retain the anchor/handles until visibility is verified after exit.
        visibilityUnknown = true; error = "proc_visibility_unverified " + error; errno = EACCES; return false;
    }
    return true;
}
bool OwnedProcesses::Signal(int number, std::string &error, int &signalErrno) {
    bool success = true; signalErrno = 0;
    for (auto &item : tracked_) {
        if (HandleSignal(item.second.handle.get(), number) != 0 && errno != ESRCH) {
            if (!signalErrno) { signalErrno = errno; Failure(error, "pidfd_send_signal", item.first); }
            success = false;
        }
    }
    return success;
}
int OwnedProcesses::Alive() const {
    int alive = 0;
    for (const auto &item : tracked_) {
        // poll errors are unknown, never a claim that the process is gone.
        if (!HandleExited(item.second.handle.get())) ++alive;
    }
    return alive;
}
void OwnedProcesses::Clear() { tracked_.clear(); leader_ = -1; }
}
