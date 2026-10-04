const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(__dirname + '/entry/src/main/resources/rawfile/terminal.js', 'utf8');
function harness(packet, initiallyConnected = true) {
  const timers = [], writes = [], inputs = [], sizes = [];
  const nodes = {status: {textContent: ''}, terminal: {addEventListener() {}}};
  let terminal, starts = 0;
  class Terminal {
    constructor(options) { this.cols = options.cols; this.rows = options.rows; terminal = this; }
    loadAddon(addon) { addon.term = this; }
    open() {} focus() {} dispose() {} resize(c,r) {this.cols=c;this.rows=r;}
    onData(callback) {this.input = callback;}
    write(bytes, done) {writes.push({bytes: Array.from(bytes), done});}
  }
  class FitAddon {fit() {this.term.cols = 140; this.term.rows = 40;}}
  const native = {readOutput: () => JSON.stringify(packet), writeInput: text => {inputs.push(text);return 0;}, resize: (c,r) => {sizes.push([c,r]);return 0;}, start: () => {starts++;return 0;}};
  const window = {addEventListener() {}, codexBridge: initiallyConnected ? native : null};
  vm.runInNewContext(source, {Terminal, FitAddon: {FitAddon}, document: {getElementById: id => nodes[id], addEventListener() {}}, window, setTimeout: (fn,delay) => {timers.push({fn,delay});return timers.length;}, clearTimeout() {}, TextEncoder, Uint8Array, atob: value => Buffer.from(value,'base64').toString('binary')});
  return {timers,writes,inputs,sizes,nodes,terminal,window,native,starts: () => starts};
}
let checks = 0;
const test = (name, body) => {body();checks++;console.log('PASS',name);};
const idle = {dataBase64:'',running:false,eof:false,exitCode:0,signal:0};
test('startup never starts a process', () => {const h=harness(idle);assert.equal(h.starts(),0);assert.equal(h.inputs.length,0);assert.deepEqual(h.sizes,[[140,40]]);});
test('late native bridge receives current geometry', () => {const h=harness(idle,false);h.window.codexBridge=h.native;h.timers.find(x=>x.delay===100).fn();assert.deepEqual(h.sizes,[[140,40]]);});
test('raw UTF8 bytes preserved across split read', () => {const raw=Buffer.from([0xe4,0xbd]);const h=harness({...idle,dataBase64:raw.toString('base64'),running:true});assert.deepEqual(h.writes[0].bytes,Array.from(raw));});
test('output backpressure waits for terminal completion', () => {const h=harness({...idle,dataBase64:'YQ==',running:true});assert.equal(h.timers.some(x=>x.delay===10),false);h.writes[0].done();assert.equal(h.timers.some(x=>x.delay===10),true);});
test('terminal HTML-like output remains terminal bytes', () => {const raw='<img src=x onerror=alert(1)>\x1b[2J';const h=harness({...idle,dataBase64:Buffer.from(raw).toString('base64'),running:true});assert.equal(Buffer.from(h.writes[0].bytes).toString(),raw);assert.equal(h.nodes.status.textContent.includes('<img'),false);});
test('input byte bound rejects complete oversized Unicode paste', () => {const h=harness(idle);h.terminal.input('🙂'.repeat(5000));assert.equal(h.inputs.length,0);h.terminal.input('你好\r');assert.deepEqual(h.inputs,['你好\r']);});
test('oversized output packet is rejected and retried', () => {const h=harness({...idle,dataBase64:'A'.repeat(90001)});assert.equal(h.writes.length,0);assert.equal(h.timers.some(x=>x.delay===250),true);});
test('native metadata cannot become markup in status', () => {const h=harness({...idle,error:'<script>bad</script>'});assert.equal(h.nodes.status.textContent.includes('<script>'),false);});
console.log(`${checks} checks passed`);
