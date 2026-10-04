// Transpile actual Index lifecycle methods and TerminalBridge. Run only after
// stopping the emulator, with the installed SDK's TypeScript module.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const tsPath = process.env.CODEX_TYPESCRIPT_PATH;
if (!tsPath) throw new Error('Set CODEX_TYPESCRIPT_PATH to the installed TypeScript module.');
const ts = require(tsPath);
const pages = path.join(__dirname, 'entry/src/main/ets/pages');
const source = fs.readFileSync(path.join(pages, 'Index.ets'), 'utf8');
const declaration = 'struct Index {';
const start = source.indexOf(declaration);
const end = source.indexOf('  @Builder', start);
assert.ok(start >= 0 && end > start, 'Index lifecycle section must exist');
// Only remove ArkUI declarations; retain every actual field and lifecycle method.
const lifecycle = source.slice(start + declaration.length, end)
  .replace(/@State\s+/g, '')
  .replace(/@StorageLink\('hostLastAction'\)\s+/g, '');

function transpile(input) {
  const result = ts.transpileModule(input, {
    compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
    reportDiagnostics: true
  });
  assert.equal((result.diagnostics || []).filter(d => d.category === ts.DiagnosticCategory.Error).length, 0);
  return result.outputText;
}
const indexCode = transpile('class Index {' + lifecycle + '\n}\nexports.Index = Index;');
const bridgeCode = transpile(fs.readFileSync(path.join(pages, 'TerminalBridge.ets'), 'utf8'));

function harness(open = true) {
  const timers = [];
  const states = {};
  let sequence = 0;
  let finishPicker, finishAction;
  const idle = kind => ({ ok: true, kind, sessionId: '', status: 'idle', running: false,
    cleanupVerified: false, error: '', policy: 'read-only' });
  states.shell = idle('shell');
  states.codex = idle('codex');
  const packet = (kind, values = {}) => JSON.stringify({ ...states[kind], ...values });
  const host = {
    terminalStart(_files, kind, _cwd, policy) {
      states[kind] = { ...idle(kind), sessionId: (++sequence).toString(16).padStart(32, '0'),
        status: 'running', running: true, policy };
      return packet(kind);
    },
    terminalStatus(kind) { return packet(kind); },
    terminalStop(kind, token, operation) {
      if (token !== states[kind].sessionId) return packet(kind, { ok: false, error: 'stale_session' });
      if (operation === 'interrupt') return packet(kind);
      states[kind] = { ...states[kind], ok: false, status: 'stop_failed',
        running: false, cleanupVerified: false, error: 'cleanup_unconfirmed' };
      return packet(kind);
    },
    terminalWrite(kind, token) {
      return packet(kind, { ok: token === states[kind].sessionId && states[kind].running,
        error: states[kind].running ? '' : 'not_running' });
    },
    runCodex() { return new Promise(resolve => { finishAction = resolve; }); },
    cancelCodex() { return true; }
  };
  const bridgeExports = {};
  vm.runInNewContext(bridgeCode, { exports: bridgeExports, require(name) {
    assert.equal(name, 'libhostprobe.so'); return { default: host };
  } });
  const exports = {};
  vm.runInNewContext(indexCode, {
    exports, TerminalBridge: bridgeExports.TerminalBridge, hostProbe: host,
    webview: { WebviewController: class {} }, TabsController: class { changeIndex() {} },
    ProjectDirectoryError: class extends Error {},
    getContext() { return { getApplicationContext: () => ({ filesDir: '/files' }) }; },
    selectProjectDirectory() { return new Promise(resolve => { finishPicker = resolve; }); },
    setInterval(callback, delay) { timers.push({ callback, delay, cancelled: false }); return timers.length - 1; },
    clearInterval(id) { if (timers[id]) timers[id].cancelled = true; }
  });
  const index = new exports.Index();
  index.getUIContext = () => ({ getHostContext: () => ({}) });
  if (open) {
    index.shell.launch('/files', '/project', 'read-only');
    index.codex.launch('/files', '/project', 'read-only');
  }
  index.aboutToAppear();
  const update = (kind, values) => { states[kind] = { ...states[kind], ...values }; };
  const complete = kind => update(kind, { ok: true, status: 'exited',
    running: false, cleanupVerified: true, error: '' });
  return { index, timers, update, complete,
    tick() { assert.equal(timers[0].delay, 250); timers[0].callback(); },
    pick(value) { finishPicker(value); }, action(value) { finishAction(value); } };
}

let count = 0;
async function check(name, test) { await test(); count++; console.log('PASS ' + name); }
async function main() {
  await check('Codex notice follows stop_failed, stopping, then verified cleanup', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    assert.match(h.index.status, /停止未确认/);
    h.update('codex', { status: 'stopping' }); h.tick();
    assert.match(h.index.status, /正在关闭/);
    h.complete('codex'); h.tick();
    assert.match(h.index.status, /已退出，子进程清理已确认/);
    assert.equal(h.index.status, h.index.codexStatus);
    assert.equal(h.index.pendingCloseBridge, undefined);
  });
  await check('Shell close uses the same verified transition', () => {
    const h = harness();
    h.index.controlTerminal(h.index.shell, 'close');
    h.complete('shell'); h.tick();
    assert.match(h.index.status, /空白终端已退出，子进程清理已确认/);
    assert.equal(h.index.status, h.index.shellStatus);
  });
  await check('leader exit without verified cleanup does not claim success', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    h.update('codex', { status: 'exited', cleanupVerified: false }); h.tick();
    assert.match(h.index.status, /停止未确认/);
    assert.equal(h.index.pendingCloseBridge, h.index.codex);
    h.complete('codex'); h.tick();
    assert.match(h.index.status, /子进程清理已确认/);
  });
  await check('persistent stop_failed remains unconfirmed', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    h.tick(); h.tick();
    assert.match(h.index.status, /停止未确认/);
    assert.equal(h.index.pendingCloseBridge, h.index.codex);
  });
  await check('directory selection owns its notice before and after completion', async () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    const selection = h.index.chooseProjectDirectory();
    h.complete('codex'); h.tick();
    assert.equal(h.index.status, '请选择一个项目文件夹。');
    h.pick({ cancelled: false, path: '/project', uri: 'file:///project', readWriteVerified: false });
    await selection;
    const notice = h.index.status; h.tick();
    assert.equal(h.index.status, notice);
    assert.match(notice, /项目目录已选择/);
  });
  await check('both accepted and rejected input notices supersede closing', () => {
    for (const accepted of [true, false]) {
      const h = harness();
      h.index.controlTerminal(h.index.codex, 'exit-codex');
      h.index.selectedTab = 0;
      if (!accepted) h.update('shell', { running: false, status: 'idle' });
      h.index.terminalInput = 'pwd'; h.index.sendTerminalText();
      const notice = h.index.status;
      assert.match(notice, accepted ? /命令已发送/ : /未接收输入/);
      h.complete('codex'); h.tick();
      assert.equal(h.index.status, notice);
    }
  });
  await check('another application action keeps its progress and result notices', async () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    h.index.runAction('version');
    h.complete('codex'); h.tick();
    assert.equal(h.index.status, '正在执行应用操作…');
    h.action('version result'); await Promise.resolve(); await Promise.resolve();
    const notice = h.index.status; h.tick();
    assert.equal(notice, '操作结束，诊断记录已保存。');
    assert.equal(h.index.status, notice);
  });
  await check('a replacement session cannot complete an old close notice', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    const notice = h.index.status;
    h.update('codex', { sessionId: 'f'.repeat(32) }); h.complete('codex'); h.tick();
    assert.equal(h.index.status, notice);
    assert.equal(h.index.pendingCloseBridge, undefined);
  });
  await check('a newer close tracks only its own terminal', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    h.index.controlTerminal(h.index.shell, 'close');
    h.complete('codex'); h.tick();
    assert.match(h.index.status, /空白终端停止未确认/);
    h.complete('shell'); h.tick();
    assert.match(h.index.status, /空白终端已退出，子进程清理已确认/);
  });
  await check('disappearance prevents a queued timer from updating the notice', () => {
    const h = harness();
    h.index.controlTerminal(h.index.codex, 'exit-codex');
    const notice = h.index.status;
    h.index.aboutToDisappear(); h.complete('codex'); h.tick();
    assert.equal(h.timers[0].cancelled, true);
    assert.equal(h.index.pendingCloseBridge, undefined);
    assert.equal(h.index.status, notice);
  });
  await check('closing an unopened terminal does not create a completion watch', () => {
    const h = harness(false);
    h.index.controlTerminal(h.index.codex, 'exit-codex'); h.tick();
    assert.equal(h.index.status, '尚未打开终端。');
    assert.equal(h.index.pendingCloseBridge, undefined);
  });
  console.log(count + ' Index close-status checks passed');
}
main().catch(error => { console.error(error); process.exitCode = 1; });
