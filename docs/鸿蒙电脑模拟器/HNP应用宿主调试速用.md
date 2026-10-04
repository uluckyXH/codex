# HNP 应用宿主调试速用

本说明对应 `platform` 通用目录与双终端版本。包内CLI不绑定UID；实际版本和摘要以同一release中的交付说明为准。2026-10-04已在鸿蒙7模拟器桌面 `1` 完成真实模型对话、命令及文件增删改查；受限执行仍有系统能力限制，详见下文。商业PC的HiShell由用户另行验收。

## 安装与打开

在 Mac 的 zsh 中执行。把路径提示填为**本次新 release 内包含 HAP 的目录**：

```zsh
: '允许粘贴命令中的中文注释'
setopt INTERACTIVE_COMMENTS
# 输入这次交付的 HAP 所在绝对目录
read -r 'harmony_release?请输入新 HAP 所在目录：'
# 校验成功才覆盖安装；保留应用，不要先卸载
(cd "$harmony_release" && shasum -a 256 -c codex-harmony-terminal-debug.hap.sha256 && \
"/Volumes/MacSSD/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc" -t 127.0.0.1:5555 install -r codex-harmony-terminal-debug.hap)
# 打开应用；这个动作不会主动重启已有终端
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" open
```

模拟器构建前关闭，构建结束后才启动；调试后没有下一次编译时保持打开。HAP 是模拟器调试容器，里面的原生 ELF 保留签名；不等于已取得商业 PC 正式应用分发证书。正式 HiShell 入口使用[新版安装与运行说明](../鸿蒙电脑原生适配/新版安装与运行说明.md)，不需要此 GUI。

## 首次准备与两个终端

1. 点击“准备目录”，由原生 CLI 的离线初始化命令建立 `codex/state`、`r`、`tmp`、`logs`。点“查看诊断”可读具体失败阶段。
2. 新应用的 `base/files` 若为0777，严格初始化会拒绝。可以显式点“收紧本应用目录”：只处理当前 ApplicationContext 确认且由自己拥有的 `base/files`，记录原始模式后重试。不修改HOME或项目，也不是独立HiShell的权限修复能力；其他来源/归属错误不可照此处理。
3. “打开空白终端”不需要API配置。可直接输入 `pwd`、`cd`、`rg --version`、`codex --version`；在Shell里退出Codex会回到Shell提示符。
4. 工作目录输入框留空时用 `codex/workspace`；外部项目按下面步骤选择授权，再点“在此目录启动 Codex”，模型固定 `gpt-5.6-terra`。
5. 两个页签是独立会话。“中断当前任务”发送终端中断；“退出 Codex”结束独立Codex会话；“关闭当前终端”关闭所选会话及已确认所属子进程。以状态显示“已退出，子进程清理已确认”为完成；屏幕保留旧输出不表示进程仍活着。

权限默认为只读。选择器仅影响新Codex会话，不替鸿蒙授予文件权限；受限后端缺少内核能力时不会自动切换完全访问。需要另行安装的Git、Node.js等也不会因获得完全访问而自动出现。

## 在桌面项目中工作

1. 点“选择项目文件夹”，选择桌面 `1` 或自己的项目文件夹。
2. 需要编辑时，点“允许读写所选项目”，在系统窗口核对路径并点“授权”。仅输入路径不等于授权；应用关闭后按提示重新选择。
3. 选择新会话的权限模式，再点“在此目录启动 Codex”。本批桌面文件操作验证使用明确选择的“完全访问”；该模式仍受应用系统权限约束。
4. 直接在黑色终端里输入，或用上方输入框点“发送”。上方输入框用于单行命令/对话；粘贴代码等多行内容使用终端本身。
5. 也可以“打开空白终端”，用 `cd` 进入有权访问的目录再执行 `codex`。退出Codex后回到Shell。每个页签的关闭按钮只针对当前会话。

顶部“启动权限”记录启动时模式；若在Codex内使用 `/permissions` 修改，以会话内 `/status` 的有效设置为准。

**当前限制：** 在此模拟器的普通应用环境中，只读受限 `pwd` 返回 `bwrap: Can't read /proc/sys/kernel/overflowuid: Permission denied`，没有静默升级权限。“项目可写”同样依赖受限执行后端，不能保证可用。完全访问下已通过桌面文件操作，不代表受限沙箱可用，也不代表能访问其他应用的私有数据。

V8 Code Mode警告表示缺少用于JavaScript组织工具调用的运行组件；此版使用直接工具调用，已验证 `exec_command` 和 `apply_patch`。它不等于文件权限拒绝。包内 `rg` 可用，未编译PCRE2扩展。

## 手动导入配置

必要配置手动导入新目录，不迁移旧会话和项目。先在GUI关闭两个终端，再执行：

```bash
# 限制本机配置文件访问权限；它不进入Git或HAP
chmod 600 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml"
# 导入到当前应用的 codex/state/config.toml；此操作会重启调试应用
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" import-config --config "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml"
```

脚本要求 `proxy`、`responses`、HTTPS和`gpt-5.6-terra`；不打印URL或Key，最终文件模式0600。没有API Key也可以先验收空白Shell；本轮不测试账号登录。

## 日志与目录

```bash
# 最近一次诊断或导入的结果；它不是PTY实时状态
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" status
# 隐藏配置中的URL、域名和Key后导出TUI日志到新的忽略目录
python3 "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/manage-harmony-app.py" collect-logs --config "/Volumes/MacSSD/Repositories/codex/docs/鸿蒙电脑模拟器/config.toml" --output "/Volumes/MacSSD/Repositories/codex/releases/终端日志-$(date +%Y%m%d-%H%M%S)"
```

| 用途 | 应用命名空间内默认位置 |
| --- | --- |
| 配置与会话 | `/data/storage/el2/base/files/codex/state/` |
| 默认测试项目 | `/data/storage/el2/base/files/codex/workspace/` |
| TUI、进程退出和诊断日志 | `/data/storage/el2/base/files/codex/logs/` |
| aliases与socket | `/data/storage/el2/base/files/codex/r/a/`、`r/s/` |
| GUI控制与配置中转 | `/data/storage/el2/base/files/codex/host/` |

通用源码运行时读取身份，不绑定旧UID。第二测试应用使用 `--app com.codex.emulatorhnp.second` 指定管理目标；同名逻辑目录在两个应用里映射到不同私有数据。两个实际身份的通过证据须看新版验收报告，不能由参数可用推断已通过。
