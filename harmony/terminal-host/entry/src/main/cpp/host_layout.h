#pragma once
#include <string>
#include <sys/stat.h>
#include <vector>

namespace codex_hnp {
class Descriptor {
public:
    explicit Descriptor(int fd = -1) : fd_(fd) {}
    ~Descriptor();
    Descriptor(const Descriptor &) = delete;
    Descriptor &operator=(const Descriptor &) = delete;
    Descriptor(Descriptor &&other) noexcept : fd_(other.release()) {}
    Descriptor &operator=(Descriptor &&other) noexcept;
    int get() const { return fd_; }
    int release() { int fd = fd_; fd_ = -1; return fd; }
    void reset(int fd = -1);
private:
    int fd_;
};

// Every named edge and descriptor remains pinned through launch/revalidation.
// The CLI owns state/r/tmp/logs initialization. This class only adds host data.
class HostLayout {
public:
    bool Open(const std::string &contextFiles, bool initializeCli, std::string &error);
    bool Revalidate(std::string &error) const;
    int Fd(const std::string &relative) const;
    const std::string &Root() const { return root_; }
    const std::string &Files() const { return files_; }
    bool InitializationUnverified() const { return initializationUnverified_; }
private:
    struct Edge {
        Descriptor fd;
        std::string name;
        std::string relative;
        int parent;
        bool privateDirectory;
    };
    bool Append(int parent, const std::string &name, const std::string &relative,
                bool privateDirectory, bool create, std::string &error);
    std::vector<Edge> edges_;
    std::string files_;
    std::string root_;
    uid_t uid_ = 0;
    gid_t gid_ = 0;
    bool initializationUnverified_ = false;
};

// Never accepts an environment-selected path. Compares the native Context API
// with the ArkTS Context and fails closed if that native source is unavailable.
bool VerifyApplicationContext(const std::string &files, std::string &error);
// Explicit GUI operation only; never part of CLI initialization or PTY start.
// Only the native Context's files and its direct app-owned base may be tightened.
bool SecureApplicationContext(const std::string &files, std::string &evidence, std::string &error);
bool InitializeCliDirectories(const std::string &files, std::string &error, bool &cleanupUnknown);
std::vector<std::string> HostEnvironment(const std::string &dataRoot, const std::string &term);
std::vector<char *> ArgumentPointers(std::vector<std::string> &values);
std::string JsonString(const std::string &value);
std::string Base64(const char *bytes, size_t size);
bool SameObject(const struct stat &left, const struct stat &right);
bool PrivateDirectory(int fd);
bool ValidTerminalSize(int columns, int rows);
}
