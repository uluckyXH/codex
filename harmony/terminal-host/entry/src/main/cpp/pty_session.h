#pragma once
#include <string>

// A single bounded terminal attached only to the fixed HNP Codex installation.
// Every function is thread-safe and returns JSON; terminal bytes only appear as
// dataBase64 in PtyRead(), never in logs or error strings. The host must prepare
// the private application directories and import config before PtyStart().
// Input queue is bounded to 64 KiB; accepted input is drained by the watcher.
// Stop intentionally discards pending input and terminates only our owned child.
namespace codex_hnp {
std::string PtyStart(int columns, int rows);
std::string PtyWrite(const std::string &bytes); // At most 16384 UTF-8 bytes per call; enqueue all or reject without writing.
std::string PtyResize(int columns, int rows);
std::string PtyRead(); // Nonblocking; at most 16384 terminal bytes per call.
std::string PtyStop(); // Nonblocking TERM, then a bounded watchdog KILL/reap.
std::string PtyStatus();
}
