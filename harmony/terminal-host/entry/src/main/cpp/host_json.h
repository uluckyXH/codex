#pragma once
#include <string>
#include <vector>
namespace codex_hnp {
// Bounded JSON traversal used only for the offline initializer's paths object.
// Reject duplicate requested properties, invalid JSON, and excess nesting.
bool JsonPathString(const std::string &json, const std::vector<std::string> &path, std::string &value);
}
