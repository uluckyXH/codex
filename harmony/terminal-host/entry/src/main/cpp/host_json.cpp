#include "host_json.h"
#include <cctype>
#include <cstdlib>

namespace codex_hnp {
namespace {
class Reader {
public:
    Reader(const std::string &input, const std::vector<std::string> &path) : input_(input), path_(path) {}
    bool Read(std::string &value) {
        if (input_.empty() || input_.size() > 16384 || path_.empty()) return false;
        if (!Value(0, 0, true)) return false;
        Space(); if (position_ != input_.size() || found_ != 1) return false;
        value = result_; return true;
    }
private:
    void Space() { while (position_ < input_.size() && (input_[position_] == ' ' || input_[position_] == '\n' || input_[position_] == '\r' || input_[position_] == '\t')) ++position_; }
    bool Take(char value) { Space(); if (position_ >= input_.size() || input_[position_] != value) return false; ++position_; return true; }
    bool Hex(unsigned &value) {
        if (position_ + 4 > input_.size()) return false;
        value = 0;
        for (unsigned i = 0; i < 4; ++i) {
            char c = input_[position_++]; unsigned digit;
            if (c >= '0' && c <= '9') digit = c - '0';
            else if (c >= 'a' && c <= 'f') digit = c - 'a' + 10;
            else if (c >= 'A' && c <= 'F') digit = c - 'A' + 10;
            else return false;
            value = value * 16 + digit;
        }
        return true;
    }
    void Utf8(unsigned value, std::string &out) {
        if (value < 0x80) out += static_cast<char>(value);
        else if (value < 0x800) { out += static_cast<char>(0xc0 | (value >> 6)); out += static_cast<char>(0x80 | (value & 63)); }
        else if (value < 0x10000) { out += static_cast<char>(0xe0 | (value >> 12)); out += static_cast<char>(0x80 | ((value >> 6) & 63)); out += static_cast<char>(0x80 | (value & 63)); }
        else { out += static_cast<char>(0xf0 | (value >> 18)); out += static_cast<char>(0x80 | ((value >> 12) & 63)); out += static_cast<char>(0x80 | ((value >> 6) & 63)); out += static_cast<char>(0x80 | (value & 63)); }
    }
    bool String(std::string &value) {
        if (!Take('"')) return false;
        value.clear();
        while (position_ < input_.size()) {
            unsigned char c = input_[position_++];
            if (c == '"') return true;
            if (c < 32) return false;
            if (c != '\\') { value += c; continue; }
            if (position_ >= input_.size()) return false;
            char escape = input_[position_++];
            if (escape == '"' || escape == '\\' || escape == '/') value += escape;
            else if (escape == 'b') value += '\b';
            else if (escape == 'f') value += '\f';
            else if (escape == 'n') value += '\n';
            else if (escape == 'r') value += '\r';
            else if (escape == 't') value += '\t';
            else if (escape == 'u') {
                unsigned unicode;
                if (!Hex(unicode)) return false;
                if (unicode >= 0xd800 && unicode <= 0xdbff) {
                    if (position_ + 2 > input_.size() || input_[position_++] != '\\' || input_[position_++] != 'u') return false;
                    unsigned low;
                    if (!Hex(low) || low < 0xdc00 || low > 0xdfff) return false;
                    unicode = 0x10000 + ((unicode - 0xd800) << 10) + low - 0xdc00;
                } else if (unicode >= 0xdc00 && unicode <= 0xdfff) return false;
                Utf8(unicode, value);
            } else return false;
        }
        return false;
    }
    bool Number() {
        size_t begin = position_;
        if (position_ < input_.size() && input_[position_] == '-') ++position_;
        if (position_ >= input_.size()) return false;
        if (input_[position_] == '0') ++position_;
        else {
            if (input_[position_] < '1' || input_[position_] > '9') return false;
            while (position_ < input_.size() && std::isdigit(static_cast<unsigned char>(input_[position_]))) ++position_;
        }
        if (position_ < input_.size() && input_[position_] == '.') {
            ++position_; size_t fraction = position_;
            while (position_ < input_.size() && std::isdigit(static_cast<unsigned char>(input_[position_]))) ++position_;
            if (position_ == fraction) return false;
        }
        if (position_ < input_.size() && (input_[position_] == 'e' || input_[position_] == 'E')) {
            ++position_; if (position_ < input_.size() && (input_[position_] == '+' || input_[position_] == '-')) ++position_;
            size_t exponent = position_;
            while (position_ < input_.size() && std::isdigit(static_cast<unsigned char>(input_[position_]))) ++position_;
            if (exponent == position_) return false;
        }
        return position_ > begin;
    }
    bool Value(unsigned depth, size_t component, bool match) {
        if (depth > 24) return false;
        Space(); if (position_ >= input_.size()) return false;
        char first = input_[position_];
        if (match && component == path_.size()) {
            std::string value;
            if (!String(value)) return false;
            result_ = value; ++found_; return true;
        }
        if (first == '{') {
            ++position_; Space(); if (position_ < input_.size() && input_[position_] == '}') { ++position_; return true; }
            bool requestedSeen = false;
            for (;;) {
                std::string key; if (!String(key) || !Take(':')) return false;
                bool child = match && component < path_.size() && key == path_[component];
                if (child && requestedSeen) return false;
                if (child) requestedSeen = true;
                if (!Value(depth + 1, child ? component + 1 : component, child)) return false;
                if (Take('}')) return true;
                if (!Take(',')) return false;
            }
        }
        if (first == '[') {
            ++position_; Space(); if (position_ < input_.size() && input_[position_] == ']') { ++position_; return true; }
            for (;;) { if (!Value(depth + 1, component, false)) return false; if (Take(']')) return true; if (!Take(',')) return false; }
        }
        if (first == '"') { std::string ignored; return String(ignored); }
        for (const char *literal : {"true", "false", "null"}) {
            std::string token(literal);
            if (input_.compare(position_, token.size(), token) == 0) { position_ += token.size(); return true; }
        }
        return Number();
    }
    const std::string &input_; const std::vector<std::string> &path_;
    size_t position_ = 0; unsigned found_ = 0; std::string result_;
};
}
bool JsonPathString(const std::string &json, const std::vector<std::string> &path, std::string &value) {
    return Reader(json, path).Read(value);
}
}
