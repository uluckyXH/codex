// Exercise the actual ArkTS bridge after TypeScript transpilation. Run only
// while the emulator is stopped, using the installed SDK's TypeScript module.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const tsPath = process.env.CODEX_TYPESCRIPT_PATH;
if (!tsPath) throw new Error('Set CODEX_TYPESCRIPT_PATH to the installed TypeScript module.');
const ts = require(tsPath);
const source = fs.readFileSync(path.join(__dirname, 'entry/src/main/ets/pages/TerminalBridge.ets'), 'utf8');
const compiled = ts.transpileModule(source, {
  compilerOptions: { target: ts.ScriptTarget.ES2020, module: ts.ModuleKind.CommonJS },
  reportDiagnostics: true
});
assert.equal((compiled.diagnostics || []).filter(d => d.category === ts.DiagnosticCategory.Error).length, 0);
let writes = [];
let state;
const packet = values => JSON.stringify({ ok: true, ...state, ...values });
const host = {
  terminalStart(_files, kind) {
    state = { kind, sessionId: 'a'.repeat(32), status: 'running', policy: 'read-only' };
    return packet();
  },
  terminalStatus() { return packet(); },
  terminalWrite(kind, token, data) {
    if (kind !== state.kind || token !== state.sessionId || Buffer.byteLength(data) > 16384) {
      return packet({ ok: false, error: 'input_rejected' });
    }
    writes.push({ kind, token, data });
    return packet();
  }
};
const moduleExports = {};
vm.runInNewContext(compiled.outputText, { exports: moduleExports, require(name) {
  assert.equal(name, 'libhostprobe.so'); return { default: host };
} });
const Bridge = moduleExports.TerminalBridge;
let count = 0;
function check(name, test) { test(); count++; console.log('PASS ' + name); }
let bridge = new Bridge('codex');
check('unopened terminal rejects input', () => assert.equal(bridge.sendText('hello'), false));
bridge.launch('/files', '/project', 'read-only');
check('Codex wraps text and submits outside the paste', () => {
  assert.equal(bridge.sendText('请检查这个项目'), true);
  assert.equal(writes.at(-1).data, '\x1b[200~请检查这个项目\x1b[201~\r');
});
check('terminal controls and extra lines rejected without writing', () => {
  const before = writes.length;
  for (const text of ['', 'a\nb', 'a\rb', '\x1b[201~bad', 'a\u2028b', '\0', '\x7f']) {
    assert.equal(bridge.sendText(text), false);
  }
  assert.equal(writes.length, before);
});
check('ASCII limit includes both paste markers and Enter', () => {
  assert.equal(bridge.sendText('a'.repeat(16371)), true);
  assert.equal(Buffer.byteLength(writes.at(-1).data), 16384);
  assert.equal(bridge.sendText('a'.repeat(16372)), false);
});
check('native UTF-8 rejection leaves input unaccepted', () => {
  const before = writes.length;
  assert.equal(bridge.sendText('中'.repeat(6000)), false);
  assert.equal(writes.length, before);
});
check('confirmed cleanup clears a prior input failure', () => {
  state.status = 'exited'; state.cleanupVerified = true;
  assert.match(bridge.describe(), /已退出，子进程清理已确认/);
});
check('old token cannot write to a replacement native session', () => {
  state.sessionId = 'b'.repeat(32);
  const before = writes.length;
  assert.equal(bridge.sendText('old session'), false);
  assert.equal(writes.length, before);
});
check('Shell retains its ordinary input path', () => {
  bridge = new Bridge('shell'); bridge.launch('/files', '/project', 'read-only');
  assert.equal(bridge.sendText('pwd'), true);
  assert.equal(writes.at(-1).data, 'pwd\r');
});
console.log(count + ' native input bridge checks passed');
