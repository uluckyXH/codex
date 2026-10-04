from pathlib import Path
import json,os,subprocess,tomllib
root=Path.cwd(); out=root/'.harmony-build/版本号调整/版本验证'; out.mkdir(exist_ok=False)
metadata=json.loads(subprocess.check_output(['cargo','metadata','--manifest-path','codex-rs/Cargo.toml','--locked','--offline','--no-deps','--format-version','1'],text=True))
version=tomllib.loads((root/'codex-rs/Cargo.toml').read_text())['workspace']['package']['version']
packages={p['name']:p['version'] for p in metadata['packages']}
assert packages['codex-cli']==packages['codex-tui']==version=='0.160.0-dev'
(out/'工作区包版本.json').write_text(json.dumps(packages,ensure_ascii=False,indent=2)+'\n')
probe=out/'版本显示回归.rs'
probe.write_text('#[path = '+json.dumps(str(root/'codex-rs/cli/src/harmony_build.rs'))+'] mod cli;\n#[path = '+json.dumps(str(root/'codex-rs/tui/src/version.rs'))+'] mod tui;\n#[test] fn cli_and_tui_share_the_workspace_version() { assert_eq!(cli::cli_version(), tui::CODEX_CLI_VERSION); assert_ne!(tui::CODEX_CLI_VERSION, "0.0.0"); }\n')
env=dict(os.environ,CARGO_PKG_VERSION=version,CODEX_HARMONY_BUILD_ID='version-check',CODEX_CLI_BUILD_TARGET='aarch64-apple-darwin')
subprocess.run(['rustc','--edition=2024','--crate-name','harmony_version_tests','--test',str(probe),'-o',str(out/'版本显示回归')],env=env,check=True)
subprocess.run([str(out/'版本显示回归')],check=True)
print('Cargo metadata 确认 CLI/TUI 统一版本：'+version)
print('执行的是共享版本模块宿主测试，没有运行登录、API 或鸿蒙程序。')
