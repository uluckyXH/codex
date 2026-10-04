(() => {
  "use strict";
  const status = document.getElementById("status");
  const term = new Terminal({
    cols: 100, rows: 32, cursorBlink: true, scrollback: 3000,
    fontSize: 16, fontFamily: "monospace", allowProposedApi: false,
    theme: { background: "#111318", foreground: "#e6e9ef" },
    linkHandler: { activate: () => {} }
  });
  const fit = new FitAddon.FitAddon();
  term.loadAddon(fit); term.open(document.getElementById("terminal"));
  let disposed = false, connected = false, received = false;
  let sessionId = "", kind = "", epoch = 0, resizeTimer, pollTimer;
  const bridge = () => window.codexBridge;
  const message = value => { status.textContent = value; };
  function validate(packet) {
    if (!packet || (packet.kind !== "shell" && packet.kind !== "codex") ||
        (packet.sessionId !== "" && !/^[0-9a-f]{32}$/.test(packet.sessionId)) ||
        typeof packet.dataBase64 !== "string" || packet.dataBase64.length > 21848 ||
        !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(packet.dataBase64)) {
      throw new Error("invalid terminal packet");
    }
    return packet;
  }
  function resize() {
    if (disposed) return;
    fit.fit();
    if (term.cols > 500 || term.rows > 200) term.resize(Math.min(term.cols, 500), Math.min(term.rows, 200));
    const native = bridge();
    if (native && term.cols > 0 && term.rows > 0) native.resize(sessionId, term.cols, term.rows);
  }
  function queuePoll(delay) {
    if (disposed) return;
    clearTimeout(pollTimer); pollTimer = setTimeout(poll, delay);
  }
  function poll() {
    if (disposed) return;
    const native = bridge();
    if (!native) { message("等待应用终端连接"); queuePoll(100); return; }
    try {
      const state = validate(JSON.parse(native.snapshot()));
      if (state.sessionId !== sessionId || state.kind !== kind) {
        sessionId = state.sessionId; kind = state.kind; ++epoch;
        received = false; term.reset(); resize(); connected = true;
      } else if (!connected) { resize(); connected = true; }
      const packet = validate(JSON.parse(native.readOutput(sessionId)));
      if (packet.sessionId !== sessionId || packet.kind !== kind) throw new Error("stale terminal packet");
      if (packet.status === "stop_failed" || (packet.error && !packet.ok)) {
        message("停止或清理尚未确认，请查看应用诊断并重试关闭");
      } else if (packet.status === "stopping") {
        message("正在关闭终端并清理子进程…");
      } else if (packet.status === "exited" && packet.cleanupVerified) {
        const code = Number.isInteger(packet.exitCode) ? packet.exitCode : "未知";
        const signal = Number.isInteger(packet.signal) ? packet.signal : "未知";
        message("会话已结束 · 退出码 " + code + " · 信号 " + signal);
      } else if (packet.running) {
        message(kind === "shell" ? "空白 Shell · 可输入 cd 和 codex" : "Codex · gpt-5.6-terra · 使用上方会话操作");
      } else {
        message(received ? "等待子进程清理确认" : "点击上方按钮打开此终端");
      }
      if (packet.dataBase64) {
        const bytes = Uint8Array.from(atob(packet.dataBase64), value => value.charCodeAt(0));
        const writtenEpoch = epoch, writtenId = sessionId; received = true;
        // Backpressure allows only one pending terminal write. An old callback
        // never sends input, resize or stop to a replacement session.
        term.write(bytes, () => {
          if (disposed) return;
          if (writtenEpoch !== epoch || writtenId !== sessionId) { queuePoll(0); return; }
          queuePoll(10);
        });
      } else queuePoll(packet.running || packet.status === "stopping" ? 40 : 150);
    } catch (_) {
      message("终端连接或会话状态暂不可用"); queuePoll(250);
    }
  }
  term.onData(data => {
    const native = bridge();
    if (!native || disposed || !sessionId) return;
    if (new TextEncoder().encode(data).length > 16384) {
      message("粘贴内容过长，请分段输入（单次最多 16 KiB）"); return;
    }
    try {
      if (native.writeInput(sessionId, data) < 0) message("输入未接受，请检查当前会话状态");
    } catch (_) { message("终端输入未接受"); }
  });
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer); const requestedEpoch = epoch;
    resizeTimer = setTimeout(() => { if (requestedEpoch === epoch) resize(); }, 100);
  });
  document.getElementById("terminal").addEventListener("pointerdown", () => term.focus());
  window.addEventListener("pagehide", () => {
    disposed = true; clearTimeout(resizeTimer); clearTimeout(pollTimer); term.dispose();
  });
  document.addEventListener("DOMContentLoaded", resize, { once: true });
  setTimeout(() => { if (!disposed && connected) { resize(); term.focus(); } }, 100);
  poll();
})();
