#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <pty.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

extern char **environ;
static long long now_ms(void) {
    struct timespec value;
    clock_gettime(CLOCK_MONOTONIC, &value);
    return (long long)value.tv_sec * 1000 + value.tv_nsec / 1000000;
}
static int same_mode(const struct termios *left, const struct termios *right) {
    return left->c_iflag == right->c_iflag && left->c_oflag == right->c_oflag &&
        left->c_cflag == right->c_cflag && left->c_lflag == right->c_lflag &&
        memcmp(left->c_cc, right->c_cc, NCCS) == 0 &&
        cfgetispeed(left) == cfgetispeed(right) && cfgetospeed(left) == cfgetospeed(right);
}
static int read_key(int fd, char expected) {
    struct pollfd waiter = {fd, POLLIN, 0};
    int result;
    do { result = poll(&waiter, 1, 3000); } while (result < 0 && errno == EINTR);
    char value = 0;
    return result > 0 && read(fd, &value, 1) == 1 && value == expected;
}
static int child_probe(void) {
    printf("CHILD_ID pid=%d sid=%d pgid=%d tty=%d,%d,%d\n", getpid(), getsid(0), getpgrp(), isatty(0), isatty(1), isatty(2));
    int controlling = open("/dev/tty", O_RDWR | O_CLOEXEC);
    if (controlling < 0) { printf("CHILD_CTTY errno=%d\n", errno); return 41; }
    struct termios original = {0}, raw = {0}, observed = {0};
    struct winsize size = {0};
    if (tcgetattr(controlling, &original) != 0) { printf("CHILD_TCGET errno=%d\n", errno); close(controlling); return 42; }
    raw = original; cfmakeraw(&raw);
    if (tcsetattr(controlling, TCSANOW, &raw) != 0) { printf("CHILD_RAW errno=%d\n", errno); close(controlling); return 43; }
    if (ioctl(controlling, TIOCGWINSZ, &size) != 0 || size.ws_row != 36 || size.ws_col != 120) {
        printf("CHILD_SIZE row=%d col=%d errno=%d\n", size.ws_row, size.ws_col, errno); close(controlling); return 44;
    }
    puts("CHILD_READY"); fflush(stdout);
    if (!read_key(STDIN_FILENO, 'K')) { puts("CHILD_INPUT_K_FAILED"); tcsetattr(controlling, TCSANOW, &original); close(controlling); return 45; }
    puts("CHILD_GOT_K"); fflush(stdout);
    if (!read_key(STDIN_FILENO, 'R')) { puts("CHILD_INPUT_R_FAILED"); tcsetattr(controlling, TCSANOW, &original); close(controlling); return 46; }
    if (ioctl(controlling, TIOCGWINSZ, &size) != 0 || size.ws_row != 44 || size.ws_col != 132) {
        printf("CHILD_RESIZE row=%d col=%d errno=%d\n", size.ws_row, size.ws_col, errno); tcsetattr(controlling, TCSANOW, &original); close(controlling); return 47;
    }
    int restore = tcsetattr(controlling, TCSANOW, &original);
    int checked = tcgetattr(controlling, &observed);
    int equal = checked == 0 && same_mode(&original, &observed);
    printf("CHILD_RESTORE result=%d checked=%d equal=%d\n", restore, checked, equal);
    close(controlling);
    if (restore || !equal) return 48;
    puts("CHILD_PASS"); fflush(stdout);
    return 23;
}
int main(int argc, char **argv) {
    if (argc == 2 && !strcmp(argv[1], "--child")) return child_probe();
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("PTY_PROBE uid=%u gid=%u\n", geteuid(), getegid());
    int master = -1, slave = -1;
    char name[128] = {0};
    int result = openpty(&master, &slave, name, NULL, NULL);
    printf("openpty=%d errno=%d name=%s\n", result, result ? errno : 0, name);
    if (result) {
        int direct = posix_openpt(O_RDWR | O_NOCTTY | O_CLOEXEC);
        printf("posix_openpt=%d errno=%d\n", direct, direct < 0 ? errno : 0);
        if (direct >= 0) close(direct);
        return 1;
    }
    fcntl(master, F_SETFD, FD_CLOEXEC); fcntl(slave, F_SETFD, FD_CLOEXEC);
    struct winsize size = {.ws_row=36, .ws_col=120}, observed_size = {0};
    result = ioctl(slave, TIOCSWINSZ, &size);
    int resize_error = result ? errno : 0;
    int query = ioctl(slave, TIOCGWINSZ, &observed_size);
    printf("set_window=%d errno=%d get_window=%d row=%u col=%u\n", result, resize_error, query, observed_size.ws_row, observed_size.ws_col);
    if (result || query || observed_size.ws_row != 36 || observed_size.ws_col != 120) { close(master); close(slave); return 2; }
    struct termios original = {0}, raw = {0}, restored = {0};
    result = tcgetattr(slave, &original);
    printf("tcgetattr=%d errno=%d\n", result, result ? errno : 0);
    if (result) { close(master); close(slave); return 3; }
    raw = original; cfmakeraw(&raw);
    result = tcsetattr(slave, TCSANOW, &raw);
    printf("raw_mode=%d errno=%d\n", result, result ? errno : 0);
    if (result) { close(master); close(slave); return 4; }
    int restored_result = tcsetattr(slave, TCSANOW, &original);
    int checked = tcgetattr(slave, &restored);
    printf("restore=%d equal=%d\n", restored_result, checked == 0 && same_mode(&original, &restored));
    if (restored_result || checked || !same_mode(&original, &restored)) { close(master); close(slave); return 5; }
    char executable[4096];
    ssize_t length = readlink("/proc/self/exe", executable, sizeof(executable)-1);
    if (length < 0 || length >= (ssize_t)sizeof(executable)-1) { close(master); close(slave); return 6; }
    executable[length] = 0;
    posix_spawnattr_t attributes;
    posix_spawn_file_actions_t actions;
    posix_spawnattr_init(&attributes); posix_spawn_file_actions_init(&actions);
    sigset_t empty, defaults; sigemptyset(&empty); sigemptyset(&defaults);
    sigaddset(&defaults, SIGINT); sigaddset(&defaults, SIGTERM); sigaddset(&defaults, SIGHUP); sigaddset(&defaults, SIGWINCH);
    int configured = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSID | POSIX_SPAWN_SETSIGMASK | POSIX_SPAWN_SETSIGDEF);
    if (!configured) configured = posix_spawnattr_setsigmask(&attributes, &empty);
    if (!configured) configured = posix_spawnattr_setsigdefault(&attributes, &defaults);
    if (!configured) configured = posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, name, O_RDWR, 0);
    if (!configured) configured = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDOUT_FILENO);
    if (!configured) configured = posix_spawn_file_actions_adddup2(&actions, STDIN_FILENO, STDERR_FILENO);
    char *arguments[] = {executable, "--child", NULL};
    pid_t child = -1;
    result = configured ? configured : posix_spawn(&child, executable, &actions, &attributes, arguments, environ);
    printf("posix_spawn=%d child=%d\n", result, child);
    posix_spawn_file_actions_destroy(&actions); posix_spawnattr_destroy(&attributes); close(slave);
    if (result) { close(master); return 7; }
    fcntl(master, F_SETFL, fcntl(master, F_GETFL) | O_NONBLOCK);
    char output[32768] = {0}; size_t used=0;
    int sent_k=0, sent_r=0, exited=0, status=0;
    long long deadline=now_ms()+8000;
    while (now_ms()<deadline && !exited) {
        struct pollfd waiter={master,POLLIN,0};
        int ready=poll(&waiter,1,50);
        if (ready>0 && (waiter.revents & (POLLIN|POLLHUP))) {
            ssize_t count=read(master,output+used,sizeof(output)-1-used);
            if (count>0) { fwrite(output+used,1,(size_t)count,stdout);used+=(size_t)count;output[used]=0; }
        }
        if (!sent_k && strstr(output,"CHILD_READY")) { sent_k=write(master,"K",1)==1; printf("parent_send_K=%d\n",sent_k); }
        if (!sent_r && strstr(output,"CHILD_GOT_K")) {
            size.ws_row=44;size.ws_col=132;
            int resized=ioctl(master,TIOCSWINSZ,&size);
            printf("parent_resize=%d errno=%d\n",resized,resized?errno:0);
            sent_r=!resized && write(master,"R",1)==1;
            printf("parent_send_R=%d\n",sent_r);
        }
        pid_t waited=waitpid(child,&status,WNOHANG);
        if (waited==child) exited=1;
        if (waited<0 && errno!=EINTR) break;
    }
    if (!exited) { kill(child,SIGKILL);while(waitpid(child,&status,0)<0 && errno==EINTR){} }
    for (;;) {
        ssize_t count=read(master,output+used,sizeof(output)-1-used);
        if (count<=0) break;
        fwrite(output+used,1,(size_t)count,stdout);used+=(size_t)count;output[used]=0;
    }
    close(master);
    printf("child_exited=%d raw_status=%d exit=%d signal=%d\n",exited,status,WIFEXITED(status)?WEXITSTATUS(status):-1,WIFSIGNALED(status)?WTERMSIG(status):0);
    int passed=exited && WIFEXITED(status) && WEXITSTATUS(status)==23 && sent_k && sent_r && strstr(output,"CHILD_PASS");
    puts(passed?"PTY_PROBE_PASS":"PTY_PROBE_FAIL");
    return passed?0:8;
}
