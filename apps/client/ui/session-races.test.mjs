import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('./session-ui.js',import.meta.url),'utf8');
const funcs=source.slice(source.indexOf('function emitInput('),source.indexOf('function bindRemoteInput('));
function setup(){
 let reject,resolve;
 const pending=new Promise((yes,no)=>{resolve=yes;reject=no;});
 const ctx={remote:{generation:0,inputQueue:[],sending:false},snapshot:{state:'active',role:'viewer',control:true,request_id:'A'},calls:[],actions:[]};
 ctx.remoteState=()=>ctx.snapshot;ctx.sessionAction=a=>ctx.actions.push(a);
 ctx.invoke=async(command,args)=>{ctx.calls.push({command,args});if(ctx.calls.length===1)return pending;};
 vm.createContext(ctx);vm.runInContext(funcs,ctx);return {ctx,reject,resolve};
}
const flush=()=>new Promise(resolve=>setImmediate(resolve));
test('old rejected input cannot stop replacement session; replacement queue resumes',async()=>{
 const {ctx,reject}=setup();ctx.emitInput({type:'key',key:'a',down:true});
 ctx.remote.generation++;ctx.snapshot.request_id='B';ctx.remote.inputQueue=[];
 ctx.emitInput({type:'key',key:'b',down:true});reject('old session closed');await flush();
 assert.deepEqual(ctx.actions,[]);assert.equal(ctx.calls.length,2);assert.equal(ctx.calls[0].args.requestId,'A');assert.equal(ctx.calls[1].args.requestId,'B');
});
test('pointer moves coalesce without reordering key events',async()=>{
 const {ctx,resolve}=setup();ctx.emitInput({type:'key',key:'a',down:true});
 for(let i=0;i<1000;i++)ctx.emitInput({type:'move',x:i/1000,y:0});
 ctx.emitInput({type:'key',key:'a',down:false});
 assert.equal(ctx.remote.inputQueue.length,2);resolve();await flush();
 assert.equal(ctx.calls.length,3);assert.equal(ctx.calls[1].args.input.x,.999);assert.equal(ctx.calls[2].args.input.down,false);
});
test('blur discards stale movement and sends release after in-flight input',async()=>{
 const {ctx,resolve}=setup();ctx.emitInput({type:'key',key:'shift',down:true});ctx.emitInput({type:'move',x:0,y:0});ctx.releaseRemoteInput();resolve();await flush();
 assert.equal(ctx.calls.length,2);assert.equal(ctx.calls[1].args.input.type,'release_all');
});
test('input never sent without active control permission',()=>{
 const {ctx}=setup();ctx.snapshot.control=false;ctx.emitInput({type:'key',key:'a',down:true});assert.equal(ctx.calls.length,0);
});
test('accept/reject use the request the user saw, not a replaced snapshot',async()=>{
 for(const action of ['accept','reject']){
  const ctx={calls:[],remote:{},remoteState:()=>({request_id:'new'}),$:()=>({checked:true}),refreshSession:async()=>{},toast:()=>{throw Error('Unexpected error');},invoke:async(command,args)=>ctx.calls.push({command,args})};
  vm.createContext(ctx);vm.runInContext(source.slice(source.indexOf('async function sessionAction('),source.indexOf('const remoteKeys=')),ctx);
  await ctx.sessionAction(action,{disabled:false,dataset:{requestId:'shown'}});
  assert.equal(ctx.calls.length,1);assert.equal(ctx.calls[0].args.requestId,'shown');assert.equal(ctx.calls[0].command,'session_'+action);
 }
});

test('queued input keeps its displayed epoch when the snapshot changes',async()=>{const {ctx,resolve}=setup();ctx.snapshot.screen_epoch=1;ctx.remote.displayedEpoch=1;ctx.emitInput({type:'key',key:'a',down:true});ctx.emitInput({type:'key',key:'a',down:false});ctx.snapshot.screen_epoch=2;ctx.emitInput({type:'move',x:.5,y:.5});resolve();await flush();assert.equal(ctx.calls.length,2);assert.equal(ctx.calls[1].args.screenEpoch,1);});
test('screen switch blocks input except releasing held keys',()=>{const {ctx}=setup();ctx.snapshot.screen_epoch=2;ctx.snapshot.screen_pending=true;ctx.remote.displayedEpoch=1;ctx.emitInput({type:'button',button:'left',down:true});assert.equal(ctx.calls.length,0);ctx.releaseRemoteInput();assert.equal(ctx.calls[0].args.input.type,'release_all');});
