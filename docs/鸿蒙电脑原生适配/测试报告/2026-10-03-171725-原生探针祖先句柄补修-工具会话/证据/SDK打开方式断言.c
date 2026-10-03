#define main runtime_probe_entrypoint
#include "harmony_runtime_probe.c"
_Static_assert(ANCESTOR_OPEN_FLAGS == (O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC), "OHOS ancestors require O_PATH without fallback");
const int sdk_ancestor_flags = O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
const int sdk_readable_directory_flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
