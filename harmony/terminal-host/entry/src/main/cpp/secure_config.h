#pragma once
#include <cstdio>

// Caller supplies a pinned, verified application filesDir descriptor after
// hardening its ancestor chain. 0: absent, 1: existing, 2: imported, -1: rejected.
// Contents are opaque here; the trusted Mac importer validates TOML and model.
int ImportPrivateConfig(int files_fd, FILE *report);
