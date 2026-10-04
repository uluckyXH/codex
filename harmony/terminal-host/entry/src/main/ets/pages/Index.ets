import { common } from '@kit.AbilityKit';
import { webview } from '@kit.ArkWeb';
import hostProbe from 'libhostprobe.so';
import { TerminalBridge } from './TerminalBridge';

@Entry
@Component
struct Index {
  @State private status: string = '空白终端无需配置或 API Key。';
  @State private busy: boolean = false;
  @State private cwd: string = '';
  @State private policyIndex: number = 0;
  @State private selectedTab: number = 0;
  @State private shellStatus: string = '空白终端尚未打开。';
  @State private codexStatus: string = 'Codex 尚未打开。';
  @State private showDiagnostics: boolean = false;
  @StorageLink('hostLastAction') private diagnostic: string = '';
  private shellController: webview.WebviewController = new webview.WebviewController();
  private codexController: webview.WebviewController = new webview.WebviewController();
  private shell: TerminalBridge = new TerminalBridge('shell');
  private codex: TerminalBridge = new TerminalBridge('codex');
  private stateTimer: number = -1;
  private visible: boolean = false;

  aboutToAppear(): void {
    this.visible = true;
    this.stateTimer = setInterval(() => {
      this.shellStatus = this.shell.describe();
      this.codexStatus = this.codex.describe();
    }, 250);
  }
  aboutToDisappear(): void {
    this.visible = false;
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
  private runAction(action: string): void {
    if (this.busy) return;
    this.busy = true; this.status = '正在执行应用操作…';
    try {
      hostProbe.runCodex(this.filesDir(), action).then((result: string) => {
        if (!this.visible) return;
        this.diagnostic = result;
        this.status = result.startsWith('error:') ? '目录准备或操作未完成。展开诊断查看原因；本应用目录保护条件不满足时，可显式收紧后重试。' : '操作结束，诊断记录已保存。';
        this.busy = false;
      }).catch(() => { if (this.visible) { this.status = '操作未完成，请查看诊断记录。'; this.busy = false; } });
    } catch (_) { this.status = '已有诊断运行中，或应用目录来源无法确认。'; this.busy = false; }
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
      TextInput({ text: this.cwd, placeholder: '工作目录（留空使用默认测试目录）' })
        .onChange((value: string) => this.cwd = value)
      Row({ space: 12 }) {
        Button('打开空白终端').onClick(() => {
          this.selectedTab = 0;
          this.status = this.shell.launch(this.filesDir(), this.cwd, 'read-only');
        })
        Select([{ value: '只读' }, { value: '项目可写' }, { value: '完全访问' }])
          .selected(this.policyIndex).onSelect((index: number) => this.policyIndex = index)
        Button('在此目录启动 Codex').onClick(() => {
          this.selectedTab = 1;
          this.status = this.codex.launch(this.filesDir(), this.cwd, this.permission());
        })
      }
      Text('权限选择用于新 Codex 会话；文件访问仍取决于应用的系统授权。').fontSize(13)
      Row({ space: 12 }) {
        Button('中断当前任务').onClick(() => this.status = this.current().control('interrupt'))
        Button('退出 Codex').enabled(this.selectedTab === 1)
          .onClick(() => this.status = this.codex.control('exit-codex'))
        Button('关闭当前终端').onClick(() => this.status = this.current().control('close'))
      }
      Text(this.selectedTab === 0 ? this.shellStatus : this.codexStatus).fontSize(14)
      Text(this.status).fontSize(13).maxLines(2)
      Tabs({ barPosition: BarPosition.Start, index: this.selectedTab }) {
        TabContent() { this.terminalView(this.shellController, this.shell) }.tabBar('空白 Shell')
        TabContent() { this.terminalView(this.codexController, this.codex) }.tabBar('Codex')
      }.onChange((index: number) => this.selectedTab = index).animationDuration(0).layoutWeight(1)
      Row({ space: 12 }) {
        Button('准备目录').onClick(() => this.runAction('prepare'))
        Button('收紧本应用目录').onClick(() => {
          this.status = '仅将当前应用的 base/files 收紧为私有模式，并保存原始权限记录。';
          this.runAction('secure-context');
        })
        Button('版本').onClick(() => this.runAction('version'))
        Button('能力诊断').onClick(() => this.runAction('doctor'))
        Button('只读问候').onClick(() => this.runAction('greeting'))
        Button('停止诊断').onClick(() => {
          this.status = hostProbe.cancelCodex() ? '已请求取消诊断，等待实际退出记录。' : '没有运行中的诊断。';
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
