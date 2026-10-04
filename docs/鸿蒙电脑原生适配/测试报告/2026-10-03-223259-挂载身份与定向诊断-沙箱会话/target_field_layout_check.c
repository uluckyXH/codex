/* Compile-only SDK ABI evidence. This is not a device execution test. */
#define _GNU_SOURCE 1
#include <stddef.h>
#include <fcntl.h>
#include <linux/stat.h>

_Static_assert(sizeof(struct statx) == 256, "statx ABI size");
_Static_assert(offsetof(struct statx, stx_mask) == 0, "mask offset");
_Static_assert(offsetof(struct statx, stx_mode) == 28, "mode offset");
_Static_assert(offsetof(struct statx, stx_ino) == 32, "inode offset");
_Static_assert(offsetof(struct statx, stx_dev_major) == 136, "device major offset");
_Static_assert(offsetof(struct statx, stx_dev_minor) == 140, "device minor offset");
_Static_assert(offsetof(struct statx, stx_mnt_id) == 144, "mount ID offset");
_Static_assert(STATX_MNT_ID == 0x1000, "mount ID valid bit");
_Static_assert(STATX_BASIC_STATS == 0x7ff, "basic stats request bits");
_Static_assert(AT_EMPTY_PATH == 0x1000, "same descriptor query");
