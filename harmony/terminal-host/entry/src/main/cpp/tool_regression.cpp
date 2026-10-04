#include "tool_regression.h"
#include "native_package.h"
#include "owned_processes.h"
#include <cerrno>
#include <chrono>
#include <cstring>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

extern char **environ;

static int PrivateDirectory(int parent, const char *name, bool fresh = false) {
    if (mkdirat(parent, name, 0700) != 0 && (fresh || errno != EEXIST)) return -1;
    int fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    struct stat st = {};
    if (fd >= 0 && (fstat(fd, &st) != 0 || !S_ISDIR(st.st_mode) || st.st_uid != geteuid() ||
        st.st_gid != getegid() || (st.st_mode & 07777) != 0700)) {
        close(fd); errno = EACCES; return -1;
    }
    return fd;
}

static bool ExpectFile(int directory, const char *name, const char *expected, FILE *report, bool privateLog = false) {
    int fd = openat(directory, name, O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (fd < 0) { fprintf(report, "assert_file=%s open_errno=%d\n", name, errno); return false; }
    struct stat st = {};
    char data[256] = {};
    bool valid = fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_uid == geteuid() &&
        st.st_nlink == 1 && (!privateLog || (st.st_mode & 07777) == 0600) && st.st_size == static_cast<off_t>(strlen(expected));
    ssize_t count = valid ? read(fd, data, sizeof(data)) : -1;
    close(fd);
    bool equal = valid && count == st.st_size && memcmp(data, expected, static_cast<size_t>(count)) == 0;
    fprintf(report, "assert_file=%s uid=%u gid=%u mode=%o links=%lu bytes=%lld read_bytes=%lld content_equal=%d private_log=%d\n",
        name, st.st_uid, st.st_gid, st.st_mode & 07777, static_cast<unsigned long>(st.st_nlink),
        static_cast<long long>(st.st_size), static_cast<long long>(count), equal ? 1 : 0, privateLog ? 1 : 0);
    return equal;
}

static bool Child(int logs, FILE *report, const std::string &prefix, const char *step,
                  const std::string &cwd, const std::vector<std::string> &command,
                  const std::atomic<bool> &cancelled, bool &cleanupUnknown, const char *expectedOutput = nullptr, bool builtInRead = false) {
    std::string trackingError;
    if (!codex_hnp::StableProcessHandlesAvailable(trackingError)) {
        fprintf(report, "step=%s stable_process_handles_unavailable errno=%d\n", step, errno); return false;
    }
    std::string outputName = prefix + "-" + step + ".stdout.txt";
    std::string errorName = prefix + "-" + step + ".stderr.txt";
    int output = openat(logs, outputName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    int error = openat(logs, errorName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    int input = open("/dev/null", O_RDONLY | O_CLOEXEC);
    if (output < 0 || error < 0 || input < 0) {
        fprintf(report, "step=%s stdio_errno=%d\n", step, errno);
        if (output >= 0) close(output); if (error >= 0) close(error); if (input >= 0) close(input);
        return false;
    }
    // This shell script is constant. All paths and patch payloads are separate argv values.
    const char *script = builtInRead ?
        "umask 077 || exit 124; cd \"$1\" || exit 125; shift; while IFS= read -r line; do printf \"%s\\n\" \"$line\"; done < \"$1\"" :
        "umask 077 || exit 124; cd \"$1\" || exit 125; shift; exec \"$@\"";
    std::vector<std::string> args = {"/system/bin/sh", "-c", script, "codex-tool-test", cwd};
    args.insert(args.end(), command.begin(), command.end());
    std::vector<char *> argv;
    for (auto &arg : args) argv.push_back(arg.data());
    argv.push_back(nullptr);
    std::vector<std::string> env;
    const char *keys[] = {"HOME=", "CODEX_HOME=", "TMPDIR=", "PATH=", "SHELL=", "TERM="};
    for (char **entry = environ; *entry; ++entry) {
        bool replaced = false;
        for (const char *key : keys) if (strncmp(*entry, key, strlen(key)) == 0) replaced = true;
        if (!replaced) env.emplace_back(*entry);
    }
    // Dedicated fixtures prevent this regression from reading the real application configuration.
    env.insert(env.end(), {"HOME=" + cwd + "/home", "CODEX_HOME=" + cwd + "/state", "TMPDIR=" + cwd + "/tmp",
        std::string("PATH=") + CODEX_HNP_PACKAGE_PATH + "/bin:" + CODEX_HNP_PACKAGE_PATH + "/codex-path:/system/bin:/system/xbin:/bin",
        "SHELL=/system/bin/sh", "TERM=dumb"});
    std::vector<char *> envp;
    for (auto &value : env) envp.push_back(value.data());
    envp.push_back(nullptr);
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attributes;
    int result = posix_spawn_file_actions_init(&actions);
    bool actionsReady = result == 0, attributesReady = false;
    if (!result) { result = posix_spawnattr_init(&attributes); attributesReady = result == 0; }
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, input, STDIN_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, output, STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, error, STDERR_FILENO);
    if (!result) result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSID);
    pid_t child = -1;
    if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
    if (attributesReady) posix_spawnattr_destroy(&attributes);
    if (actionsReady) posix_spawn_file_actions_destroy(&actions);
    close(input); close(output); close(error);
    fprintf(report, "step=%s spawn_result=%d child_pid=%d stdout=%s stderr=%s\n", step, result, child, outputName.c_str(), errorName.c_str());
    fflush(report);
    if (result) return false;
    int status = 0; bool stopped = false, stopping = false;
    codex_hnp::OwnedProcesses owned; std::string failure;
    if (!owned.Begin(child, failure)) {
        codex_hnp::SignalDirectChild(child, SIGKILL, failure);
        cleanupUnknown = true;
        fprintf(report, "step=%s tracking_failed errno=%d cleanup_unverified=yes\n", step, errno); return false;
    }
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(30);
    auto stopAt = deadline, nextSignal = deadline;
    for (;;) {
        siginfo_t information = {};
        int result = waitid(P_PID, static_cast<id_t>(child), &information, WEXITED | WNOHANG | WNOWAIT);
        if (result != 0 && errno != EINTR) {
            cleanupUnknown = true;
            fprintf(report, "step=%s waitid_errno=%d cleanup_unverified=yes\n", step, errno); return false;
        }
        bool exited = result == 0 && information.si_pid == child;
        bool observed = owned.Observe(failure);
        auto now = std::chrono::steady_clock::now();
        if (!stopping && (exited || cancelled || now >= deadline)) {
            stopped = cancelled || now >= deadline;
            stopping = true; stopAt = now; nextSignal = now + std::chrono::seconds(1);
            int signalError = 0; owned.Signal(SIGTERM, failure, signalError);
            fprintf(report, "step=%s term_errno=%d\n", step, signalError);
        }
        if (stopping && now >= nextSignal) {
            int signalError = 0; owned.Signal(SIGKILL, failure, signalError);
            fprintf(report, "step=%s kill_errno=%d survivors=%d\n", step, signalError, owned.Alive());
            nextSignal = now + std::chrono::milliseconds(250);
        }
        if (exited && observed && owned.Alive() == 0) {
            pid_t waited = waitpid(child, &status, WNOHANG);
            if (waited == child) break;
            if (waited < 0 && errno != EINTR) {
                cleanupUnknown = true;
                fprintf(report, "step=%s wait_errno=%d cleanup_unverified=yes\n", step, errno); return false;
            }
        }
        if (stopping && now - stopAt > std::chrono::seconds(5)) {
            cleanupUnknown = true;
            fprintf(report, "step=%s cleanup_unverified=yes survivors=%d\n", step, owned.Alive()); return false;
        }
        fflush(report); usleep(25000);
    }
    bool passed = !stopped && WIFEXITED(status) && WEXITSTATUS(status) == 0;
    if (passed && expectedOutput) passed = ExpectFile(logs, outputName.c_str(), expectedOutput, report, true);
    fprintf(report, "step=%s exit_code=%d signal=%d raw_wait_status=%d core_dumped=%d stopped=%d passed=%d\n", step,
        WIFEXITED(status) ? WEXITSTATUS(status) : -1, WIFSIGNALED(status) ? WTERMSIG(status) : 0, status,
        WIFSIGNALED(status) && WCOREDUMP(status) ? 1 : 0, stopped ? 1 : 0, passed ? 1 : 0);
    fflush(report);
    return passed;
}

bool RunToolRegression(int files, const std::string &dataRoot, int logs, FILE *report, const std::string &prefix,
                       const std::atomic<bool> &cancelled, bool &cleanupUnknown) {
    cleanupUnknown = false;
    // Probe only fixed system paths. A PATH lookup failure is not proof that
    // the command is absent from the OS or unavailable in a different host.
    for (const char *path : {"/system/bin/sh", "/bin/bash", "/system/bin/cat", "/system/bin/toybox", "/bin/cat", "/system/xbin/cat",
                             "/system/bin/git", "/system/bin/node", "/system/bin/python3"}) {
        errno = 0; int executable = access(path, X_OK); int accessError = errno;
        struct stat linkMetadata = {}; errno = 0; int linkObserved = lstat(path, &linkMetadata); int linkError = errno;
        struct stat metadata = {}; errno = 0; int observed = stat(path, &metadata); int statError = errno;
        // Shell lookup uses stat, while earlier diagnostics only used lstat.
        // Keep both results: execution permission does not imply metadata access.
        fprintf(report, "system_command=%s access_x=%d access_errno=%d lstat=%d lstat_errno=%d link_mode=%o stat=%d stat_errno=%d mode=%o\n",
            path, executable, accessError, linkObserved, linkError, linkObserved == 0 ? linkMetadata.st_mode & 07777 : 0,
            observed, statError, observed == 0 ? metadata.st_mode & 07777 : 0);
    }
    int workspace = PrivateDirectory(files, "workspace");
    if (workspace < 0) { fprintf(report, "tool_setup_errno=%d\n", errno); return false; }
    std::string name = "tools-" + std::to_string(getpid()) + "-" + prefix;
    int fixture = PrivateDirectory(workspace, name.c_str(), true); close(workspace);
    if (fixture < 0) { fprintf(report, "tool_fixture_errno=%d\n", errno); return false; }
    for (const char *child : {"home", "state", "tmp"}) {
        int directory = PrivateDirectory(fixture, child, true);
        if (directory < 0) { close(fixture); fprintf(report, "tool_private_fixture_errno=%d\n", errno); return false; }
        close(directory);
    }
    std::string cwd = dataRoot + "/workspace/" + name;
    std::string package = CODEX_HNP_PACKAGE_PATH;
    fprintf(report, "tool_fixture=%s mode=0700 real_config_read=false\n", cwd.c_str());
    int maskCheck = openat(fixture, "umask-check.txt", O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0666);
    struct stat maskMetadata = {};
    if (maskCheck < 0 || fstat(maskCheck, &maskMetadata) != 0) {
        if (maskCheck >= 0) close(maskCheck);
        close(fixture); fprintf(report, "native_umask_probe_errno=%d\n", errno); return false;
    }
    fprintf(report, "native_open_requested_mode=666 observed_mode=%o uid=%u\n", maskMetadata.st_mode & 07777, maskMetadata.st_uid);
    close(maskCheck); unlinkat(fixture, "umask-check.txt", 0);
    bool passed = Child(logs, report, prefix, "rg-version", cwd, {package + "/codex-path/rg", "--version"}, cancelled, cleanupUnknown);
    passed = passed && Child(logs, report, prefix, "create", cwd,
        {package + "/codex-path/apply_patch", "*** Begin Patch\n*** Add File: sample.txt\n+harmony initial\n*** End Patch"}, cancelled, cleanupUnknown);
    passed = passed && ExpectFile(fixture, "sample.txt", "harmony initial\n", report);
    fprintf(report, "assert=create_content passed=%d\n", passed ? 1 : 0);
    passed = passed && Child(logs, report, prefix, "modify", cwd,
        {package + "/codex-path/applypatch", "*** Begin Patch\n*** Update File: sample.txt\n@@\n-harmony initial\n+harmony updated\n*** End Patch"}, cancelled, cleanupUnknown);
    passed = passed && ExpectFile(fixture, "sample.txt", "harmony updated\n", report);
    fprintf(report, "assert=modify_content passed=%d\n", passed ? 1 : 0);
    passed = passed && Child(logs, report, prefix, "read", cwd,
        {"sample.txt"}, cancelled, cleanupUnknown, "harmony updated\n", true);
    passed = passed && Child(logs, report, prefix, "toybox-cat", cwd,
        {"/system/bin/toybox", "cat", "sample.txt"}, cancelled, cleanupUnknown, "harmony updated\n");
    passed = passed && Child(logs, report, prefix, "toybox-ls", cwd,
        {"/system/bin/toybox", "ls", "-1", "."}, cancelled, cleanupUnknown, "home\nsample.txt\nstate\ntmp\n");
    passed = passed && Child(logs, report, prefix, "search", cwd,
        {package + "/codex-path/rg", "--fixed-strings", "--line-number", "--no-heading", "updated", "sample.txt"}, cancelled, cleanupUnknown, "1:harmony updated\n");
    passed = passed && Child(logs, report, prefix, "delete", cwd,
        {package + "/codex-path/apply_patch", "*** Begin Patch\n*** Delete File: sample.txt\n*** End Patch"}, cancelled, cleanupUnknown);
    struct stat st = {};
    passed = passed && fstatat(fixture, "sample.txt", &st, AT_SYMLINK_NOFOLLOW) != 0 && errno == ENOENT;
    fprintf(report, "assert=deleted passed=%d\n", passed ? 1 : 0);
    close(fixture);
    return passed;
}
