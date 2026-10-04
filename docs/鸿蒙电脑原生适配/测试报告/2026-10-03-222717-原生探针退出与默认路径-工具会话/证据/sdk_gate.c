#define main probe_program_main
#include "harmony_runtime_probe.c"
_Static_assert(ANCESTOR_OPEN_FLAGS == (O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC), "ancestor flags");
_Static_assert(sizeof(uid_t) == 4, "SDK UID width matches Rust runtime naming");
const int sdk_ancestor_flags = ANCESTOR_OPEN_FLAGS;
const int sdk_readable_directory_flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
