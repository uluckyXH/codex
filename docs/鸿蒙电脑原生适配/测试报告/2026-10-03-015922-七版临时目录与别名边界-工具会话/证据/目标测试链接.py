#!/usr/bin/env python3
"""仅链接目标测试，不启动目标二进制。"""
from pathlib import Path
import importlib.util
import os
import subprocess

report = Path(__file__).resolve().parents[1]
root = report.parents[3]
spec = importlib.util.spec_from_file_location('harmony_build', root / 'scripts/build_harmony.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
sdk = module.native_sdk(Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native'))
output = root / '.harmony-build/测试批次' / report.name / '鸿蒙检查'
env = module.build_environment(sdk, output, dict(os.environ))
command = ['rustup', 'run', module.TOOLCHAIN, 'cargo', 'test', '--locked', '--offline', '--target', module.TARGET, '--release', '-p', 'codex-arg0', '--lib', '--no-run']
print('仅链接命令：', command, flush=True)
subprocess.run(command, cwd=root / 'codex-rs', env=env, check=True)
