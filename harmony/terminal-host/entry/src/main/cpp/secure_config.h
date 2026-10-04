#pragma once
#include <cstdio>

// Caller supplies the pinned, verified codex data root after shared CLI
// initialization. Inbox is host/handoff-private; destination is state/.
// 0: absent, 1: existing, 2: imported, -1: rejected.
// Contents are opaque here; the trusted Mac importer validates TOML and model.
int ImportPrivateConfig(int files_fd, FILE *report);
