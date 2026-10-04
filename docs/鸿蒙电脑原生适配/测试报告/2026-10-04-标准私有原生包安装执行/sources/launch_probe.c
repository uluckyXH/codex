#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
int main(int argc, char **argv) {
    char cwd[4096];
    printf("probe_uid=%u cwd=%s\n", getuid(), getcwd(cwd, sizeof(cwd)) ? cwd : "(error)");
    printf("home=%s\nstate=%s\ntmp=%s\n", getenv("HOME"), getenv("CODEX_HOME"), getenv("TMPDIR"));
    char byte;
    printf("stdin_read=%zd isatty=%d\n", read(STDIN_FILENO, &byte, 1), isatty(STDIN_FILENO));
    fprintf(stderr, "launch_probe_stderr=yes\n");
    fflush(stdout); fflush(stderr);
    if (argc == 2 && strcmp(argv[1], "slow") == 0) sleep(20);
    return 37;
}
