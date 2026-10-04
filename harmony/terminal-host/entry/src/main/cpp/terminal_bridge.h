#pragma once
#include <napi/native_api.h>
napi_value TerminalStart(napi_env env, napi_callback_info info);
napi_value TerminalWrite(napi_env env, napi_callback_info info);
napi_value TerminalResize(napi_env env, napi_callback_info info);
napi_value TerminalRead(napi_env env, napi_callback_info info);
napi_value TerminalStop(napi_env env, napi_callback_info info);
napi_value TerminalStatus(napi_env env, napi_callback_info info);
