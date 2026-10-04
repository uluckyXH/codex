#include "codex_launcher.h"
#include "native_package.h"
#include "secure_config.h"
#include "tool_regression.h"
#include "pty_session.h"
#include "package_verification.h"
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <signal.h>
#include <spawn.h>
#include <string>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

extern char **environ;
static constexpr const char *kFiles = "/data/storage/el2/base/files";
static constexpr const char *kPackage = CODEX_HNP_PACKAGE_PATH;
static std::atomic<bool> active{false};
static std::atomic<bool> cancellation{false};
static std::atomic<unsigned> sequence{0};
static std::atomic<bool> terminalReserved{false};

struct Work {
    napi_async_work work = nullptr;
    napi_deferred deferred = nullptr;
    std::string action;
    std::string result;
};

static bool Private(int fd) {
    struct stat st = {};
    return fstat(fd, &st) == 0 && S_ISDIR(st.st_mode) && st.st_uid == geteuid() &&
           st.st_gid == getegid() && (st.st_mode & 07777) == 0700;
}

static int Directory(int parent, const char *name) {
    if (mkdirat(parent, name, 0700) != 0 && errno != EEXIST) return -1;
    int fd = openat(parent, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd >= 0 && !Private(fd)) { close(fd); errno = EACCES; return -1; }
    return fd;
}

static int PrepareFiles() {
    // This debug prototype is bound to one installed application UID, not generic HOME.
    if (geteuid() != 20020059 || getegid() != 20020059) { errno = EACCES; return -1; }
    int parent = open("/", O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (parent < 0) return -1;
    const char *parts[] = {"data", "storage", "el2", "base", "files"};
    for (unsigned i = 0; i < 5; ++i) {
        int next = openat(parent, parts[i], (i < 3 ? O_PATH : O_RDONLY) | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        close(parent);
        if (next < 0) return -1;
        struct stat st = {};
        if (fstat(next, &st) != 0 || !S_ISDIR(st.st_mode)) { close(next); errno = EACCES; return -1; }
        if (i < 3) {
            if (st.st_uid != 0 || st.st_gid != 0 || (st.st_mode & 07777) != 0711) {
                close(next); errno = EACCES; return -1;
            }
        } else {
            if (st.st_uid != geteuid() || st.st_gid != getegid() ||
                ((st.st_mode & 07777) != 0777 && (st.st_mode & 07777) != 0700)) {
                close(next); errno = EACCES; return -1;
            }
            // Only our own dedicated HAP base/files can be tightened; no shared HOME.
            if (fchmod(next, 0700) != 0 || !Private(next)) { close(next); errno = EACCES; return -1; }
        }
        parent = next;
    }
    const char *names[] = {"r", "home", "state", "workspace", "tmp", "logs", "handoff-private", "control"};
    for (const char *name : names) {
        int fd = Directory(parent, name);
        if (fd < 0) { int error = errno; close(parent); errno = error; return -1; }
        close(fd);
    }
    return parent;
}

static std::vector<char *> Pointers(std::vector<std::string> &strings) {
    std::vector<char *> pointers;
    for (std::string &value : strings) pointers.push_back(value.data());
    pointers.push_back(nullptr);
    return pointers;
}

static void Execute(napi_env, void *data) {
    Work *work = static_cast<Work *>(data);
    int files = PrepareFiles();
    if (files < 0) { work->result = "prepare_errno=" + std::to_string(errno); return; }
    int logs = Directory(files, "logs");
    if (logs < 0) { close(files); work->result = "logs_errno=" + std::to_string(errno); return; }
    std::string name = "codex-" + work->action + "-" + std::to_string(getpid()) + "-" + std::to_string(++sequence);
    std::string reportName = name + ".status.txt";
    int reportFd = openat(logs, reportName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (reportFd < 0) { close(logs); close(files); work->result = "report_errno=" + std::to_string(errno); return; }
    FILE *report = fdopen(reportFd, "w");
    if (!report) { close(reportFd); close(logs); close(files); work->result = "report_fdopen_failed"; return; }
    work->result = std::string(kFiles) + "/logs/" + reportName;
    fprintf(report, "action=%s uid=%u gid=%u\n", work->action.c_str(), geteuid(), getegid());
    if (work->action == "verify-install") {
        bool passed = ExportInstalledPackageForVerification(files, report, name);
        fprintf(report, "verification_export_complete=yes passed=%d\n", passed ? 1 : 0);
        fclose(report); close(logs); close(files); return;
    }
    if (work->action == "tools") {
        bool passed = RunToolRegression(files, logs, report, name, cancellation);
        fprintf(report, "tool_regression_complete=yes passed=%d\n", passed ? 1 : 0);
        fclose(report); close(logs); close(files); return;
    }
    int imported = ImportPrivateConfig(files, report);
    if (imported < 0 || work->action == "prepare" || work->action == "import") {
        fprintf(report, "launch_complete=no_child config_status=%d\n", imported);
        fclose(report); close(logs); close(files); return;
    }
    bool greeting = work->action == "greeting";
    if (greeting && imported == 0) {
        fprintf(report, "launch_rejected=no_private_config\n");
        fclose(report); close(logs); close(files); return;
    }
    std::string outputName = name + ".stdout.txt";
    std::string errorName = name + ".stderr.txt";
    int output = openat(logs, outputName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    int error = openat(logs, errorName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    int input = open("/dev/null", O_RDONLY | O_CLOEXEC);
    fprintf(report, "stdout=%s/logs/%s\nstderr=%s/logs/%s\n", kFiles, outputName.c_str(), kFiles, errorName.c_str());
    if (output < 0 || error < 0 || input < 0) {
        fprintf(report, "stdio_open_errno=%d\n", errno);
        if (output >= 0) close(output); if (error >= 0) close(error); if (input >= 0) close(input);
        fclose(report); close(logs); close(files); return;
    }
    std::string workspace = std::string(kFiles) + "/workspace";
    // Fixed script; no eval, no user text or credentials are interpolated into code.
    std::vector<std::string> args = {"/system/bin/sh", "-c", "umask 077; cd \"$1\" || exit 125; shift; exec \"$@\"", "codex-hnp", workspace};
    args.push_back(std::string(kPackage) + "/bin/codex");
    if (work->action == "version") args.push_back("--version");
    else if (work->action == "doctor") {
        args.insert(args.end(), {"doctor", "--capabilities", "--json"});
    } else {
        args.insert(args.end(), {"exec", "--json", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
            "--model", "gpt-5.6-terra", "--cd", workspace, "--disable", "unbounded_connection_retries",
            "-c", "analytics.enabled=false", "-c", "model_providers.proxy.request_max_retries=0", "-c", "model_providers.proxy.stream_max_retries=0", "-c", "approval_policy=\"never\"", "-c", "web_search=\"disabled\"",
            "只回复：你好。不要调用任何工具，不要读取或修改文件。"});
    }
    std::vector<std::string> env;
    const char *overrideKeys[] = {"HOME=", "CODEX_HOME=", "TMPDIR=", "PATH=", "TERM=", "SHELL="};
    for (char **entry = environ; *entry; ++entry) {
        bool replaced = false;
        for (const char *key : overrideKeys) if (strncmp(*entry, key, strlen(key)) == 0) replaced = true;
        if (!replaced) env.emplace_back(*entry);
    }
    env.push_back(std::string("HOME=") + kFiles + "/home");
    env.push_back(std::string("CODEX_HOME=") + kFiles + "/state");
    env.push_back(std::string("TMPDIR=") + kFiles + "/tmp");
    env.push_back(std::string("PATH=") + kPackage + "/codex-path:/system/bin:/system/xbin:/bin");
    env.push_back("TERM=dumb");
    env.push_back("SHELL=/system/bin/sh");
    auto argv = Pointers(args);
    auto envp = Pointers(env);
    posix_spawn_file_actions_t actions;
    posix_spawnattr_t attributes;
    int result = posix_spawn_file_actions_init(&actions);
    bool actionsReady = result == 0;
    if (!result) result = posix_spawnattr_init(&attributes);
    bool attributesReady = result == 0;
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, input, STDIN_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, output, STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, error, STDERR_FILENO);
    if (!result) result = posix_spawnattr_setpgroup(&attributes, 0);
    if (!result) result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETPGROUP);
    pid_t child = -1;
    if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
    if (attributesReady) posix_spawnattr_destroy(&attributes);
    if (actionsReady) posix_spawn_file_actions_destroy(&actions);
    int control = Directory(files, "control");
    if (control >= 0) unlinkat(control, "cancel", 0);
    close(input); close(output); close(error); close(logs); close(files);
    fprintf(report, "spawn_result=%d child_pid=%d\n", result, child); fflush(report);
    if (result) { if (control >= 0) close(control); fclose(report); return; }
    int status = 0;
    pid_t waited = 0;
    int limit = greeting ? 180 : 30;
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(limit);
    bool stopped = false;
    while (!waited) {
        waited = waitpid(child, &status, WNOHANG);
        if (waited < 0 && errno == EINTR) { waited = 0; continue; }
        if (waited != 0) break;
        if (control >= 0) {
            struct stat cancel = {};
            if (fstatat(control, "cancel", &cancel, AT_SYMLINK_NOFOLLOW) == 0 && S_ISREG(cancel.st_mode) &&
                cancel.st_uid == geteuid() && cancel.st_nlink == 1 && cancel.st_size <= 32 &&
                ((cancel.st_mode & 07777) == 0600 || (cancel.st_mode & 07777) == 0660)) {
                cancellation = true;
                unlinkat(control, "cancel", 0);
            }
        }
        if (cancellation || std::chrono::steady_clock::now() >= deadline) {
            fprintf(report, "stop_reason=%s\n", cancellation ? "cancelled" : "timeout"); fflush(report);
            kill(-child, SIGTERM); // Only the new process group created by this launcher.
            usleep(250000);
            do { waited = waitpid(child, &status, WNOHANG); } while (waited < 0 && errno == EINTR);
            if (waited == 0) {
                kill(-child, SIGKILL);
                do { waited = waitpid(child, &status, 0); } while (waited < 0 && errno == EINTR);
            }
            stopped = true;
            break;
        }
        usleep(50000);
    }
    if (waited < 0) fprintf(report, "wait_errno=%d\n", errno);
    else fprintf(report, "wait_pid=%d exit_code=%d signal=%d raw_wait_status=%d core_dumped=%d stopped=%d\n", waited,
        WIFEXITED(status) ? WEXITSTATUS(status) : -1, WIFSIGNALED(status) ? WTERMSIG(status) : 0, status,
        WIFSIGNALED(status) && WCOREDUMP(status) ? 1 : 0, stopped ? 1 : 0);
    if (control >= 0) close(control);
    fprintf(report, "launch_complete=yes\n"); fclose(report);
}

static void Complete(napi_env env, napi_status status, void *data) {
    Work *work = static_cast<Work *>(data);
    napi_value value;
    const std::string result = status == napi_ok ? work->result : "async_work_failed";
    napi_create_string_utf8(env, result.c_str(), result.size(), &value);
    napi_resolve_deferred(env, work->deferred, value);
    napi_delete_async_work(env, work->work);
    delete work;
    active = false;
}

static napi_value QueueAction(napi_env env, const char *action) {
    RefreshCodexTerminal();
    const char *allowed[] = {"prepare", "import", "version", "doctor", "greeting", "tools", "verify-install"};
    bool allowedAction = false;
    for (const char *candidate : allowed) if (strcmp(action, candidate) == 0) allowedAction = true;
    if (!allowedAction || active.exchange(true)) { napi_throw_error(env, nullptr, "Unknown action or active child"); return nullptr; }
    cancellation = false;
    Work *work = new Work;
    work->action = action;
    napi_value promise, label;
    if (napi_create_promise(env, &work->deferred, &promise) != napi_ok ||
        napi_create_string_utf8(env, "codexHnpLaunch", NAPI_AUTO_LENGTH, &label) != napi_ok ||
        napi_create_async_work(env, nullptr, label, Execute, Complete, work, &work->work) != napi_ok ||
        napi_queue_async_work(env, work->work) != napi_ok) {
        if (work->work) napi_delete_async_work(env, work->work);
        delete work; active = false; napi_throw_error(env, nullptr, "Cannot queue launch"); return nullptr;
    }
    return promise;
}

napi_value RunCodex(napi_env env, napi_callback_info info) {
    size_t argc = 2;
    napi_value argv[2];
    char directory[256], action[32]; size_t dirLength = 0, actionLength = 0;
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 2 ||
        napi_get_value_string_utf8(env, argv[0], directory, sizeof(directory), &dirLength) != napi_ok ||
        napi_get_value_string_utf8(env, argv[1], action, sizeof(action), &actionLength) != napi_ok ||
        dirLength >= sizeof(directory)-1 || actionLength >= sizeof(action)-1 || strcmp(directory, kFiles) != 0) {
        napi_throw_type_error(env, nullptr, "Invalid application Context or action"); return nullptr;
    }
    return QueueAction(env, action);
}

napi_value RunRequestedCodex(napi_env env, napi_callback_info info) {
    size_t argc = 1; napi_value argv[1]; char directory[256]; size_t length = 0;
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 1 ||
        napi_get_value_string_utf8(env, argv[0], directory, sizeof(directory), &length) != napi_ok ||
        length >= sizeof(directory)-1 || strcmp(directory, kFiles) != 0) {
        napi_throw_type_error(env, nullptr, "Invalid Context"); return nullptr;
    }
    int files = PrepareFiles();
    if (files < 0) { napi_throw_error(env, nullptr, "Cannot prepare protected Context"); return nullptr; }
    int control = Directory(files, "control"); close(files);
    if (control < 0) { napi_throw_error(env, nullptr, "Cannot open protected control directory"); return nullptr; }
    char action[33] = "prepare";
    int input = openat(control, "action.txt", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
    if (input >= 0) {
        struct stat st = {};
        bool valid = fstat(input, &st) == 0 && S_ISREG(st.st_mode) && st.st_uid == geteuid() && st.st_nlink == 1 &&
                     st.st_size > 0 && st.st_size < 32 && ((st.st_mode & 07777) == 0600 || (st.st_mode & 07777) == 0660);
        ssize_t count = valid ? read(input, action, 32) : -1;
        close(input);
        if (!valid || count != st.st_size) { close(control); napi_throw_error(env, nullptr, "Invalid action file"); return nullptr; }
        action[count] = 0; action[strcspn(action, "\r\n")] = 0;
        if (unlinkat(control, "action.txt", 0) != 0) { close(control); napi_throw_error(env, nullptr, "Cannot consume action"); return nullptr; }
    } else if (errno != ENOENT) { close(control); napi_throw_error(env, nullptr, "Cannot read action"); return nullptr; }
    close(control);
    return QueueAction(env, action);
}

napi_value CancelCodex(napi_env env, napi_callback_info) {
    cancellation = true;
    napi_value result;
    napi_get_boolean(env, active.load(), &result);
    return result;
}

void ReleaseCodexTerminal() {
    if (terminalReserved.exchange(false)) active = false;
}

void RefreshCodexTerminal() {
    if (terminalReserved && codex_hnp::PtyStatus().find("\"running\":false") != std::string::npos) ReleaseCodexTerminal();
}

std::string PrepareCodexTerminal() {
    RefreshCodexTerminal();
    if (active.exchange(true)) return "active_child";
    terminalReserved = true;
    int files = PrepareFiles();
    if (files < 0) { ReleaseCodexTerminal(); return "private_directories"; }
    int logs = Directory(files, "logs");
    if (logs < 0) { close(files); ReleaseCodexTerminal(); return "private_logs"; }
    std::string name = "terminal-prepare-" + std::to_string(getpid()) + "-" + std::to_string(++sequence) + ".status.txt";
    int fd = openat(logs, name.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
    close(logs);
    if (fd < 0) { close(files); ReleaseCodexTerminal(); return "private_report"; }
    FILE *report = fdopen(fd, "w");
    if (!report) { close(fd); close(files); ReleaseCodexTerminal(); return "private_report"; }
    int imported = ImportPrivateConfig(files, report);
    fprintf(report, "terminal_config_status=%d\n", imported);
    fclose(report); close(files);
    if (imported <= 0) { ReleaseCodexTerminal(); return "private_config_required"; }
    return "";
}
