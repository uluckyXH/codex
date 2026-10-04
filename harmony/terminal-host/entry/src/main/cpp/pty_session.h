#pragma once
#include <string>

// Two independent PTYs. Every operation after start requires its opaque token.
// Input is accepted whole (<=16 KiB) into a bounded 64 KiB queue. Reads return
// <=16 KiB of base64 bytes; input/output/configuration never enter diagnostics.
namespace codex_hnp {
std::string PtyStart(const std::string &files, const std::string &kind, const std::string &cwd,
                     const std::string &policy, int columns, int rows);
std::string PtyWrite(const std::string &kind, const std::string &id, const std::string &bytes);
std::string PtyResize(const std::string &kind, const std::string &id, int columns, int rows);
std::string PtyRead(const std::string &kind, const std::string &id);
std::string PtyControl(const std::string &kind, const std::string &id, const std::string &operation);
std::string PtyStatus(const std::string &kind, const std::string &id);
}
