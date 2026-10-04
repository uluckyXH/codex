#include <napi/native_api.h>
#include "codex_launcher.h"
#include "terminal_bridge.h"
#include <sys/stat.h>
static napi_value Init(napi_env env, napi_value exports) {
    // Scope is this dedicated app process and children, never the system or HOME.
    umask(0077);
    napi_property_descriptor descriptors[] = {
        {"runCodex", nullptr, RunCodex, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"runRequestedCodex", nullptr, RunRequestedCodex, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"cancelCodex", nullptr, CancelCodex, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalStart", nullptr, TerminalStart, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalWrite", nullptr, TerminalWrite, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalResize", nullptr, TerminalResize, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalRead", nullptr, TerminalRead, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalStop", nullptr, TerminalStop, nullptr, nullptr, nullptr, napi_default, nullptr},
        {"terminalStatus", nullptr, TerminalStatus, nullptr, nullptr, nullptr, napi_default, nullptr}
    };
    napi_define_properties(env, exports, sizeof(descriptors) / sizeof(descriptors[0]), descriptors);
    return exports;
}
static napi_module module = {1, 0, nullptr, Init, "hostprobe", nullptr, {0}};
extern "C" __attribute__((constructor)) void RegisterHostProbe() { napi_module_register(&module); }
