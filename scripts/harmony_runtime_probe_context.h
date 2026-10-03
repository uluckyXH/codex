/* SDK 6.1.1.125 application_context.h includes start_options.h, whose C++
 * reference parameters are not C11-compatible. The three directory queries
 * share this function type, with the result enum from the actual SDK header.
 * Target validation compares it with all three SDK declarations in C++ mode.
 * No platform structure layout is copied here.
 */
#ifndef CODEX_HARMONY_RUNTIME_PROBE_CONTEXT_H
#define CODEX_HARMONY_RUNTIME_PROBE_CONTEXT_H
#include <AbilityKit/ability_runtime/ability_runtime_common.h>
#include <stdint.h>

typedef AbilityRuntime_ErrorCode (*HarmonyContextFunction)(char *, int32_t, int32_t *);
#endif
