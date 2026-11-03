import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('./session-tools.js',import.meta.url),'utf8');
function setup(){
 const elements=new Map();const ctx={TextEncoder,TextDecoder,performance:{now:()=>1000},snapshot:{state:'active',role:'viewer',request_id:'A',clipboard_enabled:true,clipboard_epoch:1},calls:[],esc:String};
 ctx.remoteState=()=>ctx.snapshot;ctx.$=selector=>elements.get(selector)||null;ctx.invoke=async(command,args)=>{ctx.calls.push({command,args});return command==='session_chat'?{request_id:args.requestId,messages:[]}:{request_id:args.requestId,epoch:1,enabled:true,text:null};};
 vm.createContext(ctx);vm.runInContext(source+'\nglobalThis.policy=SessionToolsPolicy;globalThis.tools=sessionTools;globalThis.terminalPolicy=SessionTerminalPolicy;',ctx);return ctx;
}
const deferred=()=>{let resolve;const promise=new Promise(r=>resolve=r);return {promise,resolve};};
test('text bounds are UTF8 bytes and disallow control characters',()=>{const c=setup();assert.equal(c.policy.text('я'.repeat(2048),4096).length,2048);assert.equal(c.policy.text('я'.repeat(2049),4096),null);assert.equal(c.policy.text('hello\x00world',4096),null);assert.equal(c.policy.text('a\r\nb',4096),'a\nb');});
test('chat renders received content as escaped text rather than HTML',()=>{const c=setup(),html=c.policy.chatMarkup([{author:'host',text:'<img src=x onerror=alert(1)> & "hello"'}]);assert.ok(!html.includes('<img'));assert.ok(html.includes('&lt;img'));assert.ok(html.includes('&amp;'));});
test('clipboard requires permission and matching epoch even before slow snapshot catches up',async()=>{const c=setup();c.invoke=async(command,args)=>command==='session_chat'?{messages:[]}:{request_id:'A',epoch:2,enabled:false,text:null};c.tools.clipboardEpoch=1;c.tools.clipboardText='old';await c.pollSessionTools();assert.equal(c.tools.clipboardText,'');assert.equal(c.clipboardAllowed(),false);assert.equal(c.tools.clipboardEpoch,2);});
test('late clipboard from an old permission epoch cannot repopulate text',async()=>{const c=setup(),pending=deferred();c.invoke=async command=>command==='session_chat'?{messages:[]}:pending.promise;const work=c.pollSessionTools();c.snapshot.clipboard_epoch=2;c.snapshot.clipboard_enabled=false;c.updateSessionTools();pending.resolve({request_id:'A',epoch:1,enabled:true,text:'secret'});await work;assert.equal(c.tools.clipboardText,'');assert.equal(c.clipboardAllowed(),false);});
test('late chat and clipboard do not enter replacement session',async()=>{const c=setup(),pending=deferred();c.invoke=async()=>pending.promise;const work=c.pollSessionTools();c.snapshot.request_id='B';c.resetSessionTools('B');pending.resolve({request_id:'A',epoch:1,enabled:true,text:'secret',messages:[{text:'stale'}]});await work;assert.equal(c.tools.clipboardText,'');assert.equal(c.tools.chat.length,0);});
test('terminal transcript strips split CSI and OSC including clipboard escape sequences',()=>{const c=setup(),state={mode:'text'};let text=c.terminalPolicy.strip('ok\x1b[31',state);text+=c.terminalPolicy.strip('m red\x1b]52;c;secret',state);text+=c.terminalPolicy.strip('\x07 end\n',state);assert.equal(text,'ok red end\n');});
