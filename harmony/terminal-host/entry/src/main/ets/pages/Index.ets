import { common } from '@kit.AbilityKit';
import { webview } from '@kit.ArkWeb';
import { BusinessError } from '@kit.BasicServicesKit';
import hostProbe from 'libhostprobe.so';
import { NativeResult, TerminalBridge } from './TerminalBridge';
import { authorizeProjectDirectory, ProjectDirectoryError, ProjectDirectorySelection,
  selectProjectDirectory } from './ProjectDirectoryPicker';

interface DirectorySelectionDiagnostic {
  action: string;
  uri: string;
  cwd: string;
  applicationDirectoryOpen: boolean;
  applicationReadWriteAccess: boolean;
  pickerAuthorization: boolean;
  nativeDirectoryAccess: string;
  nativeWriteAccess: string;
  persistentPermission: boolean;
}

interface DirectorySelectionFailure {
  action: string;
  stage: string;
  code: number;
  message: string;
}

@Entry
@Component
struct Index {
  @State private status: string = '空白终端无需配置或 API Key。';
  @State private busy: boolean = false;
  @State private cwd: string = '';
  @State private choosingFolder: boolean = false;
  @State private directoryNotice: string = '';
  @State private projectReadWriteReady: boolean = false;
  @State private policyIndex: number = 0;
  @State private selectedTab: number = 0;
  @State private terminalInput: string = '';
  @State private shellStatus: string = '空白终端尚未打开。';
  @State private codexStatus: string = 'Codex 尚未打开。';
  @State private showDiagnostics: boolean = false;
  @StorageLink('hostLastAction') private diagnostic: string = '';
  private shellController: webview.WebviewController = new webview.WebviewController();
  private codexController: webview.WebviewController = new webview.WebviewController();
  private shell: TerminalBridge = new TerminalBridge('shell');
  private codex: TerminalBridge = new TerminalBridge('codex');
  private tabsController: TabsController = new TabsController();
  private selectedDirectoryUri: string = '';
  private pickerGeneration: number = 0;
  private stateTimer: number = -1;
  private visible: boolean = false;
  private pendingCloseBridge: TerminalBridge | undefined = undefined;
  private pendingCloseSessionId: string = '';

  aboutToAppear(): void {
    this.visible = true;
    this.stateTimer = setInterval(() => {
      this.shellStatus = this.shell.describe();
      this.codexStatus = this.codex.describe();
      this.refreshCloseStatus();
    }, 250);
  }
  aboutToDisappear(): void {
    this.visible = false;
    this.pendingCloseBridge = undefined;
    this.pendingCloseSessionId = '';
    this.pickerGeneration++;
    this.choosingFolder = false;
    clearInterval(this.stateTimer);
    this.shell.control('close'); this.codex.control('close');
    hostProbe.cancelCodex();
  }
  private filesDir(): string {
    return (getContext(this) as common.UIAbilityContext).getApplicationContext().filesDir;
  }
  private permission(): string {
    return this.policyIndex === 1 ? 'workspace-write' : this.policyIndex === 2 ? 'danger-full-access' : 'read-only';
  }
  private current(): TerminalBridge { return this.selectedTab === 0 ? this.shell : this.codex; }
  private setStatus(status: string): void {
    // A new operation owns the notice and cancels an earlier close update.
    this.pendingCloseBridge = undefined;
    this.pendingCloseSessionId = '';
    this.status = status;
  }
  private controlTerminal(bridge: TerminalBridge, operation: string): void {
    this.setStatus(bridge.control(operation));
    if (operation !== 'close' && operation !== 'exit-codex') return;
    try {
      const state = JSON.parse(bridge.snapshot()) as NativeResult;
      if (!/^[0-9a-f]{32}$/.test(state.sessionId)) return;
      this.pendingCloseBridge = bridge;
      this.pendingCloseSessionId = state.sessionId;
      this.refreshCloseStatus();
    } catch (_) {}
  }
  private refreshCloseStatus(): void {
    const bridge = this.pendingCloseBridge;
    if (!this.visible || bridge === undefined) return;
    try {
      const state = JSON.parse(bridge.snapshot()) as NativeResult;
      if (state.sessionId !== this.pendingCloseSessionId) {
        this.pendingCloseBridge = undefined;
        this.pendingCloseSessionId = '';
        return;
      }
      const confirmed: boolean = state.status === 'exited' && state.cleanupVerified === true;
      if (!confirmed && state.status !== 'stopping' && state.status !== 'stop_failed') return;
      const status: string = bridge.describe();
      if (bridge === this.shell) this.shellStatus = status;
      else this.codexStatus = status;
      if (confirmed) this.setStatus(status);
      else this.status = status;
    } catch (_) {}
  }
  private switchTerminal(index: number): void {
    this.tabsController.changeIndex(index);
    this.selectedTab = index;
  }
  private sendTerminalText(): void {
    if (this.choosingFolder || this.terminalInput.length === 0) return;
    let accepted: boolean = false;
    try {
      accepted = this.current().sendText(this.terminalInput);
    } catch (_) {
      accepted = false;
    }
    if (accepted) {
      this.terminalInput = '';
      this.setStatus(this.selectedTab === 0 ? '命令已发送到当前 Shell。' : '对话已发送到当前 Codex 终端。');
    } else {
      this.setStatus('当前终端未接收输入，内容已保留；请先打开终端或等待就绪。');
    }
  }
  private async chooseProjectDirectory(allowReadWrite: boolean = false): Promise<void> {
    if (this.busy || this.choosingFolder) return;
    if (allowReadWrite && this.selectedDirectoryUri.length === 0) {
      this.setStatus('请先选择需要读写的项目文件夹。');
      return;
    }
    const action: string = allowReadWrite ? 'authorize-project-directory' : 'select-project-directory';
    const generation: number = ++this.pickerGeneration;
    this.choosingFolder = true;
    this.setStatus(allowReadWrite ? '请在系统界面确认仅允许读写所选项目文件夹。' : '请选择一个项目文件夹。');
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const selection: ProjectDirectorySelection = allowReadWrite ?
        await authorizeProjectDirectory(context, this.selectedDirectoryUri, this.cwd) :
        await selectProjectDirectory(context);
      if (!this.visible || generation !== this.pickerGeneration) return;
      if (selection.cancelled) {
        this.setStatus(allowReadWrite ? '已取消读写授权，工作目录和会话模式未改变。' : '已取消选择，工作目录未改变。');
        return;
      }
      this.cwd = selection.path;
      this.selectedDirectoryUri = selection.uri;
      this.projectReadWriteReady = selection.readWriteVerified;
      this.directoryNotice = selection.readWriteVerified ?
        '所选项目读写权限检查通过；请为新 Codex 会话选择合适的权限模式。关闭应用后需重新授权。' :
        '项目已选择；编辑前请允许读写所选项目。关闭应用后需重新选择。';
      this.setStatus(selection.readWriteVerified ? '项目目录读写检查通过，Codex 会话模式未改变。' :
        '项目目录已选择，可在此启动只读会话。');
      const diagnostic: DirectorySelectionDiagnostic = { action: action,
        uri: this.selectedDirectoryUri, cwd: this.cwd, applicationDirectoryOpen: true,
        applicationReadWriteAccess: selection.readWriteVerified, pickerAuthorization: allowReadWrite,
        nativeDirectoryAccess: 'pending', nativeWriteAccess: 'pending', persistentPermission: false };
      this.diagnostic = JSON.stringify(diagnostic);
    } catch (error) {
      if (!this.visible || generation !== this.pickerGeneration) return;
      const businessError = error as BusinessError;
      const stage: string = error instanceof ProjectDirectoryError ? error.stage : 'picker_context';
      const code: number = Number.isFinite(businessError.code) ? businessError.code : 0;
      this.setStatus(allowReadWrite ? '项目读写授权或检查未完成，工作目录和会话模式未改变；展开诊断查看原因。' :
        '目录选择或访问检查失败，工作目录未改变；展开诊断查看原因。');
      const failure: DirectorySelectionFailure = { action: action, stage: stage,
        code: code, message: businessError.message };
      this.diagnostic = JSON.stringify(failure);
    } finally {
      if (generation === this.pickerGeneration) this.choosingFolder = false;
    }
  }
  private runAction(action: string): void {
    if (this.busy || this.choosingFolder) return;
    this.busy = true; this.setStatus('正在执行应用操作…');
    try {
      hostProbe.runCodex(this.filesDir(), action).then((result: string) => {
        if (!this.visible) return;
        this.diagnostic = result;
        this.setStatus(result.startsWith('error:') ? '目录准备或操作未完成。展开诊断查看原因；本应用目录保护条件不满足时，可显式收紧后重试。' : '操作结束，诊断记录已保存。');
        this.busy = false;
      }).catch(() => { if (this.visible) { this.setStatus('操作未完成，请查看诊断记录。'); this.busy = false; } });
    } catch (_) { this.setStatus('已有诊断运行中，或应用目录来源无法确认。'); this.busy = false; }
  }
  @Builder
  private terminalView(controller: webview.WebviewController, bridge: TerminalBridge) {
    Web({ src: $rawfile('terminal.html'), controller: controller })
      .javaScriptAccess(true).fileAccess(false).onlineImageAccess(false)
      .domStorageAccess(false).databaseAccess(false).mixedMode(MixedMode.None)
      .javaScriptProxy({ object: bridge, name: 'codexBridge',
        methodList: ['writeInput', 'resize', 'readOutput', 'snapshot'], controller: controller })
      .onLoadIntercept((event) => event.data.getRequestUrl() !== 'resource://rawfile/terminal.html')
      .onOverrideUrlLoading((request) => request.getRequestUrl() !== 'resource://rawfile/terminal.html')
      .width('100%').height('100%')
  }
  build() {
    Column({ space: 8 }) {
      Text('Codex 原生终端').fontSize(24)
      Row({ space: 12 }) {
        TextInput({ text: this.cwd, placeholder: '工作目录（留空使用默认测试目录）' })
          .layoutWeight(1).enabled(!this.choosingFolder)
          .onChange((value: string) => {
            if (value !== this.cwd) {
              this.cwd = value;
              this.selectedDirectoryUri = '';
              this.projectReadWriteReady = false;
              this.directoryNotice = '';
            }
          })
        Button('选择项目文件夹').enabled(!this.busy && !this.choosingFolder)
          .onClick(() => { this.chooseProjectDirectory(); })
        Button('允许读写所选项目')
          .enabled(!this.busy && !this.choosingFolder && this.selectedDirectoryUri.length > 0)
          .onClick(() => { this.chooseProjectDirectory(true); })
      }
      if (this.directoryNotice.length > 0) {
        Text(this.directoryNotice).fontSize(13).maxLines(2)
      }
      Row({ space: 12 }) {
        Button('打开空白终端').enabled(!this.choosingFolder).onClick(() => {
          this.switchTerminal(0);
          this.setStatus(this.shell.launch(this.filesDir(), this.cwd, 'read-only'));
        })
        Select([{ value: '只读' }, { value: '项目可写' }, { value: '完全访问' }])
          .selected(this.policyIndex)
          .value(this.policyIndex === 0 ? '只读' : this.policyIndex === 1 ? '项目可写' : '完全访问')
          .onSelect((index: number) => this.policyIndex = index)
        Button('在此目录启动 Codex').enabled(!this.choosingFolder).onClick(() => {
          if (this.selectedDirectoryUri.length > 0 && this.policyIndex !== 0 && !this.projectReadWriteReady) {
            this.setStatus('请先允许读写所选项目，再以当前模式启动 Codex。');
            return;
          }
          this.switchTerminal(1);
          this.setStatus(this.codex.launch(this.filesDir(), this.cwd, this.permission()));
        })
      }
      Text('Codex 默认只读；编辑项目需允许目录读写并选择权限模式。“项目可写”还需系统支持受限执行。').fontSize(13)
      Row({ space: 12 }) {
        Button('中断当前任务').onClick(() => this.controlTerminal(this.current(), 'interrupt'))
        Button('退出 Codex').enabled(this.selectedTab === 1)
          .onClick(() => this.controlTerminal(this.codex, 'exit-codex'))
        Button('关闭当前终端').onClick(() => this.controlTerminal(this.current(), 'close'))
      }
      Text(this.selectedTab === 0 ? this.shellStatus : this.codexStatus).fontSize(14)
      Text(this.status).fontSize(13).maxLines(2)
      Row({ space: 12 }) {
        TextInput({ text: this.terminalInput,
          placeholder: this.selectedTab === 0 ? '输入命令，回车发送到当前 Shell' : '输入对话，回车发送到当前 Codex' })
          .layoutWeight(1).enabled(!this.choosingFolder)
          .enterKeyType(EnterKeyType.Send)
          .onChange((value: string) => this.terminalInput = value)
          .onSubmit(() => this.sendTerminalText())
        Button('发送').enabled(!this.choosingFolder && this.terminalInput.length > 0)
          .onClick(() => this.sendTerminalText())
      }
      Tabs({ barPosition: BarPosition.Start, index: this.selectedTab, controller: this.tabsController }) {
        TabContent() { this.terminalView(this.shellController, this.shell) }.tabBar('空白 Shell')
        TabContent() { this.terminalView(this.codexController, this.codex) }.tabBar('Codex')
      }.onChange((index: number) => this.selectedTab = index).animationDuration(0).layoutWeight(1)
      Row({ space: 12 }) {
        Button('准备目录').onClick(() => this.runAction('prepare'))
        Button('收紧本应用目录').onClick(() => {
          this.setStatus('仅将当前应用的 base/files 收紧为私有模式，并保存原始权限记录。');
          this.runAction('secure-context');
        })
        Button('版本').onClick(() => this.runAction('version'))
        Button('能力诊断').onClick(() => this.runAction('doctor'))
        Button('只读问候').onClick(() => this.runAction('greeting'))
        Button('停止诊断').onClick(() => {
          this.setStatus(hostProbe.cancelCodex() ? '已请求取消诊断，等待实际退出记录。' : '没有运行中的诊断。');
        })
      }
      Button(this.showDiagnostics ? '收起诊断' : '查看诊断').onClick(() => this.showDiagnostics = !this.showDiagnostics)
      if (this.showDiagnostics) {
        Scroll() {
          Text(this.diagnostic).fontSize(12).fontFamily('monospace').copyOption(CopyOptions.LocalDevice)
        }.height(100).width('100%')
      }
    }.padding(12).width('100%').height('100%')
  }
}
