#include <AbilityKit/native_child_process.h>
#include <cerrno>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fcntl.h>
#include <netdb.h>
#include <poll.h>
#include <signal.h>
#include <spawn.h>
#include <string>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <termios.h>
#include <unistd.h>

extern char **environ;

static void RunNetwork(FILE *report) {
    fprintf(report, "network_host=developer.huawei.com port=443\n");
    fflush(report);
    addrinfo hints = {};
    hints.ai_family = AF_UNSPEC;
    hints.ai_socktype = SOCK_STREAM;
    addrinfo *addresses = nullptr;
    int status = getaddrinfo("developer.huawei.com", "443", &hints, &addresses);
    fprintf(report, "dns_result=%d\n", status);
    fflush(report);
    if (status != 0) return;
    int count = 0, succeeded = 0;
    for (auto *address = addresses; address && count < 6; address = address->ai_next) {
        ++count;
        int fd = socket(address->ai_family, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
        if (fd < 0) { fprintf(report, "socket_errno=%d\n", errno); fflush(report); continue; }
        int error = 0;
        if (connect(fd, address->ai_addr, address->ai_addrlen) != 0) {
            error = errno;
            if (error == EINPROGRESS) {
                pollfd waiter = {fd, POLLOUT, 0};
                int ready = poll(&waiter, 1, 3500);
                if (ready > 0) {
                    socklen_t size = sizeof(error);
                    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size) != 0) error = errno;
                } else error = ready == 0 ? ETIMEDOUT : errno;
            }
        }
        fprintf(report, "family=%d tcp_errno=%d\n", address->ai_family, error);
        fflush(report);
        if (error == 0) ++succeeded;
        close(fd);
    }
    freeaddrinfo(addresses);
    fprintf(report, "addresses_tested=%d tcp_succeeded=%d\n", count, succeeded);
    fflush(report);
}

static void RunFile(FILE *report, const std::string &directory) {
    const std::string path = directory + "/native-child-file.txt";
    const char payload[] = "native-child-file-ok\n";
    fprintf(report, "private_file_phase=open\n"); fflush(report);
    int fd = open(path.c_str(), O_CREAT | O_RDWR | O_TRUNC | O_CLOEXEC, 0600);
    if (fd < 0) { fprintf(report, "private_file_open_errno=%d\n", errno); fflush(report); return; }
    ssize_t written = write(fd, payload, sizeof(payload) - 1);
    int syncResult = fsync(fd);
    int syncError = syncResult == 0 ? 0 : errno;
    off_t offset = lseek(fd, 0, SEEK_SET);
    char data[64] = {};
    ssize_t received = read(fd, data, sizeof(data));
    struct stat information = {};
    int statResult = fstat(fd, &information);
    fprintf(report, "private_file_written=%zd sync_errno=%d seek=%lld read=%zd matches=%d\n",
            written, syncError, static_cast<long long>(offset), received,
            received == sizeof(payload) - 1 && memcmp(data, payload, sizeof(payload) - 1) == 0);
    if (statResult == 0) fprintf(report, "private_file_uid=%u mode=%o\n", information.st_uid, information.st_mode & 0777);
    close(fd);
    fflush(report);
}

static void RunHandoffFixture(FILE *report, const std::string &directory) {
    // A first run prepares a private inbox. Only the exact public fake fixture is accepted.
    std::string privateDir = directory + "/handoff-private";
    if (mkdir(privateDir.c_str(), 0700) != 0 && errno != EEXIST) {
        fprintf(report, "handoff_mkdir_errno=%d\n", errno); fflush(report); return;
    }
    int dir = open(privateDir.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (dir < 0) { fprintf(report, "handoff_dir_open_errno=%d\n", errno); fflush(report); return; }
    struct stat dirInfo = {};
    if (fstat(dir, &dirInfo) != 0 || dirInfo.st_uid != getuid() || (dirInfo.st_mode & 0777) != 0700) {
        fprintf(report, "handoff_private_dir_rejected=yes\n"); fflush(report); close(dir); return;
    }
    fprintf(report, "handoff_private_dir_uid=%u mode=%o\n", dirInfo.st_uid, dirInfo.st_mode & 0777);
    const char expected[] = "probe_only = true\nmodel = \"gpt-5.6-terra\"\n";
    int source = openat(dir, "incoming-probe.toml", O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (source < 0) { fprintf(report, "handoff_source_errno=%d\n", errno); fflush(report); close(dir); return; }
    struct stat sourceInfo = {};
    int sourceStat = fstat(source, &sourceInfo);
    char content[sizeof(expected)] = {};
    ssize_t count = read(source, content, sizeof(content));
    bool matches = count == sizeof(expected) - 1 && memcmp(content, expected, sizeof(expected) - 1) == 0;
    fprintf(report, "handoff_source_stat=%d uid=%u mode=%o size=%lld expected_matches=%d\n",
            sourceStat, sourceInfo.st_uid, sourceInfo.st_mode & 0777,
            static_cast<long long>(sourceInfo.st_size), matches);
    close(source);
    if (sourceStat != 0 || sourceInfo.st_uid != getuid() || !S_ISREG(sourceInfo.st_mode) || !matches) {
        fprintf(report, "handoff_fixture_rejected=yes\n"); fflush(report); close(dir); return;
    }
    int output = openat(dir, "fake-config-private.toml", O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW | O_CLOEXEC, 0600);
    if (output < 0) { fprintf(report, "handoff_output_errno=%d\n", errno); fflush(report); close(dir); return; }
    ssize_t written = write(output, content, count);
    int syncResult = fsync(output);
    struct stat outputInfo = {};
    int outputStat = fstat(output, &outputInfo);
    fprintf(report, "handoff_output_written=%zd fsync=%d stat=%d uid=%u mode=%o\n",
            written, syncResult, outputStat, outputInfo.st_uid, outputInfo.st_mode & 0777);
    close(output); close(dir); fflush(report);
}

static void RunPipe(FILE *report) {
    int pipes[2];
    fprintf(report, "pipe_phase=create\n"); fflush(report);
    if (pipe(pipes) != 0) { fprintf(report, "pipe_errno=%d\n", errno); fflush(report); return; }
    const char payload[] = "native-pipe-ok";
    ssize_t written = write(pipes[1], payload, sizeof(payload));
    close(pipes[1]);
    char data[64] = {};
    ssize_t received = read(pipes[0], data, sizeof(data));
    close(pipes[0]);
    fprintf(report, "pipe_written=%zd read=%zd matches=%d\n", written, received,
            received == sizeof(payload) && memcmp(data, payload, sizeof(payload)) == 0);
    fflush(report);
}

static void RunPty(FILE *report) {
    fprintf(report, "pty_phase=posix_openpt\n"); fflush(report);
    int master = posix_openpt(O_RDWR | O_NOCTTY | O_NONBLOCK | O_CLOEXEC);
    if (master < 0) { fprintf(report, "pty_open_errno=%d\n", errno); fflush(report); return; }
    if (grantpt(master) != 0 || unlockpt(master) != 0) {
        fprintf(report, "pty_prepare_errno=%d\n", errno); fflush(report); close(master); return;
    }
    char *name = ptsname(master);
    if (!name) { fprintf(report, "pty_name_errno=%d\n", errno); fflush(report); close(master); return; }
    int slave = open(name, O_RDWR | O_NOCTTY | O_CLOEXEC);
    if (slave < 0) { fprintf(report, "pty_slave_errno=%d\n", errno); fflush(report); close(master); return; }
    struct termios settings = {};
    int terminalResult = tcgetattr(slave, &settings);
    int terminalError = terminalResult == 0 ? 0 : errno;
    const char payload[] = "native-pty-ok";
    ssize_t written = write(slave, payload, sizeof(payload) - 1);
    pollfd waiter = {master, POLLIN, 0};
    int ready = poll(&waiter, 1, 1000);
    char data[64] = {};
    ssize_t received = ready > 0 ? read(master, data, sizeof(data)) : -1;
    fprintf(report, "pty_master_tty=%d slave_tty=%d tcgetattr_errno=%d written=%zd read=%zd matches=%d\n",
            isatty(master), isatty(slave), terminalError, written, received,
            received == sizeof(payload) - 1 && memcmp(data, payload, sizeof(payload) - 1) == 0);
    close(slave); close(master); fflush(report);
}

static void RunShell(FILE *report, const std::string &directory) {
    std::string output = directory + "/native-child-shell.txt";
    int outputFd = open(output.c_str(), O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
    if (outputFd < 0) { fprintf(report, "shell_output_errno=%d\n", errno); fflush(report); return; }
    posix_spawn_file_actions_t actions;
    int status = posix_spawn_file_actions_init(&actions);
    if (status != 0) { close(outputFd); fprintf(report, "shell_actions_errno=%d\n", status); fflush(report); return; }
    status = posix_spawn_file_actions_adddup2(&actions, outputFd, STDOUT_FILENO);
    if (!status) status = posix_spawn_file_actions_adddup2(&actions, outputFd, STDERR_FILENO);
    if (status != 0) {
        posix_spawn_file_actions_destroy(&actions); close(outputFd);
        fprintf(report, "shell_actions_errno=%d\n", status); fflush(report); return;
    }
    char shell[] = "/system/bin/sh";
    char option[] = "-c";
    char command[] = "echo native-child-shell-ok";
    char *arguments[] = {shell, option, command, nullptr};
    pid_t child = -1;
    fprintf(report, "shell_phase=posix_spawn\n"); fflush(report);
    status = posix_spawn(&child, shell, &actions, nullptr, arguments, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(outputFd);
    fprintf(report, "shell_posix_spawn_result=%d\n", status); fflush(report);
    if (status != 0) return;
    int childStatus = 0;
    pid_t waited = 0;
    for (int i = 0; i < 100 && waited == 0; ++i) {
        waited = waitpid(child, &childStatus, WNOHANG);
        if (waited < 0 && errno == EINTR) { waited = 0; continue; }
        if (waited == 0) usleep(50000);
    }
    if (waited == 0) {
        fprintf(report, "shell_timeout=yes\n"); fflush(report);
        kill(child, SIGKILL);
        do { waited = waitpid(child, &childStatus, 0); } while (waited < 0 && errno == EINTR);
    }
    if (waited < 0) fprintf(report, "shell_wait_errno=%d\n", errno);
    else fprintf(report, "shell_exit=%d shell_signal=%d raw_wait_status=%d\n",
                 WIFEXITED(childStatus) ? WEXITSTATUS(childStatus) : -1,
                 WIFSIGNALED(childStatus) ? WTERMSIG(childStatus) : 0, childStatus);
    fflush(report);
}

extern "C" __attribute__((visibility("default"))) void ProbeMain(NativeChildProcess_Args args) {
    int reportFd = -1;
    for (auto *entry = args.fdList.head; entry; entry = entry->next) {
        if (entry->fdName && strcmp(entry->fdName, "report") == 0) reportFd = entry->fd;
    }
    if (reportFd < 0 || !args.entryParams || args.entryParams[0] != '/') return;
    int copy = dup(reportFd);
    if (copy < 0) return;
    FILE *report = fdopen(copy, "w");
    if (!report) { close(copy); return; }
    fcntl(copy, F_SETFD, FD_CLOEXEC);
    fprintf(report, "uid=%u gid=%u pid=%d parent_pid=%d\nisolation_mode=NORMAL\ncontext_files_dir=%s\n",
            getuid(), getgid(), getpid(), getppid(), args.entryParams);
    fflush(report);
    const std::string directory(args.entryParams);
    RunNetwork(report);
    RunFile(report, directory);
    RunHandoffFixture(report, directory);
    RunPipe(report);
    RunPty(report);
    RunShell(report, directory);
    for (int second = 1; second <= 3; ++second) {
        sleep(1);
        fprintf(report, "heartbeat_second=%d\n", second); fflush(report);
    }
    fprintf(report, "native_child_complete=yes\n");
    fclose(report);
}
