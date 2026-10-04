#include "secure_config.h"
#include <fcntl.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 2) return 99;
    int fd = open(argv[1], O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) return 98;
    int result = ImportPrivateConfig(fd, stdout);
    close(fd);
    return result < 0 ? 3 : result;
}
