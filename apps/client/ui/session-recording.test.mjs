import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
const source=fs.readFileSync(new URL('./session-recording.js',import.meta.url),'utf8');
function queue(invoke){const context=vm.createContext({Blob,Uint8Array,Map,Error,console});vm.runInContext(source,context);return new context.AssiRecordingQueue(invoke,'request','recording');}
test('splits bounded chunks and preserves exact ordering without whole-video accumulation',async()=>{
 const calls=[];const sink=queue(async(name,args)=>calls.push({name,args}));
 const bytes=new Uint8Array(1048579);bytes[0]=42;bytes[1048576]=71;
 await sink.enqueue(new Blob([bytes]));await sink.drain();
 assert.equal(calls.length,2);assert.equal(calls[0].args.bytes.length,1048576);assert.equal(calls[1].args.bytes.length,3);
 assert.equal(calls[0].args.bytes[0],42);assert.equal(calls[1].args.bytes[0],71);assert.equal(sink.pending,0);
 assert.equal(calls[0].args.recordingId,'recording');assert.equal(calls[0].args.requestId,'request');
});
test('backpressure rejects before materializing more than four chunks',async()=>{
 const sink=queue(async()=>{});
 await assert.rejects(sink.enqueue(new Blob([new Uint8Array(4*1048576+1)])),/Диск/);
 assert.equal(sink.pending,0);
});
test('disk failure poisons queued work and drain reports failure',async()=>{
 let calls=0;const sink=queue(async()=>{calls++;throw new Error('disk full');});
 await assert.rejects(sink.enqueue(new Blob([new Uint8Array(1048577)])),/disk full/);
 await assert.rejects(sink.drain(),/disk full/);assert.equal(calls,1);assert.equal(sink.pending,0);
});
test('closed sink rejects late media callbacks',async()=>{
 let calls=0;const sink=queue(async()=>calls++);sink.closed=true;
 await assert.rejects(sink.enqueue(new Blob(['late'])));assert.equal(calls,0);
});
