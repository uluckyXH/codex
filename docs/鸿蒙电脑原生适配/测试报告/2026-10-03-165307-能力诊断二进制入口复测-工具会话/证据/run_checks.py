"""记录本批限定的 arg0/doctor 回归与 OHOS 检查，不运行认证或完整 release。"""
from datetime import datetime
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
from zoneinfo import ZoneInfo

report = Path(__file__).resolve().parents[1]
root = report.parents[3]
output = root / '.harmony-build/测试批次' / report.name
case = sys.argv[1]
assert case in ('arg0', 'doctor', 'ohos')
env = os.environ.copy()
env.update(RUSTUP_HOME='/Volumes/MacSSD/dev/rust/rustup',
           CARGO_HOME='/Volumes/MacSSD/dev/rust/cargo', RUSTUP_AUTO_INSTALL='0',
           CARGO_NET_OFFLINE='true', CARGO_BUILD_JOBS='3')
env.pop('CARGO_BUILD_TARGET', None)
env['PATH'] = '/opt/homebrew/opt/rustup/bin:' + env.get('PATH', os.defpath)
(output/'临时目录').mkdir(exist_ok=True)
(output/'空白配置').mkdir(exist_ok=True)
env['TMPDIR'] = str(output/'临时目录')
env['CODEX_HOME'] = str(output/'空白配置')
env['CARGO_TARGET_DIR'] = str(output/'宿主产物')
command = ['rustup', 'run', '1.95.0', 'cargo']
if case == 'ohos':
    sys.path.insert(0, str(root/'scripts'))
    from build_harmony import build_environment, native_sdk
    sdk = native_sdk(Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native'))
    env = build_environment(sdk, output/'鸿蒙检查', env)
    env['CODEX_OHOS_RUNTIME_BASE'] = '/data/storage/el2/base/files'
    command += ['check', '--target', 'aarch64-unknown-linux-ohos', '-p', 'codex-cli', '--bin', 'codex']
else:
    # The workspace has no build.target; remove inherited target and record the
    # actual rustc host. This reuses a private clone of this session's Mac cache.
    command += (['test', '-p', 'codex-arg0', '--lib'] if case == 'arg0' else ['test', '-p', 'codex-cli', '--bin', 'codex'])
command += ['--manifest-path', str(root/'codex-rs/Cargo.toml'), '--locked', '--offline']
if case == 'doctor':
    command += ['doctor::capabilities::', '--', '--nocapture']
if case == 'arg0':
    command += ['--', '--nocapture']
now = lambda: datetime.now(ZoneInfo('Asia/Shanghai')).isoformat()
started = now()
log = output/'日志'/f'{case}.log'
with log.open('x') as stream:
    stream.write('工作目录：'+str(root)+'\n开始：'+started+'\n命令：'+shlex.join(command)+'\n')
    stream.flush()
    done = subprocess.run(command, cwd=root, env=env, stdout=stream, stderr=subprocess.STDOUT)
    stream.write('\n退出码：'+str(done.returncode)+'\n结束：'+now()+'\n')
summary = {'case':case,'command':command,'cwd':str(root),'started':started,'finished':now(),'exit_code':done.returncode,'log':str(log),'host_toolchain':subprocess.check_output(['rustup','run','1.95.0','rustc','-vV'],env=env,text=True),'selected_environment':{key:env[key] for key in ['RUSTUP_HOME','CARGO_HOME','CARGO_TARGET_DIR','TMPDIR','CODEX_HOME','CARGO_NET_OFFLINE','CARGO_BUILD_JOBS']}}
if case == 'ohos': summary['runtime_base'] = env['CODEX_OHOS_RUNTIME_BASE']
(report/'证据'/f'{case}-执行摘要.json').write_text(json.dumps(summary,ensure_ascii=False,indent=2)+'\n')
lines=log.read_text(errors='replace').splitlines()
(report/'证据'/f'{case}-关键输出.txt').write_text('完整原始日志在执行摘要所列本机路径；以下保留末尾最多160行。\n'+'\n'.join(line.rstrip() for line in lines[-160:])+'\n')
print(json.dumps({'case':case,'exit_code':done.returncode,'log':str(log)},ensure_ascii=False))
sys.exit(done.returncode)
