#include "terminal_bridge.h"
#include "host_layout.h"
#include "pty_session.h"
#include <cmath>
#include <string>
#include <vector>

namespace {
napi_value String(napi_env env, const std::string &value) {
    napi_value result;
    if (napi_create_string_utf8(env, value.data(), value.size(), &result) != napi_ok) return nullptr;
    return result;
}
napi_value Error(napi_env env, const char *reason) {
    return String(env, std::string("{\"ok\":false,\"error\":") + codex_hnp::JsonString(reason) + "}");
}
bool Arguments(napi_env env, napi_callback_info info, size_t count, napi_value *argv) {
    size_t argc = count;
    return napi_get_cb_info(env, info, &argc, argv, nullptr, nullptr) == napi_ok && argc == count;
}
bool Text(napi_env env, napi_value argument, size_t limit, std::string &value, bool allowNull = false) {
    size_t length = 0, copied = 0;
    if (napi_get_value_string_utf8(env, argument, nullptr, 0, &length) != napi_ok || length > limit) return false;
    std::vector<char> buffer(length + 1);
    if (napi_get_value_string_utf8(env, argument, buffer.data(), buffer.size(), &copied) != napi_ok || copied != length) return false;
    value.assign(buffer.data(), copied);
    return allowNull || value.find('\0') == std::string::npos;
}
bool Size(napi_env env, napi_value first, napi_value second, int &columns, int &rows) {
    double c = 0, r = 0;
    if (napi_get_value_double(env, first, &c) != napi_ok || napi_get_value_double(env, second, &r) != napi_ok ||
        !std::isfinite(c) || !std::isfinite(r) || std::floor(c) != c || std::floor(r) != r ||
        c < 1 || c > 1000 || r < 1 || r > 1000) return false;
    columns = static_cast<int>(c); rows = static_cast<int>(r);
    return codex_hnp::ValidTerminalSize(columns, rows);
}
bool Identity(napi_env env, napi_value *argv, std::string &kind, std::string &id) {
    return Text(env, argv[0], 8, kind) && (kind == "shell" || kind == "codex") && Text(env, argv[1], 32, id);
}
}
napi_value TerminalStart(napi_env env, napi_callback_info info) {
    napi_value argv[6]; std::string files, kind, cwd, policy; int columns = 0, rows = 0;
    if (!Arguments(env, info, 6, argv) || !Text(env, argv[0], 4095, files) || !Text(env, argv[1], 8, kind) ||
        !Text(env, argv[2], 4095, cwd) || !Text(env, argv[3], 32, policy) || !Size(env, argv[4], argv[5], columns, rows))
        return Error(env, "invalid_start_arguments");
    return String(env, codex_hnp::PtyStart(files, kind, cwd, policy, columns, rows));
}
napi_value TerminalWrite(napi_env env, napi_callback_info info) {
    napi_value argv[3]; std::string kind, id, bytes;
    if (!Arguments(env, info, 3, argv) || !Identity(env, argv, kind, id) || !Text(env, argv[2], 16384, bytes, true))
        return Error(env, "invalid_input_arguments");
    return String(env, codex_hnp::PtyWrite(kind, id, bytes));
}
napi_value TerminalResize(napi_env env, napi_callback_info info) {
    napi_value argv[4]; std::string kind, id; int columns = 0, rows = 0;
    if (!Arguments(env, info, 4, argv) || !Identity(env, argv, kind, id) || !Size(env, argv[2], argv[3], columns, rows))
        return Error(env, "invalid_resize_arguments");
    return String(env, codex_hnp::PtyResize(kind, id, columns, rows));
}
napi_value TerminalRead(napi_env env, napi_callback_info info) {
    napi_value argv[2]; std::string kind, id;
    if (!Arguments(env, info, 2, argv) || !Identity(env, argv, kind, id)) return Error(env, "invalid_read_identity");
    return String(env, codex_hnp::PtyRead(kind, id));
}
napi_value TerminalStop(napi_env env, napi_callback_info info) {
    napi_value argv[3]; std::string kind, id, operation;
    if (!Arguments(env, info, 3, argv) || !Identity(env, argv, kind, id) || !Text(env, argv[2], 16, operation))
        return Error(env, "invalid_control_arguments");
    return String(env, codex_hnp::PtyControl(kind, id, operation));
}
napi_value TerminalStatus(napi_env env, napi_callback_info info) {
    napi_value argv[2]; std::string kind, id;
    if (!Arguments(env, info, 2, argv) || !Identity(env, argv, kind, id)) return Error(env, "invalid_status_identity");
    return String(env, codex_hnp::PtyStatus(kind, id));
}
