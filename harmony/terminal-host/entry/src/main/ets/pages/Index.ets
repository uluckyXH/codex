import { common } from '@kit.AbilityKit';
import { webview } from '@kit.ArkWeb';
import hostProbe from 'libhostprobe.so';
import { TerminalBridge } from './TerminalBridge';
@Entry
@Component
struct Index {
  @State private status: string = '默认只初始化私有目录。模型固定为 gpt-5.6-terra。';
  @State private busy: boolean = false;
  private controller: webview.WebviewController = new webview.WebviewController();
  private terminal: TerminalBridge = new TerminalBridge();
  aboutToDisappear(): void {
    this.terminal.stop();
    hostProbe.cancelCodex();
  }
  private runAction(action: string): void {
    if (this.busy) return;
    const context = getContext(this) as common.UIAbilityContext;
    this.busy = true;
    this.status = '正在执行：' + action;
    try {
      hostProbe.runCodex(context.getApplicationContext().filesDir, action).then((result: string) => {
        this.status = '执行结束。状态文件：' + result;
        this.busy = false;
      }).catch(() => { this.status = '启动失败，请查看私有日志。'; this.busy = false; });
    } catch (_) { this.status = '已有任务或启动条件未满足。'; this.busy = false; }
  }
  build() {
    Column({ space: 10 }) {
      Text('Codex 鸿蒙原生终端').fontSize(24)
      Text('只读问候不会自动切换完全权限；受限工具是否可用以能力诊断为准。').fontSize(16)
      Row({ space: 16 }) {
        Button('初始化').onClick(() => this.runAction('prepare'))
        Button('版本').onClick(() => this.runAction('version'))
        Button('能力诊断').onClick(() => this.runAction('doctor'))
      }
      Row({ space: 16 }) {
        Button('启动 Codex').onClick(() => {
          this.status = this.terminal.start() === 0 ? '终端已启动。模型为 gpt-5.6-terra，初始权限为只读。' : '启动未完成，请先初始化并查看私有诊断记录。';
        })
        Button('停止终端').onClick(() => { this.terminal.stop(); this.status = '已请求停止终端。'; })
        Button('发送只读问候').onClick(() => this.runAction('greeting'))
        Button('停止诊断').onClick(() => { hostProbe.cancelCodex(); this.status = '已请求停止当前诊断子进程。'; })
      }
      Text(this.status).fontSize(14).maxLines(3)
      Web({ src: $rawfile('terminal.html'), controller: this.controller })
        .javaScriptAccess(true)
        .fileAccess(false)
        .onlineImageAccess(false)
        .domStorageAccess(false)
        .databaseAccess(false)
        .mixedMode(MixedMode.None)
        .javaScriptProxy({ object: this.terminal, name: 'codexBridge',
          methodList: ['writeInput', 'resize', 'readOutput', 'start', 'stop'], controller: this.controller })
        .onLoadIntercept((event) => {
          const url = event.data.getRequestUrl();
          return url !== 'resource://rawfile/terminal.html';
        })
        .onOverrideUrlLoading((request) => request.getRequestUrl() !== 'resource://rawfile/terminal.html')
        .layoutWeight(1).width('100%')
    }.padding(16).width('100%').height('100%')
  }
}
