// Compile-only validation against the installed SDK's public declarations.
#include "harmony_runtime_probe_context.h"
#include <AbilityKit/ability_runtime/application_context.h>

static_assert(__is_same(HarmonyContextFunction,
                        decltype(&OH_AbilityRuntime_ApplicationContextGetFilesDir)));
static_assert(__is_same(HarmonyContextFunction,
                        decltype(&OH_AbilityRuntime_ApplicationContextGetCacheDir)));
static_assert(__is_same(HarmonyContextFunction,
                        decltype(&OH_AbilityRuntime_ApplicationContextGetTempDir)));
