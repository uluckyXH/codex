#define _GNU_SOURCE
#include <stdio.h>
#include <unistd.h>
int main(int argc, char **argv) {
    char path[4096] = {0};
    ssize_t length = readlink("/proc/self/exe", path, sizeof(path) - 1);
    printf("uid=%u real_exe=%s readlink_bytes=%zd\n", getuid(), path, length);
    for (int i = 0; i < argc; ++i) printf("argv[%d]=%s\n", i, argv[i]);
    return 0;
}
