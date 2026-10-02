# 鸿蒙PC 7.0 Codex 安装与运行故障报告

**提交对象  uluckyXH/codex 项目开发者**

测试日期  2026年10月2日至3日  |  整理截至 10月3日01:23  |  时区 UTC+8

我在 HarmonyOS PC 7.0 的 ARM64 真机上安装了原生候选包。安装校验、主程序启动均已通过；指定系统 CA 文件后，第三方 API 已能正常回答，并能看到服务端请求记录。目前仍无法完成本地编码任务：终端命令退出码为 159，旧沙箱模式下文件写入工具也失败；独立沙箱测试还出现只读文件系统错误和 legacy Landlock 的显式 panic。

请优先处理本地工具与沙箱的真机兼容问题，并将临时目录、证书和环境持久化的已知补救步骤纳入安装或诊断流程。本文保留失败步骤和证据边界，供复现、分派与回归验收使用。

### 当前验收结论

| 能力 | 结果 | 说明 |
| --- | --- | --- |
| 包校验与安装 | 通过 | 归档校验及安装过程中的文件校验显示 OK |
| CLI 与交互界面 | 通过 | 可显示 codex-cli 0.0.0 并进入会话 |
| 第三方模型请求 | 有条件通过 | 设置 CODEX_CA_CERTIFICATE 后实际回答成功 |
| 本地终端与文件写入 | 阻塞 | 终端退出 159；旧沙箱下写入工具失败 |
| 受限沙箱完整可用性 | 未通过 | bubblewrap 构建错误；legacy 路径被策略保护拒绝 |

### 建议开发者建立的问题单

| 编号 | 问题 | 建议优先级 |
| --- | --- | --- |
| HM-01 | 临时目录被判为 HOME，无法创建 PATH aliases | P2 |
| HM-02 | 默认 TLS 请求失败，显式 CA 后恢复 | P1 |
| HM-03 | 本地工具执行失败  包含终端 159 与文件写入失败 | P1 |
| HM-04 | 沙箱构建报 EROFS，疑似固定 /tmp 运行目录不适配 | P1 |
| HM-05 | legacy Landlock 不兼容策略时以 panic 退出 | P2 |
| HM-06 | 打包资源提示、环境持久化及版本诊断信息不足 | P2 / P3 |

P1 表示阻塞正常使用或核心功能；P2 表示高频安装与排障障碍；P3 表示可观测性改进。编号是本报告的建议分组，不代表仓库已有 issue。HM-03 按用户要求合并记录两种工具失败现象，尚未证明它们与 HM-04 属于同一根因。

## 阅读导航

- [测试环境与安装过程](#1-测试环境与安装过程)
- [HM-01 临时目录与环境持久化](#2-hm-01-临时目录与环境持久化)
- [HM-02 第三方请求持续重连](#3-hm-02-第三方请求持续重连)
- [API 配置与日志排障](#4-api-配置与日志排障中的误判风险)
- [HM-03 本地终端与文件写入失败](#5-hm-03-本地终端与文件写入失败)
- [HM-04 bubblewrap 只读文件系统错误](#6-hm-04-bubblewrap-构建时只读文件系统)
- [HM-05 旧沙箱回退被策略保护拒绝](#7-hm-05-旧沙箱回退被策略保护拒绝)
- [HM-06 启动提示与安装体验](#8-hm-06-启动提示与安装体验)
- [建议修复顺序](#9-建议修复顺序与开发者交付物)
- [回归测试与最小采集](#10-回归测试与下一轮最小采集)
- [证据索引与源码链接](#11-证据索引与源码链接)

## 1 测试环境与安装过程

| 项目 | 已知信息 |
| --- | --- |
| 设备与系统 | 鸿蒙 PC，系统 7.0；具体型号、完整系统构建号及内核版本尚未采集 |
| CPU 架构 | aarch64 |
| 实际用户目录 | /storage/Users/currentUser  注意 Users 大小写 |
| 终端与命令 | HiShell / zsh；已确认 /usr/bin/sh、/usr/bin/zsh、/usr/bin/tar |
| 安装目录 | /storage/Users/currentUser/codex-hm |
| 归档位置 | /storage/Users/currentUser/Download/鸿蒙Codex安装 |
| 安装项目 | uluckyXH/codex 的 codex/harmony-native 原生移植版 |
| 候选包身份 | 0.0.0-harmony.bc1a10fa191c；文件名带“未真机验证” |
| 构建目标 | aarch64-unknown-linux-ohos  来自项目构建记录，非现场编译 |

曾比较过 QinpanWan/codex-harmonyos 的 Linux ARM64 musl 二进制签名路线，但本次安装与问题反馈针对 uluckyXH/codex 原生版本，不应混用两个项目的构建假设。

### 安装操作与结果

```text
sha256sum -c *.sha256 && tar -xzf *.gz
sh *Codex/*.sh --prefix ~/codex-hm
~/codex-hm/bin/codex --version
```

用户反馈归档校验为 OK；安装输出中的 codex、rg、bwrap 校验通过。主程序能够运行，返回版本号 0.0.0。该版本号来自候选构建自身的版本标识，不能据此判断安装失败。上述通配符命令是当时目录内容下的操作记录，不宜直接当作通用安装脚本。

### 从安装到阻塞的顺序

| 阶段 | 结果或变化 |
| --- | --- |
| 安装后首次启动 | 出现临时目录 / PATH aliases 警告 |
| 指定 TMPDIR | 版本命令只输出版本号；关闭终端后环境丢失问题再次出现 |
| 配置第三方 API | 进入 TUI，出现 bubblewrap 与 V8 能力警告；请求持续重连 |
| 采集日志与 curl 对照 | curl TLS 校验通过；Codex 请求仍失败；期间纠正 $HOM 输入错误 |
| 显式指定系统 CA | 用户确认回答成功且第三方有请求记录 |
| 测试本地工具 | pwd 返回退出码 159；直接 sh 执行 pwd 正常 |
| 测试沙箱及旧模式 | bubblewrap 构建报 EROFS；legacy 测试触发显式 panic |
| 最新补充 | 旧沙箱模式下，生成代码的文件写入工具失败，终端仍退出 159 |

时间线按会话先后整理，未给未留存精确时刻的步骤补造时间。关闭终端、修改配置及重复尝试均沿用同一安装，不代表重新安装了其他二进制。

## 2 HM 01 临时目录与环境持久化

### 实际现象

首次运行 --version 时，程序能够输出版本号，但创建 PATH aliases 失败。以下警告按阅读宽度换行，内容来自启动输出：

```text
WARNING: proceeding, even though we could not create PATH aliases:
Refusing to create helper binaries under temporary dir
"/storage/Users/currentUser"
(codex_home: AbsolutePathBuf("/storage/Users/currentUser/.codex"))
```

### 已经验证的补救

```text
mkdir -p ~/tmp
TMPDIR=~/tmp ~/codex-hm/bin/codex --version
```

用户随后反馈“返回一行版本号”。这证明至少在该次启动环境中，指定独立的可写临时目录可以消除这条警告。用户关掉终端重新打开后，直接执行完整路径又出现 warning + 0.0.0，与此前只对当前 shell 或单次命令设置环境变量相符。

### 源码定位与解释

候选提交的 codex-rs/arg0/src/lib.rs 在 release 构建中读取 std::env::temp_dir()，如果 codex_home 位于该临时根目录内，就拒绝创建 helper binaries。现场警告显示本次临时根被解析为整个用户目录，而默认 .codex 又位于其下，因此触发安全检查。随后用于 aliases 的目录实际是 codex_home/tmp/arg0。[S2]

该检查有安全目的，不能简单删除。应修复 OHOS 环境中临时目录选择或安装环境初始化，使受信任的用户配置目录不会因“临时根等于整个 HOME”而被误分类。

### 建议修复与验收

- 安装器为当前用户创建并检查独立、可写的临时目录；检查路径、所有者与权限，不把整个 HOME 当成临时根。
- 提供短小的环境激活命令，或经用户选择写入带标记、可重复执行的 zsh 配置块。重复安装不应不断追加相同 export，也不应覆盖用户既有配置。
- 在全新终端中验证 codex --version、直接启动及工具调用；既检查 aliases 是否创建，也检查 helper 是否真的能运行。警告消失不等于整个沙箱可用。

### 本次提供过的持久化内容

```text
export TMPDIR=~/tmp
export PATH=~/codex-hm/bin:$PATH
```

上述内容曾指导写入 ~/.zshrc，并通过 source 重新加载；报告没有回读最终 ~/.zshrc，不能声称已审计持久化文件。与网络修复相关的 CODEX_CA_CERTIFICATE 另见下一节。

## 3 HM 02 第三方请求持续重连

### 现象与对照

交互界面持续出现 “Reconnecting... waiting for network” 和 “Connection failed: error sending request”。用户说明同一第三方服务在这台电脑的其他 agent 中可用，Mac 上 Codex 也使用同类配置。网络日志显示请求发送失败；尚未拿到模型接口返回的 HTTP 业务错误码。

日志显示模型请求使用 responses_http，Authorization header 已附加。未配置 env_key、某些鉴权环境变量为 false，不能推出“没有 API Key”，因为该用户实际使用 TOML 中的 experimental_bearer_token。

### curl 证据

```text
curl -vI -m 15 https://abc.com/v1/
# 关键输出（域名及 IP 已脱敏）
* Connected to [provider] port 443
* CAfile: /etc/ssl/certs/cacert.pem
* CApath: /system/etc/security/certificates/
* SSL connection using TLSv1.3 / TLS_AES_128_GCM_SHA256
* SSL certificate verify ok.
< HTTP/1.1 404 Not Found
```

这里的 404 来自对 /v1/ 的 HEAD 请求，说明 curl 已完成 DNS、连接、TLS 校验并收到服务器响应。它不等于 /v1/responses 的 POST 请求成功，也不证明 Codex 使用了相同的 CA 或 TLS 后端。无需据此把 base_url 改成 /responses。

### 已经验证的补救

```text
CODEX_CA_CERTIFICATE=/etc/ssl/certs/cacert.pem codex
```

用户明确反馈：“生效了，回答了，请求也有记录了。” 因此该设置在这台设备上已经恢复一次真实模型请求。曾提供将以下内容写入 ~/.zshrc 的持久化办法：

```text
export CODEX_CA_CERTIFICATE=/etc/ssl/certs/cacert.pem
```

### 定位结论与尚未证明的部分

codex-rs/http-client/src/custom_ca.rs 支持 CODEX_CA_CERTIFICATE，并优先于 SSL_CERT_FILE。更关键的是，配置自定义 CA 会调用 builder.use_rustls_tls()，同时装载该证书包。[S3] 本次成功可能与信任根发现、默认 TLS 后端，或二者共同作用有关；不能只凭这次对照就写成“已证明唯一原因是缺少证书”。

建议在 OHOS 上明确系统 CA 的发现和 TLS 后端选择，保留证书校验；诊断输出应区分 DNS、TCP、TLS 证书链、握手、代理及 HTTP 状态。验证显式 CA、默认环境、无效路径、损坏 PEM、证书轮换及不同证书链。当前证据不涵盖其他遥测或插件请求是否一并恢复。

## 4 API 配置与日志排障中的误判风险

### 用户提供的配置脱敏转录

```text
approvals_reviewer = "user"
model = "gpt-5.6-terra"
model_provider = "proxy"
service_tier = "default"
web_search = "live"
model_reasoning_effort = "max"
personality = "pragmatic"
plan_mode_reasoning_effort = "xhigh"

[model_providers.proxy]
base_url = "https://abc.com/v1"
requires_openai_auth = false
name = "OpenAI"
wire_api = "responses"
experimental_bearer_token = "<REDACTED>"
```

这是用户粘贴的 Mac 风格配置，不是对最终生效文件的完整审计。后续截图中的 grep 结果显示 model_reasoning_effort = "high"；两处不同应保留，不应猜测哪次修改导致连接变化。模型名称为用户的配置字符串，未核验第三方实际映射到的模型。

候选源码支持 experimental_bearer_token；provider 的 name 是显示名称，不能据此断定请求被发往 OpenAI 官方地址。[S4] 对话中曾提供另一份基于环境变量的模板，用户并未照该模板配置，因此不能以其变量是否存在来诊断当前鉴权。粘贴消息中的反斜杠、Markdown 链接包装也未被证明真的写进 TOML 文件。

### 日志采集与两次路径错误

```text
RUST_LOG=debug codex -c "log_dir='$HOME/tmp/cxlog'"
grep -i error ~/tmp/cxlog/codex-tui.log | tail -n 8
```

这是当时使用的日志采集方式。开始尝试相对 log_dir 时，用户报告 Permission denied (os error 13)，原始完整命令与目标路径未留全，原因未定。随后出现的第一次 Read-only file system (os error 30)，经截图发现 $HOME 被少打成 $HOM；用户明确承认并纠正，之后可以进入 Codex 看日志。该次 OS 30 应记为输入错误，不能与 HM-04 的独立沙箱报错合并归因。

### 日志中的伴随信息

还观察到遥测地址连接被拒绝、插件仓库或 Git 相关同步失败，以及退出清理时的 app-server channel closed。这些信息与模型请求失败同处日志，但尚无证据表明它们是本次模型连接故障或工具退出 159 的根因。建议诊断工具按“模型传输”“可选同步”“退出清理”标记来源，保留完整错误链而不是只显示 error sending request。

报告只保留脱敏文本，未嵌入含真实域名、IP 或桌面内容的整张截图。复现不需要把真实 API Key 发给开发者。

## 5 HM 03 本地终端与文件写入失败

本节合并记录用户要求归并的现象：首次 pwd 失败，以及后来在“旧的沙箱模式”下生成代码时文件写入工具失败、终端仍返回 159。它们共同阻塞实际编码；目前只有终端明确提供退出码，文件写入失败的底层错误与工具名称尚未留存。

### 首次本地命令调用

```text
我来执行 `pwd`。
`pwd` 执行失败（退出码 159），没有输出。环境提供的当前目录为：
/storage/Users/currentUser
```

上面的目录来自会话环境信息，不是成功执行 pwd 的输出。随后在系统终端直接运行相同 shell 命令成功：

```text
/usr/bin/sh -c pwd
/storage/Users/currentUser
```

### 用户最新补充

“生成的代码在这个旧的沙箱模式里，会提示文件写入工具失败，终端无法运行，退出码159。” 用户随后确认应与之前的问题合并。此处“旧的沙箱模式”沿用用户描述；尚未取得这次交互会话的完整启动命令、生效 sandbox_mode、feature 状态及原始工具调用记录，不能直接断言它与下一节独立 CLI 测试拥有完全相同的启动路径。

### 影响与定位边界

- 模型已经能回答，但无法依赖本地工具完成创建代码文件、执行命令与验证结果的闭环；不能将当前状态标为“编码可用”。
- 159 在常见 shell 约定中可能对应 128 + 31，即 SIGSYS；这里只是排查线索。尚未取得原始 wait status、信号名称或触发的 syscall，不能宣称已确认 seccomp 杀进程。
- 直接 shell 成功，说明 /usr/bin/sh 和 pwd 在普通终端环境中可用；仍需分别检查工具宿主、PTY、子进程启动、沙箱重入与策略应用。
- HM-04 的独立 sandbox 测试在构建 bubblewrap 命令时失败，该分支源码以退出码 1 退出。它与 159 可能有关，但不能在缺少同一次调用链证据时直接认定为同一错误。

### 建议的最小复现与所需证据

请开发者在专用测试目录内，通过相同会话分别调用终端工具执行 pwd、调用实际的文件写入工具创建一个小文本、再读取验证。记录原始工具名、参数、cwd、审批策略、文件系统策略、PTY 开关、启动器与 helper 路径、失败阶段及原始退出状态。不要只收集模型转述的“失败”。

验收必须同时覆盖终端工具和文件写入工具。若二者共享同一沙箱或 helper 故障，修复后统一关闭 HM-03；若实际涉及不同执行通道，可在同一问题下建立子项，避免丢失本次合并反馈。

## 6 HM 04 bubblewrap 构建时只读文件系统

### 可直接复现的命令与输出

```text
/usr/bin/sh -c pwd
/storage/Users/currentUser

codex sandbox -- /usr/bin/sh -c pwd
error building bubblewrap command: Read-only file system (os error 30)
```

这是纠正此前输入错误之后的独立测试，属于真实待修复阻塞。现有输出将失败定位在构建 bubblewrap 命令阶段，不能据此宣称 bwrap 已进入 namespace 或目标 shell 已执行。候选版本实际接受的命令语法如上。

### 高置信度的源码线索

codex-rs/linux-sandbox/src/bwrap.rs 的 create_filesystem_args 调用 prepare_shared_daemon_socket_directory()，并通过 ? 传播失败。该函数在 codex-rs/uds/src/daemon_directory.rs 中固定使用 /tmp，随后创建按 UID 区分的目录。[S5][S6]

```text
// daemon_directory.rs  节选
let temporary_root = fs::canonicalize("/tmp")?;
let uid = unsafe { libc::geteuid() };
Ok(temporary_root.join(format!("codex-daemon-{uid}")))

// 随后准备目录
fs::DirBuilder::new().mode(0o700).create(&directory)
```

这与该设备上返回 EROFS 高度相符：若 /tmp 指向只读位置，创建 `codex-daemon-<uid>` 会失败。但是本次没有采集 syscall 跟踪、/tmp 挂载信息或实际失败路径，因此这是源码支持的高置信度候选根因，仍需开发者真机确认。

设置 TMPDIR=~/tmp 对这个固定路径没有作用。源码注释明确要求此共享目录不依赖 HOME、TMPDIR、CODEX_HOME 或命令设置，因此不能把 HM-01 的临时目录补救视为本问题的完整修复。

### 修复应保留的安全条件

此目录用于特权 app-server RPC socket；所有监听者与沙箱必须对同一个保留路径达成一致，沙箱在 daemon 尚未启动时也需要隐藏它。当前实现检查目录类型、当前 UID 所有权和精确 0700 权限，并拒绝不安全的现有对象。

- 为 OHOS 选择可验证、稳定、用户隔离的运行目录，以可信平台或账号信息确定位置；同步修改 daemon 与各沙箱的目录解析和屏蔽逻辑。
- 继续验证所有者、权限、非符号链接、并发创建以及路径别名；检查 UNIX socket 路径长度与清理行为，不能只把 /tmp 替换成任意环境变量。
- 在报错中附上操作、解析后的路径、errno 和执行阶段，便于确认究竟在哪一步失败。
- 修复目录后再验证 bwrap 签名与加载、描述符执行、namespace、mount、proc 与 seccomp 等能力；路径修复通过不代表后续隔离机制已经在鸿蒙可用。

## 7 HM 05 旧沙箱回退被策略保护拒绝

### 实际执行与结果

```text
codex --enable use_legacy_landlock sandbox -- /usr/bin/sh -c pwd

thread 'main' (4894) panicked at
linux-sandbox/src/linux_run_main.rs:419:9:
filesystem-restricted execution requires bubblewrap
 to isolate app-server sockets
note: run with `RUST_BACKTRACE=1` environment variable
 to display a backtrace
```

长行在此按版面换行。用户没有进一步运行 RUST_BACKTRACE=1，也没有提供由此产生的回溯。该参数来自排障中的一次临时尝试，并未证明已永久写入配置文件。

### 源码已经明确的行为

codex-rs/linux-sandbox/src/linux_run_main.rs 的 ensure_legacy_landlock_mode_supports_policy 在约第 414–420 行检查：启用 legacy Landlock 且策略不具有全盘写入权限时，直接触发上述 panic。[S7]

```text
if use_legacy_landlock
    && !file_system_sandbox_policy.has_full_disk_write_access()
{
    panic!("filesystem-restricted execution requires bubblewrap to isolate app-server sockets");
}
```

上段为相关条件和错误消息的源码节选。这里表明该回退在当前策略下被程序主动拒绝；不是已经证明“鸿蒙内核不支持 Landlock”。原因是受限执行仍需 bubblewrap 隔离 app-server socket。排障时推荐该参数未先核对这条保护，属于无效尝试，不能当作可用解决办法。

### 需要修复的部分

应保留隔离约束，把可预期的不兼容组合变成清晰的参数或策略错误，尽早告知哪些能力缺失、如何使用受支持的候选包及诊断命令，避免用户遇到线程 panic 后继续试错。平台能力探测与 feature 提示应一致。

不要为了绕过该报错而删除保护、自动切换无沙箱执行，或将全盘写入作为安装前置条件。本报告未通过解除隔离来取得“成功”结果，也不把这条临时命令列为用户的长期配置。

### 与 HM 03 的关系

本节是独立 CLI 沙箱测试的已知 panic；HM-03 是交互式工具返回 159 和文件写入失败的合并记录。二者都发生于沙箱排障阶段，但应保留各自原始输出、进程和策略上下文。下一步由开发者通过同一构建的诊断日志建立调用链，而不是按“都失败了”合并成一个已确定根因。

## 8 HM 06 启动提示与安装体验

### bubblewrap 提示与包内资源的关系

启动时出现的警告关键内容如下：

```text
Codex could not find bubblewrap on PATH.
Install bubblewrap with your OS package manager.
Codex will try packaged bubblewrap if available;
restricted commands cannot run without a working sandbox.
```

安装包已包含 codex-resources/bwrap，并且安装时对应文件校验通过。警告只证明 PATH 查找未发现 bubblewrap；不能据此认定包内 bwrap 缺失，也不能把“使用系统包管理器安装”的通用 Linux 建议当作鸿蒙的实际修复路径。

建议启动诊断明确显示选择的是 PATH 资源还是包内资源、解析路径、签名或摘要检查、加载与能力探测结果；若包内资源可用，不应让第一条提示暗示整个依赖没有安装。包内文件存在与文件摘要正确，也不能替代实际执行测试。

### V8 Code Mode 是已知能力边界

```text
Model `gpt-5.6-terra` advertises Code Mode,
but the V8 code-mode host is not available in this native
HarmonyOS build; use direct tools. Direct tools will be used.
```

该提示说明本机构建没有 V8 Code Mode host，系统计划退回 direct tools。它不是模型 API 网络连接失败的证据。本次真正的验收问题是回退后的本地工具仍无法执行，因此应测试 direct tools 的完整读写执行链，而不是仅确认显示了回退提示。

### 版本与环境诊断

- CLI 只显示 0.0.0，使现场难以确认具体提交。建议 --version 或 doctor 输出候选包版本、源码 SHA、OHOS target、打包时间以及 helper 身份。
- 把可写临时目录、系统 CA、包内 helper 和新终端 PATH 作为安装后自检；不要要求用户记住多次临时 export 的先后顺序。
- 安装环境脚本应说明如何加载、如何持久化与如何撤销；不得静默覆盖第三方 provider、密钥或用户原有配置。

### 实际操作成本

用户无法方便地跨端复制命令，多次依赖手工输入和拍照；长路径、嵌套引号及环境变量增加了出错概率，出现过 $HOM 少一字母以及 HOME 路径大小写混淆。建议提供可下载的短命令入口、可重复执行的安装自检和一键脱敏诊断包，优先避免要求用户连续手输复杂命令。

这里区分产品体验问题与运行缺陷：输入错误不能伪装成系统 bug；但安装流程可以通过明确路径、预检和更好的错误提示减少同类错误。

## 9 建议修复顺序与开发者交付物

### 第一阶段 恢复核心编码能力

先用设备诊断确认 HM-04 的具体失败路径，完成 OHOS 共享运行目录的安全适配，再在同一台普通用户设备上跟进 bwrap 后续能力。同步收集 HM-03 的原始工具错误与退出状态，确认终端 159 和文件写入失败是否经过同一执行通道。若涉及信号，记录具体 signal 与触发阶段；若涉及多个问题，分别修复但保留统一回归任务。

完成后交付重新构建、按原生流程签名并校验的候选包。若 bwrap 变化，需要遵守项目“先签名 helper，再将其摘要编入 CLI，再签名 CLI 并组包”的流程，避免资源与编译期摘要不一致。[S9]

### 第二阶段 修复默认网络与安装自检

针对 HM-02 检查默认信任根与 TLS 后端，确保在不手动 export 的新终端中完成模型请求，且显式自定义 CA 仍有清晰优先级和错误诊断。针对 HM-01 与 HM-06，把 TMPDIR、PATH 和资源发现纳入安装后的可重复自检，不依靠一条碰巧成功的旧终端环境。

### 第三阶段 改善可观测性与错误体验

将 HM-05 的预期不兼容从 panic 改为明确错误；输出 sandbox 准备、helper 加载、策略应用、子进程启动等阶段，并区分模型传输与可选遥测。为 bug 报告提供脱敏导出，默认不包含 API Key、Authorization、完整用户提示词或无关文件内容。

### 候选修复包应附带的材料

| 材料 | 必须能回答的问题 |
| --- | --- |
| 构建身份 | 本次包对应哪个源码 SHA、target、SDK 与 helper 摘要？ |
| 变更说明 | 分别修复 HM-01 至 HM-06 中哪些项？哪些仍未验证？ |
| 真机日志 | 在 HarmonyOS PC 7.0 普通用户环境中，哪个步骤实际执行成功？ |
| 隔离验证 | 工作目录内写入正常时，明确禁止的路径是否仍受限？ |
| 安装回归 | 新安装与保留原配置的升级，关闭终端后是否都能正常工作？ |
| 最小编码演示 | 能否实际创建文件、运行终端命令、读取结果并完成修改验证？ |

### 不应作为完成标准的结果

仅通过交叉编译、静态检查、签名工具检查、Mac 上安装模拟或 --version，不足以关闭真机工具故障。项目原有构建记录已明确缺少商业鸿蒙 PC 验证，这次反馈补充了“CLI 与模型连接可用、受限编码执行仍阻塞”的设备证据。[S1]

## 10 回归测试与下一轮最小采集

| 测试 | 通过标准 | 关联项 |
| --- | --- | --- |
| 新终端启动 | codex 可找到；临时目录与 CA 不依赖上次 shell 残留 | 01 / 02 / 06 |
| 启动资源诊断 | 显示实际 bwrap 选择路径与加载结果，提示符合鸿蒙环境 | 04 / 06 |
| 模型真实请求 | 用同一 Responses provider 发起短请求，回答成功且有服务端记录 | 02 |
| 独立受限执行 | codex sandbox -- /usr/bin/sh -c pwd 成功并输出真实 cwd | 04 |
| 交互终端工具 | 会话内 pwd 正常，无 159；明确验证实际工具成功 | 03 |
| 文件写入工具 | 在专用测试目录创建小文本并读取，内容与预期一致 | 03 |
| 最小编码闭环 | 生成一个小文件、运行可用工具验证、修改后再次执行 | 03 / 04 |
| 负向隔离测试 | 对策略明确禁止位置的访问仍被拒绝；保留 socket 隔离 | 04 |
| legacy 不兼容组合 | 返回可解释的策略错误，不触发未处理 panic 或自动降级 | 05 |
| TLS 负向与轮换 | 无效 CA 明确失败；正常系统链通过；证书校验不被关闭 | 02 |
| 关闭并重开终端 | 上述必要功能仍可用；不要求反复手工 export | 01 / 02 / 06 |

### 开发者下一轮最小采集清单

- 完整系统版本、设备型号、uname 信息、候选包和 helper 摘要；不要仅记录 0.0.0。
- 失败调用的原始工具名、cwd、shell、PTY 设置、原始退出状态及生效的沙箱策略；当前这部分证据不完整。
- 沙箱准备阶段实际解析出的 daemon 目录、所有者和权限，以及失败操作的完整错误链；确认 /tmp 是否为本次 EROFS 来源。
- 有效 TMPDIR、CODEX_HOME 和 CA 配置是否设置、解析到何处；只记录诊断所需字段，避免导出含 token 的完整 TOML。
- 模型请求记录与工具执行记录分别留存；不要以模型说“已执行”替代进程输出、写入结果和读取校验。

本轮未采集系统调用跟踪、SIGSYS 信息、完整 backtrace、实际文件写入工具名称，也未验证所有 namespace 与 seccomp 能力。上述信息属于开发者的后续确认项，不应在收到报告前继续要求用户逐项手输大量复杂命令。

## 11 证据索引与源码链接

### 证据等级与使用范围

A 现场证据：用户文字输出、照片中的命令与报错，以及用户确认的实际结果。B 源码证据：本次候选提交的相关实现与项目构建记录。C 待验证推断：/tmp 写入是否正是 HM-04 的失败点、159 是否为 SIGSYS、终端与文件写入是否共享根因。报告中的修复建议不是已经提交或完成的补丁。

| 证据 | 内容与用途 |
| --- | --- |
| E1 安装与启动 | 归档校验 OK、安装文件校验、PATH aliases 警告与版本号 |
| E2 界面与配置 | bubblewrap / V8 提示、重连状态、用户 TOML 与后续 grep 片段 |
| E3 网络与日志 | curl 证书验证成功及 404；显式 CA 后用户确认回答与请求记录 |
| E4 本地工具 | pwd 退出 159；直接 shell 成功；最新补充的文件写入失败 |
| E5 沙箱命令 | bubblewrap 构建报 OS 30；legacy 命令触发明确 panic |

### 包身份

构建记录所列候选提交：

```text
bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9
```

归档名：鸿蒙Codex-0.0.0-harmony.bc1a10fa191c-未真机验证.tar.gz

构建记录所列 SHA-256（用户现场校验显示 OK；本报告未取得归档字节独立复算）：

```text
8c89055d466f86efa7f9956c9f3331860d1689035176c7ee78b01affaeff6383
```

### 可点击的开发者参考

[[S1] 项目构建记录与候选包身份  实施进展与构建记录](https://github.com/uluckyXH/codex/blob/codex/harmony-native/docs/鸿蒙电脑原生适配/实施进展与构建记录.md)

[[S2] PATH aliases 与临时目录防护  arg0/src/lib.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/arg0/src/lib.rs#L349-L389)

[[S3] 自定义 CA 与 rustls 切换  http-client/src/custom_ca.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/http-client/src/custom_ca.rs#L289-L320)

[[S4] Provider token 读取  model-provider/src/auth.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/model-provider/src/auth.rs#L285-L305)

[[S5] bubblewrap 文件系统参数构建  linux-sandbox/src/bwrap.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/linux-sandbox/src/bwrap.rs#L437-L450)

[[S6] 固定 daemon 目录与权限校验  uds/src/daemon_directory.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/uds/src/daemon_directory.rs)

[[S7] legacy Landlock 策略保护  linux-sandbox/src/linux_run_main.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/linux-sandbox/src/linux_run_main.rs#L414-L421)

[[S8] 平台能力边界  features/src/platform.rs](https://github.com/uluckyXH/codex/blob/bc1a10fa191c07a621dca0f6a41ef76c2f4f50e9/codex-rs/features/src/platform.rs)

[[S9] 原生资源构建签名与安装流水线](https://github.com/uluckyXH/codex/blob/codex/harmony-native/docs/鸿蒙电脑原生适配/原生资源构建签名与安装流水线.md)

S2–S8 固定至候选提交，避免后续分支变动影响源码定位；S1 与 S9 为维护分支文档，所述构建状态以本次查阅内容为准。所有第三方服务域名统一替换为 abc.com，密钥替换为 `<REDACTED>`。未附原始私人截图。
