#include "codex_launcher.h"
#include "host_layout.h"
#include "native_package.h"
#include "owned_processes.h"
#include "package_verification.h"
#include "secure_config.h"
#include "tool_regression.h"
#include <atomic>
#include <cerrno>
#include <chrono>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <signal.h>
#include <spawn.h>
#include <string>
#include <sys/stat.h>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>
#include <vector>

using namespace codex_hnp;
namespace {
std::atomic<bool> active{false}, cancellation{false};
std::atomic<unsigned> sequence{0};
struct Work {
    napi_async_work work = nullptr;
    napi_deferred deferred = nullptr;
    std::string files, action, result;
    std::unique_ptr<HostLayout> layout;
    bool requested = false;
    bool cleanupUnknown = false;
};

bool Allowed(const std::string &action) {
    for (const char *candidate : {"prepare", "secure-context", "import", "version", "doctor", "greeting", "tools", "verify-install"})
        if (action == candidate) return true;
    return false;
}
bool NapiText(napi_env env, napi_value value, size_t limit, std::string &text) {
    size_t length = 0, copied = 0;
    if (napi_get_value_string_utf8(env, value, nullptr, 0, &length) != napi_ok || !length || length > limit) return false;
    std::vector<char> bytes(length + 1);
    if (napi_get_value_string_utf8(env, value, bytes.data(), bytes.size(), &copied) != napi_ok || copied != length) return false;
    text.assign(bytes.data(), copied); return text.find('\0') == std::string::npos;
}
bool RegularPrivate(int fd, struct stat &st, bool handoff = false) {
    return fstat(fd, &st) == 0 && S_ISREG(st.st_mode) && st.st_uid == geteuid() && st.st_nlink == 1 &&
        ((st.st_mode & 07777) == 0600 || (handoff && (st.st_mode & 07777) == 0660));
}
bool ConsumeAction(HostLayout &layout, std::string &action, std::string &error) {
    int control = layout.Fd("host/control");
    Descriptor input(openat(control, "action.txt", O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK));
    if (input.get() < 0) {
        if (errno == ENOENT) { action = "prepare"; return true; }
        error = "action_open"; return false;
    }
    struct stat st = {}, named = {}; char bytes[33] = {};
    if (!RegularPrivate(input.get(), st, true) || st.st_size <= 0 || st.st_size >= 32) { error = "action_metadata"; return false; }
    ssize_t count = read(input.get(), bytes, sizeof(bytes));
    if (count != st.st_size) { error = "action_read"; return false; }
    action.assign(bytes, static_cast<size_t>(count));
    while (!action.empty() && (action.back() == '\n' || action.back() == '\r')) action.pop_back();
    if (!Allowed(action) || !layout.Revalidate(error) || fstatat(control, "action.txt", &named, AT_SYMLINK_NOFOLLOW) != 0 ||
        !SameObject(st, named) || unlinkat(control, "action.txt", 0) != 0) { error = "action_rejected_or_replaced"; return false; }
    return true;
}
bool WriteCompletion(Work &work) {
    if (!work.layout) return false;
    std::string error;
    if (!work.layout->Revalidate(error)) return false;
    int host = work.layout->Fd("host");
    struct stat previous = {};
    int existing = fstatat(host, "launch-completion.txt", &previous, AT_SYMLINK_NOFOLLOW);
    if (existing == 0 && (!S_ISREG(previous.st_mode) || previous.st_uid != geteuid() ||
        previous.st_nlink != 1 || (previous.st_mode & 07777) != 0600)) return false;
    if (existing != 0 && errno != ENOENT) return false;
    std::string temporary = ".completion-" + std::to_string(getpid()) + "-" + std::to_string(++sequence);
    Descriptor output(openat(host, temporary.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
    if (output.get() < 0) return false;
    size_t offset = 0;
    while (offset < work.result.size()) {
        ssize_t count = write(output.get(), work.result.data() + offset, work.result.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) { unlinkat(host, temporary.c_str(), 0); return false; }
        offset += static_cast<size_t>(count);
    }
    bool saved = fsync(output.get()) == 0 && work.layout->Revalidate(error) &&
                 renameat(host, temporary.c_str(), host, "launch-completion.txt") == 0 && fsync(host) == 0;
    if (!saved) unlinkat(host, temporary.c_str(), 0);
    return saved;
}
bool CancellationFile(HostLayout &layout) {
    int control = layout.Fd("host/control");
    Descriptor file(openat(control, "cancel", O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK));
    struct stat st = {}, named = {};
    if (file.get() < 0 || !RegularPrivate(file.get(), st, true) || st.st_size > 32 ||
        fstatat(control, "cancel", &named, AT_SYMLINK_NOFOLLOW) != 0 || !SameObject(st, named)) return false;
    return unlinkat(control, "cancel", 0) == 0;
}
void RunChild(Work &work, FILE *report, int output, int error, int input) {
    std::string workspace = work.layout->Root() + "/workspace";
    std::vector<std::string> args = {"/system/bin/sh", "-c", "umask 077; cd \"$1\" || exit 125; shift; exec \"$@\"",
                                    "codex-hnp", workspace, std::string(CODEX_HNP_PACKAGE_PATH) + "/bin/codex"};
    bool greeting = work.action == "greeting";
    if (work.action == "version") args.push_back("--version");
    else if (work.action == "doctor") args.insert(args.end(), {"doctor", "--capabilities", "--json"});
    else args.insert(args.end(), {"exec", "--json", "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
        "--model", "gpt-5.6-terra", "--cd", workspace, "--disable", "unbounded_connection_retries",
        "-c", "analytics.enabled=false", "-c", "model_providers.proxy.request_max_retries=0",
        "-c", "model_providers.proxy.stream_max_retries=0", "-c", "approval_policy=\"never\"", "-c", "web_search=\"disabled\"",
        "只回复：你好。不要调用任何工具，不要读取或修改文件。"});
    auto values = HostEnvironment(work.layout->Root(), "dumb");
    auto argv = ArgumentPointers(args); auto envp = ArgumentPointers(values);
    posix_spawn_file_actions_t actions; posix_spawnattr_t attributes;
    int result = posix_spawn_file_actions_init(&actions); bool actionsReady = result == 0, attributesReady = false;
    if (!result) { result = posix_spawnattr_init(&attributes); attributesReady = result == 0; }
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, input, STDIN_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, output, STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, error, STDERR_FILENO);
    if (!result) result = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSID);
    std::string failure;
    if (!result && !work.layout->Revalidate(failure)) result = errno;
    pid_t child = -1;
    if (!result) result = posix_spawn(&child, "/system/bin/sh", &actions, &attributes, argv.data(), envp.data());
    if (attributesReady) posix_spawnattr_destroy(&attributes);
    if (actionsReady) posix_spawn_file_actions_destroy(&actions);
    fprintf(report, "spawn_result=%d child_pid=%d\n", result, child); fflush(report);
    if (result) { fprintf(report, "launch_complete=no spawn_failed=yes\n"); return; }
    OwnedProcesses owned;
    bool tracked = owned.Begin(child, failure), stopping = false, unknown = !tracked;
    auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(greeting ? 180 : 30);
    auto stopAt = deadline, nextSignal = deadline;
    for (;;) {
        siginfo_t info = {};
        int waitResult = waitid(P_PID, static_cast<id_t>(child), &info, WEXITED | WNOHANG | WNOWAIT);
        if (waitResult != 0 && errno != EINTR) {
            fprintf(report, "waitid_errno=%d launch_complete=no cleanup_unverified=yes\n", errno);
            work.cleanupUnknown = true; return;
        }
        bool exited = waitResult == 0 && info.si_pid == child;
        if (tracked) unknown = !owned.Observe(failure);
        cancellation = cancellation || CancellationFile(*work.layout);
        auto now = std::chrono::steady_clock::now();
        if (!stopping && (exited || cancellation || now >= deadline || !tracked)) {
            stopping = true; stopAt = now; nextSignal = now + std::chrono::seconds(1);
            int signalError = 0;
            if (tracked) owned.Signal(SIGTERM, failure, signalError);
            else SignalDirectChild(child, SIGTERM, failure);
            fprintf(report, "stop_reason=%s signal_errno=%d\n", exited ? "leader_exit_cleanup" : cancellation ? "cancelled" : "timeout_or_tracking", signalError);
        }
        if (stopping && now >= nextSignal) {
            int signalError = 0;
            if (tracked) owned.Signal(SIGKILL, failure, signalError);
            else SignalDirectChild(child, SIGKILL, failure);
            fprintf(report, "kill_errno=%d survivors=%d\n", signalError, owned.Alive());
            nextSignal = now + std::chrono::milliseconds(250);
        }
        if (exited && tracked && !unknown && owned.Alive() == 0) {
            int status = 0; pid_t waited = waitpid(child, &status, WNOHANG);
            if (waited == child) {
                fprintf(report, "wait_pid=%d exit_code=%d signal=%d raw_wait_status=%d stopped=%d cleanup_verified=yes\n", waited,
                    WIFEXITED(status) ? WEXITSTATUS(status) : -1, WIFSIGNALED(status) ? WTERMSIG(status) : 0, status,
                    cancellation || now >= deadline ? 1 : 0);
                fprintf(report, "launch_complete=yes\n"); return;
            }
            if (waited < 0 && errno != EINTR) {
                fprintf(report, "wait_errno=%d launch_complete=no cleanup_unverified=yes\n", errno);
                work.cleanupUnknown = true; return;
            }
        }
        if (stopping && now - stopAt > std::chrono::seconds(5)) {
            fprintf(report, "launch_complete=no cleanup_unverified=yes survivors=%d tracking_errno=%d\n", owned.Alive(), unknown ? errno : 0);
            work.cleanupUnknown = true; return;
        }
        fflush(report); std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
}
void Execute(napi_env, void *data) {
    Work &work = *static_cast<Work *>(data);
    std::string evidence, failure;
    if (work.action == "secure-context" && !SecureApplicationContext(work.files, evidence, failure)) {
        work.result = "error: " + failure + "\n" + evidence; return;
    }
    work.layout = std::make_unique<HostLayout>();
    if (!work.layout->Open(work.files, true, failure)) {
        work.cleanupUnknown = work.layout->InitializationUnverified();
        work.layout.reset(); work.result = "error: " + failure + "\n" + evidence; return;
    }
    if (work.requested && !ConsumeAction(*work.layout, work.action, failure)) {
        work.result = "error: " + failure; WriteCompletion(work); return;
    }
    int logs = work.layout->Fd("logs");
    std::string name = "codex-" + work.action + "-" + std::to_string(getpid()) + "-" + std::to_string(++sequence);
    std::string reportName = name + ".status.txt";
    Descriptor reportFd(openat(logs, reportName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
    if (reportFd.get() < 0) { work.result = "error: report_open errno=" + std::to_string(errno); return; }
    FILE *report = fdopen(reportFd.release(), "w");
    if (!report) { work.result = "error: report_fdopen"; return; }
    work.result = work.layout->Root() + "/logs/" + reportName;
    fprintf(report, "action=%s uid=%u euid=%u gid=%u egid=%u source=native-application-context\nfiles=%s data=%s\n",
        work.action.c_str(), getuid(), geteuid(), getgid(), getegid(), work.files.c_str(), work.layout->Root().c_str());
    if (!evidence.empty()) fprintf(report, "%s", evidence.c_str());
    if (work.action == "prepare" || work.action == "secure-context") {
        fprintf(report, "shared_cli_initialization=yes shell_config_required=no launch_complete=no_child\n");
    } else if (work.action == "import") {
        int imported = ImportPrivateConfig(work.layout->Fd(""), report);
        fprintf(report, "launch_complete=no_child config_status=%d\n", imported);
    } else if (work.action == "verify-install") {
        bool passed = ExportInstalledPackageForVerification(work.layout->Fd("host"), work.layout->Root() + "/host", report, name);
        fprintf(report, "verification_export_complete=yes passed=%d\n", passed ? 1 : 0);
    } else if (work.action == "tools") {
        bool passed = RunToolRegression(work.layout->Fd(""), work.layout->Root(), logs, report, name, cancellation, work.cleanupUnknown);
        fprintf(report, "tool_regression_complete=yes passed=%d\n", passed ? 1 : 0);
    } else {
        std::string outputName = name + ".stdout.txt", errorName = name + ".stderr.txt";
        Descriptor output(openat(logs, outputName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
        Descriptor error(openat(logs, errorName.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600));
        Descriptor input(open("/dev/null", O_RDONLY | O_CLOEXEC));
        fprintf(report, "stdout=%s/logs/%s\nstderr=%s/logs/%s\n", work.layout->Root().c_str(), outputName.c_str(),
                work.layout->Root().c_str(), errorName.c_str());
        if (output.get() < 0 || error.get() < 0 || input.get() < 0) fprintf(report, "stdio_errno=%d launch_complete=no\n", errno);
        else RunChild(work, report, output.get(), error.get(), input.get());
    }
    fclose(report);
    if (!WriteCompletion(work)) work.result = "error: completion_write errno=" + std::to_string(errno);
}
void Complete(napi_env env, napi_status status, void *data) {
    Work *work = static_cast<Work *>(data); napi_value value;
    std::string result = status == napi_ok ? work->result : "error: async_work_failed";
    napi_create_string_utf8(env, result.c_str(), result.size(), &value);
    napi_resolve_deferred(env, work->deferred, value); napi_delete_async_work(env, work->work);
    // An unverified live child blocks subsequent diagnostics; it is not marked
    // complete by clearing the reservation. Independent PTYs are unaffected.
    active = work->cleanupUnknown; delete work;
}
napi_value QueueAction(napi_env env, const std::string &files, const std::string &action, bool requested = false) {
    if (!Allowed(action) || active.exchange(true)) { napi_throw_error(env, nullptr, "Unknown action or active/unverified diagnostic child"); return nullptr; }
    std::string error;
    if (!VerifyApplicationContext(files, error)) { active = false; napi_throw_error(env, nullptr, error.c_str()); return nullptr; }
    cancellation = false; Work *work = new Work;
    work->files = files; work->action = action; work->requested = requested;
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
}
napi_value RunCodex(napi_env env, napi_callback_info info) {
    size_t argc = 2; napi_value argv[2]; std::string directory, action;
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 2 ||
        !NapiText(env, argv[0], 4095, directory) || !NapiText(env, argv[1], 32, action)) {
        napi_throw_type_error(env, nullptr, "Invalid Context or action"); return nullptr;
    }
    return QueueAction(env, directory, action);
}
napi_value RunRequestedCodex(napi_env env, napi_callback_info info) {
    size_t argc = 1; napi_value argv[1]; std::string directory;
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 1 || !NapiText(env, argv[0], 4095, directory)) {
        napi_throw_type_error(env, nullptr, "Invalid Context"); return nullptr;
    }
    return QueueAction(env, directory, "prepare", true);
}
napi_value CancelCodex(napi_env env, napi_callback_info) {
    cancellation = true; napi_value result; napi_get_boolean(env, active.load(), &result); return result;
}
