# HNP 应用宿主调试速用

适用当前鸿蒙 7 PC 模拟器的 `com.codex.emulatorhnp`。完整原生 Codex 已使用你的配置收到 `gpt-5.6-terra` 回复；交互和文件工具的逐项结果见[验收报告](../鸿蒙电脑原生适配/测试报告/2026-10-04-HNP完整包与原生终端验收-主会话/测试报告.md)。

## 打开已经安装的应用

在 Mac 终端执行：

```bash
# 打开模拟器中的 Codex 应用；不重启正在运行的终端会话。
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" open
```

在应用内点击“启动 Codex”。模型固定为 `gpt-5.6-terra`，初始权限为只读。停止用应用上方“停止终端”。当前镜像的受限命令后端仍缺少内核能力，入口不会自动改成完全权限。

首次出现升级模型提示时，选择 `Use existing model` 保留原模型。输入 `/status` 可核对模型与权限；如果斜杠变成顿号，先把模拟器输入法切到英文。

## 更新本次调试应用

交付目录：`releases/2026-10-04-063742-鸿蒙Codex-HNP应用调试`。HAP、校验文件、中文说明放在一起；个人配置单独保存，不进入交付包。

最终 HAP 已安装并验收。需要覆盖更新时，在 Mac 执行以下命令；保留现有应用，不要先卸载：

```bash
# 进入本次交付目录，后续校验和安装使用同一份 HAP。
cd "/Volumes/MacSSD/Repositories/codex/releases/2026-10-04-063742-鸿蒙Codex-HNP应用调试"
# 核验安装包；&& 确保仅校验成功才覆盖安装，保留已有配置。
shasum -a 256 -c codex-harmony-terminal-debug.hap.sha256 && \
"/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc" -t 127.0.0.1:5555 install codex-harmony-terminal-debug.hap
# 打开已安装应用；配置已存在时不必再次导入。
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" open
```

HAP SHA256：`bad4428c9931df803f2bfeb73b885e12a125c41747357f5850d876c7e39f263b`。主程序版本为 `0.160.0-dev.harmony.g8c4b3a6f48ef`，Rust 源码为 `8c4b3a6f48efa9eb4642d75929de8ba5ebdc6e84`；界面显示其上游版本 `0.160.0-dev`。HAP 外壳版本单独记录为 `0.1.0`。

SDK 自签名原生文件压缩包是本 HNP 构建的输入，不是通用 HiShell 安装包。不要直接在 HDC shell 中运行应用专用二进制。

## 以后修改配置时重新导入

当前配置已经导入，无需重复。以后在 Mac 修改 `config.toml` 后执行：

```bash
# 限制本地密钥配置文件仅当前用户可读写。
chmod 600 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml"
# 将配置独立导入应用私有目录；会重启本调试应用，请先结束终端会话。
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" import-config --config "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml"
```

脚本要求 `proxy`、`responses`、HTTPS 地址和 `gpt-5.6-terra`；不会打印 URL 或 Key。应用最终保存为 `state/config.toml`、权限 `0600`。

## 查看状态与日志

```bash
# 查看最近一次原生诊断或配置导入的退出状态；不显示配置内容。
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" status
# 收集已启动 TUI 的日志，按本地配置隐藏 URL、域名和 Key，输出到新的忽略目录。
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" collect-logs --config "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml" --output "/Volumes/MacSSD/Repositories/codex/releases/终端日志-$(date +%Y%m%d-%H%M%S)"
```

日志包含任务内容时，分享前仍需检查。`status` 是最近一次诊断状态，不是交互会话实时状态。

| 用途 | 应用内路径 |
| --- | --- |
| 配置 | `/data/storage/el2/base/files/state/config.toml` |
| 项目工作区 | `/data/storage/el2/base/files/workspace` |
| TUI 日志（应用启动参数明确指定） | `/data/storage/el2/base/files/state/log/codex-tui.log` |
| 启动、版本和问候诊断 | `/data/storage/el2/base/files/logs/` |
| 受保护运行目录 | `/data/storage/el2/base/files/r` |

这个调试包绑定已验证的应用 UID `20020059`。保留应用做覆盖更新；卸载、换模拟器或换真机后必须重新核验 UID 和签名要求。原生 ELF 使用 SDK 自签名，调试 HAP 在本模拟器的安装不代表已取得商业 PC 的正式分发证书。不会修改整个 HOME 或系统权限。
