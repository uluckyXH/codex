#include "terminal_bridge.h"
#include "codex_launcher.h"
#include "pty_session.h"
#include <cmath>
#include <string>
#include <vector>

static napi_value String(napi_env env, const std::string &value) {
    napi_value result;
    if (napi_create_string_utf8(env, value.data(), value.size(), &result) != napi_ok) return nullptr;
    return result;
}

static napi_value Error(napi_env env, const char *reason) {
    // Reasons are fixed literals; terminal output, commands and configuration never enter this channel.
    return String(env, std::string("{\"ok\":false,\"error\":\"") + reason + "\"}");
}

static bool Dimensions(napi_env env, napi_callback_info info, int &columns, int &rows) {
    size_t argc = 2; napi_value argv[2]; double values[2] = {};
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 2) return false;
    for (unsigned index = 0; index < 2; ++index) {
        if (napi_get_value_double(env, argv[index], &values[index]) != napi_ok || !std::isfinite(values[index]) ||
            values[index] < 1 || values[index] > 1000 || std::floor(values[index]) != values[index]) return false;
    }
    columns = static_cast<int>(values[0]); rows = static_cast<int>(values[1]);
    return columns * rows <= 100000;
}

napi_value TerminalStart(napi_env env, napi_callback_info info) {
    int columns = 0, rows = 0;
    if (!Dimensions(env, info, columns, rows)) return Error(env, "invalid_dimensions");
    std::string error = PrepareCodexTerminal();
    if (!error.empty()) return Error(env, error.c_str());
    std::string result = codex_hnp::PtyStart(columns, rows);
    if (result.find("\"ok\":true") == std::string::npos) ReleaseCodexTerminal();
    return String(env, result);
}

napi_value TerminalWrite(napi_env env, napi_callback_info info) {
    size_t argc = 1, length = 0; napi_value argv[1];
    if (napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) != napi_ok || argc != 1 ||
        napi_get_value_string_utf8(env, argv[0], nullptr, 0, &length) != napi_ok || length == 0 || length > 16384)
        return Error(env, "input_size_or_type");
    std::vector<char> buffer(length + 1); size_t copied = 0;
    if (napi_get_value_string_utf8(env, argv[0], buffer.data(), buffer.size(), &copied) != napi_ok || copied != length)
        return Error(env, "input_encoding");
    return String(env, codex_hnp::PtyWrite(std::string(buffer.data(), copied)));
}

napi_value TerminalResize(napi_env env, napi_callback_info info) {
    int columns = 0, rows = 0;
    if (!Dimensions(env, info, columns, rows)) return Error(env, "invalid_dimensions");
    return String(env, codex_hnp::PtyResize(columns, rows));
}

napi_value TerminalRead(napi_env env, napi_callback_info) {
    std::string result = codex_hnp::PtyRead();
    RefreshCodexTerminal();
    return String(env, result);
}

napi_value TerminalStop(napi_env env, napi_callback_info) {
    return String(env, codex_hnp::PtyStop());
}

napi_value TerminalStatus(napi_env env, napi_callback_info) {
    RefreshCodexTerminal();
    return String(env, codex_hnp::PtyStatus());
}
