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
  term.loadAddon(fit);
  term.open(document.getElementById("terminal"));
  let disposed = false;
  let received = false;
  let connected = false;
  let lastError = "";
  let resizeTimer;
  const bridge = () => window.codexBridge;
  const message = value => { status.textContent = value; };
  function resize() {
    if (disposed) return;
    fit.fit();
    if (term.cols > 500 || term.rows > 200) term.resize(Math.min(term.cols, 500), Math.min(term.rows, 200));
    const native = bridge();
    if (native && term.cols > 0 && term.rows > 0) {
      native.resize(Math.min(term.cols, 500), Math.min(term.rows, 200));
    }
  }
  function queuePoll(delay) {
    if (!disposed) setTimeout(poll, delay);
  }
  function poll() {
    if (disposed) return;
    const native = bridge();
    if (!native) { message("等待应用终端连接"); queuePoll(100); return; }
    try {
      if (!connected) { resize(); connected = true; }
      const packet = JSON.parse(native.readOutput());
      if (typeof packet.dataBase64 !== "string" || packet.dataBase64.length > 90000) {
        throw new Error("invalid terminal packet");
      }
      if (packet.error) {
        message("终端状态异常，请查看应用诊断记录");
      } else if (packet.running) {
        message("Codex 原生终端 · gpt-5.6-terra · 在上方停止会话");
      } else if (packet.eof && received) {
        message("会话已结束 · 退出码 " + String(packet.exitCode) + " · 信号 " + String(packet.signal));
      } else {
        message("点击应用上方“启动 Codex”进入终端");
      }
      if (packet.dataBase64) {
        const binary = atob(packet.dataBase64);
        const bytes = Uint8Array.from(binary, value => value.charCodeAt(0));
        received = true;
        term.write(bytes, () => queuePoll(10));
      } else {
        queuePoll(packet.running ? 40 : 150);
      }
      lastError = "";
    } catch (_) {
      if (lastError !== "read") message("终端连接暂不可用");
      lastError = "read";
      queuePoll(250);
    }
  }
  term.onData(data => {
    const native = bridge();
    if (!native || disposed) return;
    // Bound individual pastes; never silently run a truncated command.
    if (new TextEncoder().encode(data).length > 16384) {
      message("粘贴内容过长，请分段输入（单次最多 16 KiB）"); return;
    }
    try {
      if (native.writeInput(data) < 0) message("当前没有运行中的终端，或输入暂未接受");
    } catch (_) { message("终端输入未接受"); }
  });
  window.addEventListener("resize", () => {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(resize, 100);
  });
  document.getElementById("terminal").addEventListener("pointerdown", () => term.focus());
  window.addEventListener("pagehide", () => { disposed = true; term.dispose(); });
  document.addEventListener("DOMContentLoaded", resize, { once: true });
  setTimeout(() => { resize(); term.focus(); }, 100);
  poll();
})();
