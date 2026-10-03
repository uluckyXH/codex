#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

int main(void) {
    int pair[2];
    errno = 0;
    int status = socketpair(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0, pair);
    printf("socketpair status=%d errno=%d\n", status, errno);
    if (status == 0) { close(pair[0]); close(pair[1]); }
    errno = 0;
    int socket_fd = socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC, 0);
    printf("socket status=%d errno=%d\n", socket_fd, errno);
    if (socket_fd < 0) return 1;
    struct sockaddr_un address = { .sun_family = AF_UNIX };
    snprintf(address.sun_path, sizeof(address.sun_path),
        "/data/local/tmp/cdx/socket-probe-%ld", (long)getpid());
    struct stat metadata;
    if (lstat(address.sun_path, &metadata) == 0 || errno != ENOENT) {
        puts("existing pathname refused");
        close(socket_fd);
        return 1;
    }
    errno = 0;
    status = bind(socket_fd, (struct sockaddr *)&address,
        offsetof(struct sockaddr_un, sun_path) + strlen(address.sun_path) + 1);
    int saved_errno = errno;
    printf("bind status=%d errno=%d description=%s\n", status, saved_errno, strerror(saved_errno));
    if (status == 0) {
        errno = 0;
        int result = listen(socket_fd, 1);
        printf("listen status=%d errno=%d\n", result, errno);
        unlink(address.sun_path);
    }
    close(socket_fd);
    return status == 0 ? 0 : 1;
}
