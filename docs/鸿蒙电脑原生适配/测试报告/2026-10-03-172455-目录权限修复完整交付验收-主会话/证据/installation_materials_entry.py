from pathlib import Path
import hashlib,json,shutil
root=Path.cwd()
release=root/'releases/2026-10-03-鸿蒙Codex-目录权限修复候选'
summary=json.loads((release/'交付摘要.json').read_text())
record=json.loads((release/'鸿蒙Codex/构建与签名记录.json').read_text())
source=release/summary['归档']
folder=release/'鸿蒙Codex安装'
folder.mkdir(exist_ok=False)
shutil.copyfile(source,folder/source.name)
assert hashlib.sha256((folder/source.name).read_bytes()).hexdigest()==summary['SHA-256']
shutil.copyfile(release/'安装包校验.sha256',folder/'安装包校验.sha256')
for name in ('账号登录安装速用.md','接口密钥安装速用.md','七版修复包安装与复测.md','升级与专项日志速用.md'):
    shutil.copyfile(release/'鸿蒙Codex'/name,folder/name)
    assert (folder/name).read_bytes()==(root/'docs/鸿蒙电脑原生适配'/name).read_bytes()
(folder/'资料说明.md').write_text('''# 鸿蒙 PC 七版目录权限修复候选安装资料

已有安装：把本目录整体复制到鸿蒙主目录，命名为“鸿蒙Codex更新-目录权限修复”，按《升级与专项日志速用.md》升级。首次安装：命名为“鸿蒙Codex安装-目录权限修复”，选择接口密钥或账号登录速用。已有配置继续使用，不必重新填写 Key。先退出旧 Codex、daemon 及其受限子进程，本轮不支持新旧实例并行。

CLI 版本：`0.160.0-dev`。官方更新检查、提醒和执行继续禁用，没有跟进上游新版本源码。

包版本：`'''+summary['版本']+'''`。

完整源码 SHA：`'''+record['源码']['提交']+'''`。

压缩包：`'''+source.name+'''`。

SHA256：`'''+summary['SHA-256']+'''`。

本包将 aliases 与控制 socket 放入固定私有目录候选 `/data/storage/el2/base/files` 下的不同用途目录，保留严格校验，不修改 HOME/.codex 权限。该路径尚无本机探针证明可用；如果拒绝，独立探针可在 Codex 初始化之前采集目录与身份信息。不依赖 id，不读取账号配置。

已完成 Mac 交叉构建、SDK 自签名、独立解包、摘要与文件安装检查；没有在 Mac 运行鸿蒙 ELF。目录可用性、真实隔离、此前终端 159 和文件写入故障均仍需同一设备复测，不能将本包标记为真机验收通过。

压缩包内含四个已签名 ELF：codex、rg、bwrap、harmony-runtime-probe，以及安装器、终端配置器、诊断脚本、四份速用文档、构建记录与文件校验清单。不要单独替换程序或复制未签名中间产物。

本机 releases 目录由 Git 忽略；源码、计划和测试报告已保存在仓库 docs/鸿蒙电脑原生适配/。只需传本资料目录，不必传编译缓存、SDK、Rust。当前流程不创建或上传 GitHub Release。
''')
print(folder)
for path in sorted(folder.iterdir()):print(path.name,path.stat().st_size)
