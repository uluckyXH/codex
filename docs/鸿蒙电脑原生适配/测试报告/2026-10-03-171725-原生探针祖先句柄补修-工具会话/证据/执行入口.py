"""独立探针 O_PATH 变更的限定验证，不启动 Codex、认证或目标 ELF。"""
from datetime import datetime
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import tomllib
from zoneinfo import ZoneInfo

report = Path(__file__).resolve().parents[1]
root = report.parents[3]
evidence = report / '证据'
output = root / '.harmony-build/测试批次' / report.name
sdk = Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native')
case = sys.argv[1]
assert case in ('host', 'sdk')
records = []
def now():
    return datetime.now(ZoneInfo('Asia/Shanghai')).isoformat()
def run(name, command):
    start = now()
    completed = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=120)
    text = completed.stdout + completed.stderr
    (output/'日志'/f'{name}.log').write_text(text)
    (evidence/f'{name}-输出.txt').write_text('\n'.join(line.rstrip() for line in text.splitlines()) + '\n' if text else '退出码 0，无编译器警告。\n')
    records.append({'case':name,'command':command,'cwd':str(root),'started':start,'finished':now(),'exit_code':completed.returncode})
    print(name, completed.returncode, flush=True)
    return completed.returncode
try:
    if case == 'host':
        code = run('宿主01', [sys.executable, '-B', 'scripts/test_harmony_runtime_probe.py'])
    else:
        identity = json.loads((evidence/'源码摘要.json').read_text())
        version = tomllib.loads((root/'codex-rs/Cargo.toml').read_text())['workspace']['package']['version']
        common = [str(sdk/'llvm/bin/clang'), '--target=aarch64-linux-ohos', '--sysroot='+str(sdk/'sysroot'), '-D__MUSL__', '-Wall', '-Wextra', '-Werror']
        ir = output/'产物/SDK打开方式断言.ll'
        code = run('交叉01-打开标志断言', common + ['-std=c11', '-O0', '-I', str(root/'scripts'), '-S', '-emit-llvm', str(evidence/'SDK打开方式断言.c'), '-o', str(ir)])
        if not code:
            text = ir.read_text()
            expected = {name:int(re.search(r'@'+name+r' = .*?constant i32 (\d+)', text).group(1)) for name in ['sdk_ancestor_flags','sdk_readable_directory_flags']}
            calls = {}
            for function in ['inspect_chain', 'create_test']:
                body = re.search(r'define [^\n]*@'+function+r'\([^\n]*\{\n(.*?)\n\}', text, re.S).group(1)
                calls[function] = [line.strip() for line in body.splitlines() if re.search(r'call .*@open(?:at)?\(',line)]
            assert len(calls['inspect_chain']) == 2, calls
            for call in calls['inspect_chain']:
                assert 'i32 noundef '+str(expected['sdk_ancestor_flags']) in call, call
            directory_calls = [call for call in calls['create_test'] if '@openat(' in call]
            assert 'i32 noundef '+str(expected['sdk_readable_directory_flags']) in directory_calls[0], calls
            assert all('i32 noundef '+str(expected['sdk_ancestor_flags']) not in call for call in calls['create_test'])
            (evidence/'目标打开方式.json').write_text(json.dumps({'sdk_constants':expected,'actual_calls':calls,'meaning':'实际 SDK/LLVM IR 静态验证；不是 OHOS O_PATH 运行或权限测试。'},ensure_ascii=False,indent=2)+'\n')
        if not code:
            binary = output/'产物/harmony-runtime-probe'
            code = run('交叉02-C探针链接', common + ['-std=c11', '-O2', '-fPIE', '-pie', '-DCODEX_HARMONY_BUILD_ID="'+identity['head']+'"', '-DCODEX_HARMONY_VERSION="'+version+'"', 'scripts/harmony_runtime_probe.c', '-ldl', '-o', str(binary)])
            if not code:
                sys.path.insert(0, str(root/'scripts'))
                from harmony_elf import inspect_ohos_elf
                result = inspect_ohos_elf(binary)
                (evidence/'ELF摘要.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
                code = run('静态01-导入符号', [str(sdk/'llvm/bin/llvm-nm'), '-D', '--undefined-only', str(binary)])
        records.append({'toolchain':subprocess.check_output([str(sdk/'llvm/bin/clang'),'--version'],text=True),'sdk':json.loads((sdk/'oh-uni-package.json').read_text())})
finally:
    (evidence/f'{case}-执行摘要.json').write_text(json.dumps({'host':platform.platform(),'cases':records},ensure_ascii=False,indent=2)+'\n')
sys.exit(code)
