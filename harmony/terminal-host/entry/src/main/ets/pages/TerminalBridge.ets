import hostProbe from 'libhostprobe.so';

export interface NativeResult {
  ok: boolean;
  kind: string;
  sessionId: string;
  status: string;
  running: boolean;
  cleanupVerified: boolean;
  error: string;
  errno: number;
  policy: string;
}

// The bundled page only gets token-checked terminal I/O and state. Startup,
// cwd, policy and Context remain in the trusted application UI.
export class TerminalBridge {
  private columns: number = 120;
  private rows: number = 32;
  private sessionId: string = '';
  private kind: string;
  private lastFailure: string = '';

  constructor(kind: string) { this.kind = kind; }

  private parse(result: string): NativeResult { return JSON.parse(result) as NativeResult; }
  private accept(result: string): number {
    const parsed = this.parse(result);
    this.lastFailure = parsed.ok ? '' : parsed.error;
    return parsed.ok ? 0 : -1;
  }

  launch(filesDir: string, cwd: string, policy: string): string {
    const result = hostProbe.terminalStart(filesDir, this.kind, cwd, policy, this.columns, this.rows);
    const parsed = this.parse(result);
    if (parsed.kind === this.kind && /^[0-9a-f]{32}$/.test(parsed.sessionId)) this.sessionId = parsed.sessionId;
    this.lastFailure = parsed.ok ? '' : parsed.error;
    return this.describe();
  }

  control(operation: string): string {
    if (this.sessionId.length === 0) return '尚未打开终端。';
    this.accept(hostProbe.terminalStop(this.kind, this.sessionId, operation));
    return this.describe();
  }

  snapshot(): string { return hostProbe.terminalStatus(this.kind, this.sessionId); }

  writeInput(sessionId: string, data: string): number {
    if (sessionId !== this.sessionId || sessionId.length === 0) return -1;
    return this.accept(hostProbe.terminalWrite(this.kind, sessionId, data));
  }

  resize(sessionId: string, columns: number, rows: number): number {
    if (!Number.isInteger(columns) || !Number.isInteger(rows) || columns < 1 || rows < 1 ||
      columns > 1000 || rows > 1000 || columns * rows > 100000 || sessionId !== this.sessionId) return -1;
    this.columns = columns; this.rows = rows;
    if (sessionId.length === 0) return 0;
    return this.parse(hostProbe.terminalResize(this.kind, sessionId, columns, rows)).ok ? 0 : -1;
  }

  readOutput(sessionId: string): string {
    if (sessionId !== this.sessionId) return '{"ok":false,"error":"stale_session"}';
    return hostProbe.terminalRead(this.kind, sessionId);
  }

  describe(): string {
    try {
      const state = this.parse(this.snapshot());
      const label = this.kind === 'shell' ? '空白终端' : 'Codex';
      if (state.status === 'stop_failed') return label + '停止未确认，请查看诊断记录并重试关闭。';
      if (state.status === 'stopping') return label + '正在关闭并清理子进程…';
      if (this.lastFailure === 'working_directory_access') return '无法进入此目录，请检查路径及应用的文件访问权限。';
      if (this.lastFailure === 'directory_preparation_required') return '请先准备目录；若保护条件不满足，查看本应用目录操作说明。';
      if (this.lastFailure === 'stable_process_handles_unavailable') return '当前系统未提供安全关闭终端所需的进程能力。';
      if (this.lastFailure.length > 0) return label + '操作未完成，请查看诊断记录。';
      if (state.status === 'running') {
        const permission = state.policy === 'workspace-write' ? '项目可写' : state.policy === 'danger-full-access' ? '完全访问' : '只读';
        return this.kind === 'shell' ? '空白终端运行中：可输入 cd，再运行 codex。' : 'Codex 运行中 · gpt-5.6-terra · ' + permission;
      }
      if (state.status === 'exited' && state.cleanupVerified) return label + '已退出，子进程清理已确认。';
      return label + '尚未打开。';
    } catch (_) { return '终端状态暂不可用。'; }
  }
}
