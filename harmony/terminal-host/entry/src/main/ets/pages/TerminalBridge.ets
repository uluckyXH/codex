import hostProbe from 'libhostprobe.so';

interface NativeResult {
  ok: boolean;
  error: string;
}

// Only this fixed bridge is exposed to the bundled terminal page. No path,
// executable, configuration or credential getter is exposed to JavaScript.
export class TerminalBridge {
  private columns: number = 120;
  private rows: number = 32;

  private status(result: string): number {
    try {
      const parsed = JSON.parse(result) as NativeResult;
      return parsed.ok ? 0 : -1;
    } catch (_) { return -1; }
  }

  start(): number {
    return this.status(hostProbe.terminalStart(this.columns, this.rows));
  }

  stop(): number {
    return this.status(hostProbe.terminalStop());
  }

  writeInput(data: string): number {
    return this.status(hostProbe.terminalWrite(data));
  }

  resize(columns: number, rows: number): number {
    if (!Number.isInteger(columns) || !Number.isInteger(rows) || columns < 1 || rows < 1 ||
      columns > 1000 || rows > 1000 || columns * rows > 100000) return -1;
    this.columns = columns;
    this.rows = rows;
    // Retain the first fit size even when the native child has not started yet.
    return this.status(hostProbe.terminalResize(columns, rows));
  }

  readOutput(): string {
    return hostProbe.terminalRead();
  }
}
