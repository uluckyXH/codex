#include <napi/native_api.h>
#include <arpa/inet.h>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <netdb.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <string>
#include <sys/socket.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>

extern char **environ;
struct ProbeWork {
    napi_async_work work = nullptr;
    napi_deferred deferred = nullptr;
    std::string directory;
    std::string result;
};

static void Log(FILE *file, const char *format, long value) {
    fprintf(file, format, value);
    fflush(file);
}

static void RunNetwork(FILE *file) {
    // Only this fixed public official endpoint is tested; no credentials or config.
    fprintf(file, "network_host=developer.huawei.com port=443\n");
    fflush(file);
    struct addrinfo hints = {};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    struct addrinfo *addresses = nullptr;
    int status = getaddrinfo("developer.huawei.com", "443", &hints, &addresses);
    Log(file, "dns_result=%ld\n", status);
    if (status != 0) return;
    int count = 0, succeeded = 0;
    for (struct addrinfo *address = addresses; address && count < 6; address = address->ai_next) {
        ++count;
        int socketFd = socket(address->ai_family, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
        if (socketFd < 0) {
            fprintf(file, "family=%d socket_errno=%d\n", address->ai_family, errno);
            fflush(file);
            continue;
        }
        int error = 0;
        if (connect(socketFd, address->ai_addr, address->ai_addrlen) != 0) {
            error = errno;
            if (error == EINPROGRESS) {
                struct pollfd waiter = {socketFd, POLLOUT, 0};
                int ready;
                do { ready = poll(&waiter, 1, 3500); } while (ready < 0 && errno == EINTR);
                if (ready > 0) {
                    socklen_t length = sizeof(error);
                    if (getsockopt(socketFd, SOL_SOCKET, SO_ERROR, &error, &length) != 0) error = errno;
                } else error = ready == 0 ? ETIMEDOUT : errno;
            }
        }
        fprintf(file, "family=%d tcp_errno=%d\n", address->ai_family, error);
        fflush(file);
        if (!error) ++succeeded;
        close(socketFd);
    }
    freeaddrinfo(addresses);
    fprintf(file, "addresses_tested=%d tcp_succeeded=%d\n", count, succeeded);
    fflush(file);
}

static void RunSpawn(FILE *file, const std::string &directory) {
    std::string output = directory + "/spawn-output.txt";
    int outputFd = open(output.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (outputFd < 0) { Log(file, "spawn_output_open_errno=%ld\n", errno); return; }
    posix_spawn_file_actions_t actions;
    int result = posix_spawn_file_actions_init(&actions);
    if (result != 0) { close(outputFd); Log(file, "spawn_actions_errno=%ld\n", result); return; }
    result = posix_spawn_file_actions_adddup2(&actions, outputFd, STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, outputFd, STDERR_FILENO);
    if (result != 0) {
        posix_spawn_file_actions_destroy(&actions); close(outputFd);
        Log(file, "spawn_actions_errno=%ld\n", result); return;
    }
    char shell[] = "/system/bin/sh";
    char option[] = "-c";
    char command[] = "echo codex-host-spawn-ok";
    char *arguments[] = {shell, option, command, nullptr};
    pid_t child = -1;
    fprintf(file, "spawn_phase=calling_posix_spawn\n");
    fflush(file);
    result = posix_spawn(&child, shell, &actions, nullptr, arguments, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(outputFd);
    Log(file, "posix_spawn_result=%ld\n", result);
    if (result != 0) return;
    int childStatus = 0;
    pid_t waited = 0;
    for (int i = 0; i < 100 && waited == 0; ++i) {
        waited = waitpid(child, &childStatus, WNOHANG);
        if (waited < 0 && errno == EINTR) { waited = 0; continue; }
        if (waited == 0) usleep(50000);
    }
    if (waited == 0) {
        fprintf(file, "spawn_timeout=yes\n"); fflush(file);
        kill(child, SIGKILL); // Only the harmless child created by this probe.
        do { waited = waitpid(child, &childStatus, 0); } while (waited < 0 && errno == EINTR);
    }
    if (waited < 0) { Log(file, "spawn_wait_errno=%ld\n", errno); return; }
    fprintf(file, "spawn_exit=%d spawn_signal=%d raw_wait_status=%d\n",
            WIFEXITED(childStatus) ? WEXITSTATUS(childStatus) : -1,
            WIFSIGNALED(childStatus) ? WTERMSIG(childStatus) : 0, childStatus);
    fflush(file);
}

static void RunStandalone(FILE *file, const std::string &directory) {
    std::string executable = directory + "/network-probe";
    std::string endpoint = directory + "/public-endpoint.txt";
    std::string output = directory + "/standalone-output.txt";
    // This test does not accept a caller-controlled host, payload, or executable.
    FILE *input = fopen(endpoint.c_str(), "w");
    if (!input) { Log(file, "standalone_endpoint_errno=%ld\n", errno); return; }
    fprintf(input, "developer.huawei.com\n443\n");
    fclose(input);
    if (chmod(executable.c_str(), 0700) != 0) {
        Log(file, "standalone_chmod_errno=%ld\n", errno); return;
    }
    int outputFd = open(output.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (outputFd < 0) { Log(file, "standalone_output_errno=%ld\n", errno); return; }
    posix_spawn_file_actions_t actions;
    int result = posix_spawn_file_actions_init(&actions);
    if (result != 0) { close(outputFd); Log(file, "standalone_actions_errno=%ld\n", result); return; }
    result = posix_spawn_file_actions_adddup2(&actions, outputFd, STDOUT_FILENO);
    if (!result) result = posix_spawn_file_actions_adddup2(&actions, outputFd, STDERR_FILENO);
    if (result != 0) {
        posix_spawn_file_actions_destroy(&actions); close(outputFd);
        Log(file, "standalone_actions_errno=%ld\n", result); return;
    }
    char *arguments[] = {executable.data(), endpoint.data(), nullptr};
    pid_t child = -1;
    fprintf(file, "standalone_phase=calling_posix_spawn\n");
    fflush(file);
    result = posix_spawn(&child, executable.c_str(), &actions, nullptr, arguments, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(outputFd);
    Log(file, "standalone_posix_spawn_result=%ld\n", result);
    if (result != 0) return;
    int childStatus = 0;
    pid_t waited = 0;
    for (int i = 0; i < 700 && waited == 0; ++i) {
        waited = waitpid(child, &childStatus, WNOHANG);
        if (waited < 0 && errno == EINTR) { waited = 0; continue; }
        if (waited == 0) usleep(50000);
    }
    if (waited == 0) {
        fprintf(file, "standalone_timeout=yes\n"); fflush(file);
        kill(child, SIGKILL); // Only this isolated fixed-domain probe child.
        do { waited = waitpid(child, &childStatus, 0); } while (waited < 0 && errno == EINTR);
    }
    if (waited < 0) { Log(file, "standalone_wait_errno=%ld\n", errno); return; }
    fprintf(file, "standalone_exit=%d standalone_signal=%d raw_wait_status=%d\n",
            WIFEXITED(childStatus) ? WEXITSTATUS(childStatus) : -1,
            WIFSIGNALED(childStatus) ? WTERMSIG(childStatus) : 0, childStatus);
    fflush(file);
}

static void Execute(napi_env, void *data) {
    auto *probe = static_cast<ProbeWork *>(data);
    std::string path = probe->directory + "/host-probe.txt";
    FILE *file = fopen(path.c_str(), "w");
    if (!file) { probe->result = "report_open_errno=" + std::to_string(errno); return; }
    fcntl(fileno(file), F_SETFD, FD_CLOEXEC);
    fprintf(file, "uid=%u gid=%u pid=%d\ncontext_files_dir=%s\n", getuid(), getgid(), getpid(), probe->directory.c_str());
    fflush(file);
    RunNetwork(file);
    RunSpawn(file, probe->directory);
    RunStandalone(file, probe->directory);
    fprintf(file, "probe_complete=yes\n");
    fclose(file);
    probe->result = path;
}

static void Complete(napi_env env, napi_status status, void *data) {
    auto *probe = static_cast<ProbeWork *>(data);
    napi_value result;
    std::string message = status == napi_ok ? probe->result : "async_work_failed";
    napi_create_string_utf8(env, message.c_str(), message.size(), &result);
    napi_resolve_deferred(env, probe->deferred, result);
    napi_delete_async_work(env, probe->work);
    delete probe;
}

static napi_value RunProbe(napi_env env, napi_callback_info info) {
    size_t argc = 1;
    napi_value argv[1];
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 1) {
        napi_throw_type_error(env, nullptr, "Expected Context.filesDir"); return nullptr;
    }
    char directory[4096];
    size_t length = 0;
    if (napi_get_value_string_utf8(env, argv[0], directory, sizeof(directory), &length) != napi_ok ||
        length == 0 || length >= sizeof(directory) - 1 || directory[0] != '/') {
        napi_throw_type_error(env, nullptr, "Invalid Context.filesDir"); return nullptr;
    }
    auto *probe = new ProbeWork;
    probe->directory.assign(directory, length);
    napi_value promise, label;
    if (napi_create_promise(env, &probe->deferred, &promise) != napi_ok ||
        napi_create_string_utf8(env, "hostCapabilityProbe", NAPI_AUTO_LENGTH, &label) != napi_ok ||
        napi_create_async_work(env, nullptr, label, Execute, Complete, probe, &probe->work) != napi_ok) {
        delete probe; napi_throw_error(env, nullptr, "Cannot create native probe work"); return nullptr;
    }
    if (napi_queue_async_work(env, probe->work) != napi_ok) {
        napi_delete_async_work(env, probe->work); delete probe;
        napi_throw_error(env, nullptr, "Cannot queue native probe work"); return nullptr;
    }
    return promise;
}

static napi_value Init(napi_env env, napi_value exports) {
    napi_property_descriptor descriptor = {"runProbe", nullptr, RunProbe, nullptr, nullptr, nullptr, napi_default, nullptr};
    napi_define_properties(env, exports, 1, &descriptor);
    return exports;
}
static napi_module module = {1, 0, nullptr, Init, "hostprobe", nullptr, {0}};
extern "C" __attribute__((constructor)) void RegisterHostProbe() { napi_module_register(&module); }
