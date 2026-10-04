const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(__dirname + '/entry/src/main/resources/rawfile/terminal.js', 'utf8');
const idA = 'a'.repeat(32), idB = 'b'.repeat(32);
const idle = {ok:true,kind:'shell',sessionId:'',status:'idle',dataBase64:'',running:false,eof:true,cleanupVerified:false,exitCode:null,signal:0};
const running = {...idle,sessionId:idA,status:'running',running:true,eof:false};
function harness(packet, initiallyConnected = true) {
  const timers = [], writes = [], inputs = [], sizes = [], handlers = {};
  const nodes = {status: {textContent: ''}, terminal: {addEventListener() {}}};
  let terminal, current = packet, readOverride, resetCount = 0, starts = 0, stops = 0;
  class Terminal {
    constructor(options) { this.cols = options.cols; this.rows = options.rows; terminal = this; }
    loadAddon(addon) { addon.term = this; }
    open() {} focus() {} dispose() {} reset() {resetCount++;} resize(c,r) {this.cols=c;this.rows=r;}
    onData(callback) {this.input = callback;}
    write(bytes, done) {writes.push({bytes: Array.from(bytes), done});}
  }
  class FitAddon {fit() {this.term.cols = 140; this.term.rows = 40;}}
  const native = {
    snapshot: () => JSON.stringify({...current,dataBase64:''}),
    readOutput: id => JSON.stringify(readOverride || (id===current.sessionId ? current : {...current,ok:false,error:'stale_session',dataBase64:''})),
    writeInput: (id,text) => {if(id!==current.sessionId)return -1;inputs.push({id,text});return 0;},
    resize: (id,c,r) => {sizes.push([id,c,r]);return id===current.sessionId?0:-1;},
    start: () => {starts++;return 0;}, stop: () => {stops++;return 0;}
  };
  const window = {addEventListener(name,fn) {handlers[name]=fn;}, codexBridge: initiallyConnected ? native : null};
  vm.runInNewContext(source, {Terminal, FitAddon: {FitAddon}, document: {getElementById: id => nodes[id], addEventListener() {}}, window,
    setTimeout: (fn,delay) => {const timer={fn,delay,cancelled:false};timers.push(timer);return timer;}, clearTimeout(timer) {if(timer)timer.cancelled=true;},
    TextEncoder, Uint8Array, atob: value => Buffer.from(value,'base64').toString('binary')});
  return {timers,writes,inputs,sizes,nodes,terminal,window,native,handlers,
    starts: () => starts, stops: () => stops, resets: () => resetCount,
    packet: value => {current=value;}, readPacket: value => {readOverride=value;},
    tick: delay => {const timer=timers.find(x=>!x.cancelled&&x.delay===delay);assert.ok(timer,`timer ${delay}`);timer.cancelled=true;timer.fn();}};
}
let checks = 0;
const test = (name, body) => {body();checks++;console.log('PASS',name);};
test('page never starts or stops a process', () => {const h=harness(idle);assert.equal(h.starts(),0);assert.equal(h.stops(),0);assert.equal(h.inputs.length,0);assert.deepEqual(h.sizes,[['',140,40]]);});
test('late bridge receives geometry and token', () => {const h=harness(running,false);h.window.codexBridge=h.native;h.tick(100);h.tick(100);assert.deepEqual(h.sizes,[[idA,140,40]]);});
test('split UTF8 bytes remain raw bytes', () => {const raw=Buffer.from([0xe4,0xbd]);const h=harness({...running,dataBase64:raw.toString('base64')});assert.deepEqual(h.writes[0].bytes,Array.from(raw));});
test('output backpressure waits for completion', () => {const h=harness({...running,dataBase64:'YQ=='});assert.equal(h.timers.some(x=>x.delay===10),false);h.writes[0].done();assert.equal(h.timers.some(x=>x.delay===10),true);});
test('HTML and escapes stay terminal bytes', () => {const raw='<img src=x onerror=alert(1)>\x1b[2J';const h=harness({...running,dataBase64:Buffer.from(raw).toString('base64')});assert.equal(Buffer.from(h.writes[0].bytes).toString(),raw);assert.equal(h.nodes.status.textContent.includes('<img'),false);});
test('oversized Unicode paste rejected whole', () => {const h=harness(running);h.terminal.input('🙂'.repeat(5000));assert.equal(h.inputs.length,0);h.terminal.input('你好\r');assert.deepEqual(h.inputs,[{id:idA,text:'你好\r'}]);});
test('oversized output rejected', () => {const h=harness({...running,dataBase64:'A'.repeat(21852)});assert.equal(h.writes.length,0);assert.equal(h.timers.some(x=>x.delay===250),true);});
test('invalid base64 rejected', () => {const h=harness({...running,dataBase64:'<script>'});assert.equal(h.writes.length,0);});
test('native error text cannot become markup', () => {const h=harness({...running,ok:false,error:'<script>bad</script>'});assert.equal(h.nodes.status.textContent.includes('<script>'),false);});
test('exit metadata requires integers', () => {const h=harness({...running,status:'exited',cleanupVerified:true,running:false,exitCode:'<script>',signal:'<img>'});assert.equal(h.nodes.status.textContent.includes('<script>'),false);assert.equal(h.nodes.status.textContent.includes('未知'),true);});
test('leader exit alone never claims cleanup complete', () => {const h=harness({...running,running:false,eof:true,status:'stopping',cleanupVerified:false});assert.equal(h.nodes.status.textContent.includes('会话已结束'),false);});
test('signal failure stays visible', () => {const h=harness({...running,status:'stop_failed',error:'signal_failed',errno:1});assert.equal(h.nodes.status.textContent.includes('未确认'),true);});
test('replacement session resets display and changes input token', () => {const h=harness(running);const before=h.resets();h.packet({...running,sessionId:idB});h.tick(40);assert.equal(h.resets(),before+1);h.terminal.input('pwd\r');assert.deepEqual(h.inputs,[{id:idB,text:'pwd\r'}]);});
test('stale native packet discarded', () => {const h=harness(running);h.packet({...running,sessionId:idB});h.readPacket({...running,dataBase64:'YQ=='});h.tick(40);assert.equal(h.writes.length,0);});
test('input pending page refresh cannot reach new session', () => {const h=harness(running);h.packet({...running,sessionId:idB});h.terminal.input('old command\r');assert.equal(h.inputs.length,0);});
test('late output callback after disposal does not schedule work', () => {const h=harness({...running,dataBase64:'YQ=='});h.handlers.pagehide();const before=h.timers.length;h.writes[0].done();assert.equal(h.timers.length,before);assert.equal(h.stops(),0);});
test('idle page does not send input', () => {const h=harness(idle);h.terminal.input('pwd\r');assert.equal(h.inputs.length,0);});
console.log(`${checks} checks passed`);
