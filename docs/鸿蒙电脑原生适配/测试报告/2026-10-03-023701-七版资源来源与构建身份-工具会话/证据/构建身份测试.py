#!/usr/bin/env python3
"""直接编译实际身份模块及纯函数单测；不启动 Codex CLI。"""
from pathlib import Path
import json
import os
import subprocess

report=Path(__file__).resolve().parents[1]
root=report.parents[3]
run=root/'.harmony-build/测试批次'/report.name
source=run/'identity-harness.rs'
module=json.dumps(str(root/'codex-rs/cli/src/harmony_build.rs'),ensure_ascii=False)
source.write_text(f'#[path = {module}]\nmod harmony_build;\n'+'''#[test]
fn consumes_compile_time_identity() {
    assert_eq!(harmony_build::build_id(), "fixture-pc7-7ec387671");
    assert_eq!(harmony_build::target(), "aarch64-unknown-linux-ohos");
}
''')
env=os.environ.copy()
env.update(CARGO_PKG_VERSION='0.0.0', CODEX_HARMONY_BUILD_ID='fixture-pc7-7ec387671', CODEX_CLI_BUILD_TARGET='aarch64-unknown-linux-ohos')
exe=run/'identity-tests'
for command in [['rustc','--edition=2024','--test',str(source),'-o',str(exe)],[str(exe),'--nocapture']]:
    print('命令：',command,flush=True)
    subprocess.run(command,cwd=root,env=env,check=True)
