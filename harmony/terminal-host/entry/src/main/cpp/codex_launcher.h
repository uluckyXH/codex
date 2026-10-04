#pragma once
#include <napi/native_api.h>
#include <string>
napi_value RunCodex(napi_env env, napi_callback_info info);
napi_value CancelCodex(napi_env env, napi_callback_info info);
napi_value RunRequestedCodex(napi_env env, napi_callback_info info);
std::string PrepareCodexTerminal();
void ReleaseCodexTerminal();
void RefreshCodexTerminal();
