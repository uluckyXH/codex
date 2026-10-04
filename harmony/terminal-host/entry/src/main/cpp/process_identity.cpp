#include "owned_processes.h"
#include <cerrno>
#include <cstdlib>
#include <limits>
#include <sstream>

namespace codex_hnp {
bool ParseProcessUids(const std::string &record, ProcessIdentity &identity) {
    size_t start = record.find("Uid:");
    while (start != std::string::npos && start > 0 && record[start - 1] != '\n') start = record.find("Uid:", start + 1);
    if (start == std::string::npos) return false;
    size_t end = record.find('\n', start);
    std::istringstream fields(record.substr(start + 4, end == std::string::npos ? end : end - start - 4));
    unsigned long long values[4];
    for (auto &value : values) {
        std::string token;
        if (!(fields >> token) || token.empty() || token.find_first_not_of("0123456789") != std::string::npos) return false;
        errno = 0; char *tail = nullptr; value = strtoull(token.c_str(), &tail, 10);
        if (errno || !tail || *tail || value > std::numeric_limits<uid_t>::max()) return false;
    }
    fields >> std::ws; if (!fields.eof()) return false;
    identity.realUid = static_cast<uid_t>(values[0]); identity.effectiveUid = static_cast<uid_t>(values[1]);
    identity.savedUid = static_cast<uid_t>(values[2]); identity.filesystemUid = static_cast<uid_t>(values[3]);
    return true;
}
bool ParseProcessStat(const std::string &record, ProcessIdentity &identity) {
    size_t open = record.find('('), close = record.rfind(')');
    if (open == std::string::npos || close <= open || close + 2 >= record.size()) return false;
    std::istringstream prefix(record.substr(0, open)); long long pid;
    if (!(prefix >> pid) || pid <= 0 || pid > std::numeric_limits<pid_t>::max()) return false;
    prefix >> std::ws; if (!prefix.eof()) return false;
    std::istringstream fields(record.substr(close + 2));
    std::string tokens[20];
    for (auto &token : tokens) if (!(fields >> token)) return false;
    if (tokens[0].size() != 1) return false;
    unsigned long long parsed[4] = {};
    unsigned indices[] = {1, 2, 3, 19};
    for (unsigned i = 0; i < 4; ++i) {
        const auto &token = tokens[indices[i]];
        if (token.empty() || token.find_first_not_of("0123456789") != std::string::npos) return false;
        errno = 0; char *end = nullptr;
        parsed[i] = strtoull(token.c_str(), &end, 10);
        if (errno || !end || *end) return false;
        if (i < 3 && parsed[i] > static_cast<unsigned long long>(std::numeric_limits<pid_t>::max())) return false;
    }
    // Application hosts can legitimately have pgrp/session 0 in their PID
    // namespace. Observation is not authorization: Begin separately requires
    // a spawned terminal leader whose nonzero group/session equal its PID.
    if (!parsed[3]) return false;
    identity = {static_cast<pid_t>(pid), static_cast<pid_t>(parsed[0]), static_cast<pid_t>(parsed[1]),
                static_cast<pid_t>(parsed[2]), parsed[3], tokens[0][0]};
    return true;
}
}
