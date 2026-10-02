#pragma once

#include <errno.h>
#include <stdint.h>
#include <stdlib.h>
#include <unistd.h>

/* OHOS does not export GNU get_current_dir_name(). Use the physical cwd and
 * ordinary getcwd buffers, without relying on the non-POSIX getcwd(NULL, 0).
 * The caller owns the returned buffer, just as with the GNU function. */
static inline char *codex_ohos_get_current_dir_name(void) {
    size_t size = 128;
    for (;;) {
        char *buffer = malloc(size);
        if (buffer == NULL) {
            errno = ENOMEM;
            return NULL;
        }
        if (getcwd(buffer, size) != NULL) {
            return buffer;
        }
        int error = errno;
        free(buffer);
        if (error != ERANGE) {
            errno = error;
            return NULL;
        }
        if (size > SIZE_MAX / 2) {
            errno = ENOMEM;
            return NULL;
        }
        size *= 2;
    }
}

#define get_current_dir_name codex_ohos_get_current_dir_name
