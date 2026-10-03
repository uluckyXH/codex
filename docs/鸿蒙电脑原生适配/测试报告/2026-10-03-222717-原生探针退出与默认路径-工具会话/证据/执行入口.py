"""只验证独立探针；不启动 Codex、认证、模型或目标 ELF。"""
from datetime import datetime
import hashlib
import json
from pathlib import Path
import platform
import re
import subprocess
import sys
import tomllib
import traceback
from zoneinfo import ZoneInfo

sys.dont_write_bytecode = True
report = Path(__file__).resolve().parents[1]
root = report.parents[3]
evidence = report / '证据'
output = root / '.harmony-build/测试批次' / report.name
sdk = Path('/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native')
case = sys.argv[1]
assert case in ('host', 'sdk')
records = []
code = 1

def now():
    return datetime.now(ZoneInfo('Asia/Shanghai')).isoformat()

def run(name, command):
    start = now()
    completed = subprocess.run(command, cwd=root, capture_output=True, text=True, timeout=120)
    content = completed.stdout + completed.stderr
    log = output / '日志' / (name + '.log')
    with log.open('x') as stream:
        stream.write(content)
    with (evidence / (name + '-输出.txt')).open('x') as stream:
        stream.write('\n'.join(line.rstrip() for line in content.splitlines()) + '\n' if content else '命令未输出文字。\n')
    records.append({'case': name, 'command': command, 'cwd': str(root), 'started': start,
                    'finished': now(), 'exit_code': completed.returncode})
    print(name, completed.returncode, flush=True)
    if completed.returncode:
        raise RuntimeError(f'{name} exit {completed.returncode}; see {log}')
    return content

try:
    identity = json.loads((evidence / '源码摘要.json').read_text())
    if case == 'host':
        run('宿主01', [sys.executable, '-B', 'scripts/test_harmony_runtime_probe.py'])
        records.append({'compiler': subprocess.check_output(['/usr/bin/clang', '--version'], text=True),
                        'python': sys.version})
    else:
        version = tomllib.loads((root / 'codex-rs/Cargo.toml').read_text())['workspace']['package']['version']
        common = [str(sdk / 'llvm/bin/clang'), '--target=aarch64-linux-ohos',
                  '--sysroot=' + str(sdk / 'sysroot'), '-D__MUSL__', '-Wall', '-Wextra', '-Werror']
        contract = '-DCODEX_OHOS_RUNTIME_BASE="/data/storage/el2/base/files"'
        run('交叉01-C无宏门禁', common + ['-std=c11', '-fsyntax-only', 'scripts/harmony_runtime_probe.c'])
        run('交叉01-C++接口门禁', common + ['-x', 'c++', '-std=c++17', '-fsyntax-only', 'scripts/harmony_runtime_probe_sdk_check.cpp'])
        ir = output / '产物/SDK门禁.ll'
        run('交叉01-C含宏与IR门禁', common + ['-std=c11', contract, '-O0', '-I', str(root / 'scripts'), '-S', '-emit-llvm', str(evidence / 'SDK门禁.c'), '-o', str(ir)])
        content = ir.read_text()
        expected = {name: int(re.search(r'@' + name + r' = .*?constant i32 (\d+)', content).group(1))
                    for name in ('sdk_ancestor_flags', 'sdk_readable_directory_flags')}
        calls = {}
        for function in ('inspect_chain', 'create_test', 'native_context', 'native_worker_exit'):
            body = re.search(r'define [^\n]*@' + function + r'\([^\n]*\{\n(.*?)\n\}', content, re.S).group(1)
            calls[function] = [line.strip() for line in body.splitlines()
                               if re.search(r'call .*@(?:open|openat|dlopen|dlclose|fflush|fclose|_Exit|exit)\(', line)]
        assert len(calls['inspect_chain']) == 2
        assert all('i32 noundef ' + str(expected['sdk_ancestor_flags']) in call for call in calls['inspect_chain'])
        assert any('@openat(' in call and 'i32 noundef ' + str(expected['sdk_readable_directory_flags']) in call for call in calls['create_test'])
        assert '@dlclose(' not in content
        assert any('@_Exit(' in call for call in calls['native_worker_exit'])
        (evidence / '目标生命周期与打开方式.json').write_text(json.dumps({'sdk_constants': expected, 'actual_calls': calls, 'meaning': '实际 SDK 的静态 IR；不是 OHOS 运行测试。'}, ensure_ascii=False, indent=2) + '\n')
        binary = output / '产物/harmony-runtime-probe'
        run('交叉02-C探针链接', common + ['-std=c11', contract, '-O2', '-fPIE', '-pie',
            '-DCODEX_HARMONY_BUILD_ID="' + identity['head'] + '"', '-DCODEX_HARMONY_VERSION="' + version + '"',
            'scripts/harmony_runtime_probe.c', '-ldl', '-o', str(binary)])
        sys.path.insert(0, str(root / 'scripts'))
        from harmony_elf import inspect_ohos_elf
        elf = inspect_ohos_elf(binary)
        (evidence / 'ELF摘要.json').write_text(json.dumps(elf, ensure_ascii=False, indent=2) + '\n')
        imports = run('静态01-导入符号', [str(sdk / 'llvm/bin/llvm-nm'), '-D', '--undefined-only', str(binary)])
        assert re.search(r'\bU _Exit\b', imports)
        assert not re.search(r'\bU dlclose\b', imports)
        records.append({'toolchain': subprocess.check_output([str(sdk / 'llvm/bin/clang'), '--version'], text=True),
                        'sdk': json.loads((sdk / 'oh-uni-package.json').read_text()),
                        'elf_sha256': hashlib.sha256(binary.read_bytes()).hexdigest()})
    actual = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in identity['files']}
    assert actual == identity['files'], 'source changed during tests'
    code = 0
except BaseException:
    details = traceback.format_exc()
    (evidence / (case + '-未完成说明.txt')).write_text(details)
    print(details, file=sys.stderr)
finally:
    (evidence / (case + '-执行摘要.json')).write_text(json.dumps({'host': platform.platform(), 'cases': records, 'runner_exit_code': code}, ensure_ascii=False, indent=2) + '\n')
sys.exit(code)
