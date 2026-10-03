#!/usr/bin/env python3
import re
import sys
import time
print('$ ', end='', flush=True)
command=sys.stdin.readline()
if "then LINES=36 COLUMNS=120 '" not in command:
    print('missing validated dimension prefix', flush=True)
    sys.exit(20)
start=re.search(r'(__HARMONY_CODEX_[0-9]+_[0-9]+__START)', command).group(1)
end=re.search(r'(__HARMONY_CODEX_[0-9]+_[0-9]+__EXIT)', command).group(1)
print('\n'+start, flush=True)
time.sleep(0.1)
print('validated_dimensions=120x36', flush=True)
print('\n'+end+':7', flush=True)
