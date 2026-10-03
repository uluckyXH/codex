#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>

static void report(const char *stage, int value) {
    int saved_errno = value < 0 ? errno : 0;
    printf("stage=%s result=%d errno=%d\n", stage, value, saved_errno);
}

static void probe_fd(int fd) {
    struct termios original;
    int ret = tcgetattr(fd, &original);
    report("libc_tcgetattr", ret);
    if (ret == 0) {
        report("libc_tcsetattr_unchanged", tcsetattr(fd, TCSANOW, &original));
        struct termios raw = original;
        cfmakeraw(&raw);
        int raw_result = tcsetattr(fd, TCSANOW, &raw);
        report("libc_tcsetattr_raw", raw_result);
        if (raw_result == 0) report("libc_tcsetattr_restore", tcsetattr(fd, TCSANOW, &original));
    }
    /* AArch64 asm-generic termios2 request encodes a 44-byte kernel struct. */
    unsigned char termios2[256] = {0};
    int ret2 = ioctl(fd, 0x802c542aUL, termios2);
    report("ioctl_TCGETS2", ret2);
    if (ret2 == 0) report("ioctl_TCSETS2_unchanged", ioctl(fd, 0x402c542bUL, termios2));
    struct winsize size = {0};
    report("ioctl_TIOCGWINSZ", ioctl(fd, TIOCGWINSZ, &size));
}

int main(void) {
    setvbuf(stdout, NULL, _IONBF, 0);
    printf("uid=%u stdin_isatty=%d stdout_isatty=%d\n", getuid(), isatty(0), isatty(1));
    puts("fd=stdin");
    probe_fd(STDIN_FILENO);
    int tty = open("/dev/tty", O_RDWR | O_CLOEXEC);
    report("open_dev_tty", tty);
    if (tty >= 0) {
        puts("fd=dev_tty");
        probe_fd(tty);
        close(tty);
    }
    puts("terminal_probe_complete");
    return 0;
}
