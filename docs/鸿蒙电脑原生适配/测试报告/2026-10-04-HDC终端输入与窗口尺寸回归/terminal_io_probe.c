#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static void window_probe(const char *name, int fd) {
    struct winsize size = {0};
    int result = ioctl(fd, TIOCGWINSZ, &size);
    int error = result < 0 ? errno : 0;
    printf("fd=%s stage=TIOCGWINSZ result=%d errno=%d rows=%u columns=%u\n",
           name, result, error, size.ws_row, size.ws_col);
    if (result == 0) {
        result = ioctl(fd, TIOCSWINSZ, &size);
        error = result < 0 ? errno : 0;
        printf("fd=%s stage=TIOCSWINSZ_unchanged result=%d errno=%d\n", name, result, error);
    }
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("uid=%u stdin_isatty=%d stdout_isatty=%d\n", getuid(), isatty(0), isatty(1));
    window_probe("stdin", 0);
    window_probe("stdout", 1);
    int tty = open("/dev/tty", O_RDWR | O_CLOEXEC);
    printf("stage=open_dev_tty result=%d errno=%d\n", tty, tty < 0 ? errno : 0);
    if (tty < 0) return 1;
    window_probe("dev_tty", tty);
    struct termios initial = {0}, after = {0}, raw;
    if (tcgetattr(tty, &initial) < 0) return 2;
    int unchanged = tcsetattr(0, TCSANOW, &initial);
    int unchanged_errno = unchanged < 0 ? errno : 0;
    if (tcgetattr(tty, &after) < 0) return 3;
    printf("stage=inherited_stdin_set result=%d errno=%d modes_unchanged=%d\n", unchanged,
           unchanged_errno, initial.c_iflag == after.c_iflag && initial.c_oflag == after.c_oflag &&
           initial.c_cflag == after.c_cflag && initial.c_lflag == after.c_lflag &&
           memcmp(initial.c_cc, after.c_cc, sizeof(initial.c_cc)) == 0);
    raw = initial;
    cfmakeraw(&raw);
    if (tcsetattr(tty, TCSANOW, &raw) < 0) return 4;
    puts("__TERMINAL_INPUT_READY__");
    struct pollfd request = {.fd = tty, .events = POLLIN};
    int polled = poll(&request, 1, 5000);
    int poll_errno = polled < 0 ? errno : 0;
    unsigned char input = 0;
    ssize_t received = polled > 0 && (request.revents & POLLIN) ? read(tty, &input, 1) : -1;
    int restored = tcsetattr(tty, TCSANOW, &initial);
    int restore_errno = restored < 0 ? errno : 0;
    memset(&after, 0, sizeof(after));
    int observed = tcgetattr(tty, &after);
    int same = observed == 0 && initial.c_iflag == after.c_iflag && initial.c_oflag == after.c_oflag &&
               initial.c_cflag == after.c_cflag && initial.c_lflag == after.c_lflag &&
               memcmp(initial.c_cc, after.c_cc, sizeof(initial.c_cc)) == 0;
    printf("stage=poll result=%d errno=%d revents=%d read_count=%ld byte=%u\n",
           polled, poll_errno, request.revents, (long)received, input);
    printf("stage=restore result=%d errno=%d modes_restored=%d\n", restored, restore_errno, same);
    close(tty);
    int passed = polled == 1 && received == 1 && input == 'K' && restored == 0 && same;
    printf("terminal_io_probe_complete passed=%d\n", passed);
    return passed ? 0 : 5;
}
