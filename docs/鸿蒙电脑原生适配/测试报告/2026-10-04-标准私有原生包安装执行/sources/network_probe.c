#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *input = fopen(argv[1], "r");
    if (!input) return 3;
    char hostname[256], port[8];
    if (!fgets(hostname, sizeof(hostname), input) || !fgets(port, sizeof(port), input)) {
        fclose(input); return 4;
    }
    fclose(input);
    hostname[strcspn(hostname, "\r\n")] = 0;
    port[strcspn(port, "\r\n")] = 0;
    struct addrinfo hints = {.ai_family = AF_UNSPEC, .ai_socktype = SOCK_STREAM};
    struct addrinfo *addresses = NULL;
    alarm(30);
    int result = getaddrinfo(hostname, port, &hints, &addresses);
    printf("dns_result=%d\n", result);
    if (result) return 5;
    int count = 0, succeeded = 0;
    for (struct addrinfo *p = addresses; p && count < 6; p = p->ai_next) {
        ++count;
        int fd = socket(p->ai_family, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0);
        if (fd < 0) { printf("family=%d socket_errno=%d\n", p->ai_family, errno); continue; }
        int error = 0;
        if (connect(fd, p->ai_addr, p->ai_addrlen) != 0) {
            error = errno;
            if (error == EINPROGRESS) {
                struct pollfd waiter = {.fd = fd, .events = POLLOUT};
                int ready = poll(&waiter, 1, 3500);
                if (ready > 0) {
                    socklen_t size = sizeof(error);
                    if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size)) error = errno;
                } else error = ready == 0 ? ETIMEDOUT : errno;
            }
        }
        printf("family=%d tcp_errno=%d\n", p->ai_family, error);
        if (!error) succeeded++;
        close(fd);
    }
    freeaddrinfo(addresses);
    printf("addresses_tested=%d tcp_succeeded=%d\n", count, succeeded);
    return succeeded ? 0 : 6;
}
