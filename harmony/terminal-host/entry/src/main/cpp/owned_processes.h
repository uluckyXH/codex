#pragma once
#include "host_layout.h"
#include <map>
#include <string>
#include <sys/types.h>

namespace codex_hnp {
struct ProcessIdentity {
    pid_t pid = -1;
    pid_t parent = -1;
    pid_t group = -1;
    pid_t session = -1;
    unsigned long long start = 0;
    char state = 0;
    uid_t realUid = 0;
    uid_t effectiveUid = 0;
    uid_t savedUid = 0;
    uid_t filesystemUid = 0;
};
bool ParseProcessUids(const std::string &record, ProcessIdentity &identity);
bool ParseProcessStat(const std::string &record, ProcessIdentity &identity);
bool StableProcessHandlesAvailable(std::string &error);
bool SignalDirectChild(pid_t child, int number, std::string &error);

// Tracks the session and observed descendants. Each signal uses a pidfd;
// recycled numeric PIDs and unrelated terminal sessions are never signalled.
// This is terminal lifecycle management, not containment of hostile daemons.
class OwnedProcesses {
public:
    bool Begin(pid_t leader, std::string &error);
    // Observe remains strict: only a complete proc scan can authorize cleanup.
    bool Observe(std::string &error);
    // A live, pinned direct child can keep its terminal usable when proc is
    // unreadable. complete=false is diagnostic evidence, never cleanup proof.
    bool ObserveRunning(std::string &diagnostic, bool &complete);
    bool Signal(int number, std::string &error, int &signalErrno);
    int Alive() const;
    void Clear();
private:
    struct Tracked { ProcessIdentity identity; Descriptor handle; };
    bool Pin(const ProcessIdentity &identity, std::string &error);
    bool Scan(std::string &error, bool &visibilityUnknown);
    bool LeaderAlive(std::string &error) const;
    pid_t leader_ = -1;
    std::map<pid_t, Tracked> tracked_;
};
}
