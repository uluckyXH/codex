from pathlib import Path
import hashlib,json,os,subprocess
root=Path(__file__).resolve().parents[4]
report=Path(__file__).parent
run=root/'.harmony-build/测试批次'/report.name
source=root/'codex-rs/cli/src/debug_sandbox.rs'
text=source.read_text()
start=text.index('fn handle_debug_sandbox_exit_status(')
end=text.index('#[cfg(all(test, unix))]',start)
function=text[start:end].strip()
assert function.endswith('}'),function
assert text.count('handle_debug_sandbox_exit_status(')==3
assert 'handle_debug_sandbox_exit_status(child.wait().await?);' in text
assert '    handle_debug_sandbox_exit_status(status);' in text
assert text.count('handle_exit_status(')==1
paths={
    'exit_status':root/'codex-rs/cli/src/exit_status.rs',
    'codex_utils_pty':root/'codex-rs/utils/pty/src/diagnostics.rs',
    'exit_status_tests':root/'codex-rs/cli/src/debug_sandbox/exit_status_tests.rs',
}
harness=run/'宿主产物/退出状态回归.rs'
content='#![allow(dead_code)]\n'
for name,path in paths.items():
    content+=f'#[path = {json.dumps(str(path),ensure_ascii=False)}]\nmod {name};\n'
content+='use exit_status::handle_exit_status;\n'+function+'\n'
harness.write_text(content)
executable=run/'宿主产物/退出状态回归'
commands=[['rustup','run','1.95.0','rustc','--edition','2024','--test','--crate-name','sandbox_exit_regression',str(harness),'-o',str(executable)],
    [str(executable),'standalone_sandbox_exit_diagnostics_keep_native_status_and_redact_payloads','--nocapture']]
for command in commands:
    print('执行命令：'+json.dumps(command,ensure_ascii=False),flush=True)
    subprocess.run(command,check=True)
(report/'宿主入口摘要.json').write_text(json.dumps({'生产函数来源':str(source.relative_to(root)),'生产函数SHA256':hashlib.sha256(function.encode()).hexdigest(),'源文件引用':{str(path.relative_to(root)):hashlib.sha256(path.read_bytes()).hexdigest() for path in paths.values()},'入口SHA256':hashlib.sha256(harness.read_bytes()).hexdigest(),'产物SHA256':hashlib.sha256(executable.read_bytes()).hexdigest(),'命令':commands,'边界':'复用生产源码与仓库测试模块的独立宿主 harness；不执行完整 CLI、配置加载、认证或实际沙箱'},ensure_ascii=False,indent=2)+'\n')
