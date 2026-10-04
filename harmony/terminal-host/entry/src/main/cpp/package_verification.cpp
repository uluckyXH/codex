#include "package_verification.h"
#include "native_package.h"
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

// Only these public installation artifacts can be copied. User files/config are
// never accepted as input. The private copies are for host-side digest checking
// and are never executed.
bool ExportInstalledPackageForVerification(int files, FILE *report, const std::string &prefix) {
    std::string directory = "verify-" + prefix;
    if (mkdirat(files, directory.c_str(), 0700) != 0) return false;
    int outputDirectory = openat(files, directory.c_str(), O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (outputDirectory < 0) return false;
    fprintf(report, "verification_directory=/data/storage/el2/base/files/%s\n", directory.c_str());
    const char *paths[] = {"bin/codex", "codex-path/rg", "codex-resources/bwrap", "codex-resources/harmony-runtime-probe",
        "codex-path/apply_patch", "codex-path/applypatch", "codex-path/codex-linux-sandbox", "codex-path/codex-execve-wrapper", "codex-package.json"};
    bool passed = true;
    for (const char *relative : paths) {
        std::string source = std::string(CODEX_HNP_PACKAGE_PATH) + "/" + relative;
        int input = open(source.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC | O_NONBLOCK);
        struct stat before = {}, after = {};
        bool valid = input >= 0 && fstat(input, &before) == 0 && S_ISREG(before.st_mode) &&
            (before.st_uid == 0 || before.st_uid == 3060) && (before.st_mode & 07022) == 0 &&
            before.st_size > 0 && before.st_size < 536870912;
        if (!valid) {
            fprintf(report, "verification_source=%s rejected=yes errno=%d\n", relative, errno);
            if (input >= 0) close(input); passed = false; break;
        }
        std::string name(relative);
        for (char &character : name) if (character == '/') character = '_';
        int output = openat(outputDirectory, name.c_str(), O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW | O_CLOEXEC, 0600);
        if (output < 0) { close(input); passed = false; break; }
        off_t copied = 0;
        char buffer[65536];
        while (copied < before.st_size) {
            ssize_t count;
            do { count = read(input, buffer, sizeof(buffer)); } while (count < 0 && errno == EINTR);
            if (count <= 0) { valid = false; break; }
            ssize_t written = 0;
            while (written < count) {
                ssize_t result = write(output, buffer + written, static_cast<size_t>(count - written));
                if (result < 0 && errno == EINTR) continue;
                if (result <= 0) { valid = false; break; }
                written += result;
            }
            if (!valid) break;
            copied += count;
        }
        valid = valid && copied == before.st_size && fstat(input, &after) == 0 && before.st_dev == after.st_dev &&
            before.st_ino == after.st_ino && before.st_size == after.st_size && before.st_mtime == after.st_mtime &&
            before.st_mode == after.st_mode && before.st_uid == after.st_uid && fsync(output) == 0;
        fprintf(report, "verification_source=%s exported=%s bytes=%lld success=%d\n", relative, name.c_str(), static_cast<long long>(copied), valid ? 1 : 0);
        fflush(report); close(output); close(input);
        if (!valid) { passed = false; break; }
    }
    close(outputDirectory);
    return passed;
}
