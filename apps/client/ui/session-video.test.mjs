import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';

const source=fs.readFileSync(new URL('./session-video.js',import.meta.url),'utf8');
function h264(keyframe=true,width=64,height=48){
 const ue=n=>{const value=(n+1).toString(2);return '0'.repeat(value.length-1)+value;};
 let bits=[0x42,0xc0,0x32].map(v=>v.toString(2).padStart(8,'0')).join('')+ue(0)+ue(0)+ue(0)+ue(0)+ue(1)+'0'+ue(width/16-1)+ue(height/16-1)+'110'+'1';
 bits=bits.padEnd(Math.ceil(bits.length/8)*8,'0');const sps=Array.from({length:bits.length/8},(_,i)=>parseInt(bits.slice(i*8,i*8+8),2));
 return keyframe?[0,0,0,1,0x67,...sps,0,0,0,1,0x68,1,0,0,0,1,0x65,1]:[0,0,0,1,0x41,1];
}
function fixture({supported=true}={}){
 const decoders=[],draws=[],frames=[],errors=[];let requests=0;
 class Decoder{
  static async isConfigSupported(){return {supported};}
  constructor(callbacks){this.callbacks=callbacks;this.state='unconfigured';this.decodeQueueSize=0;this.chunks=[];decoders.push(this);}
  configure(config){this.config=config;this.state='configured';}
  decode(chunk){this.chunks.push(chunk);}
  close(){this.state='closed';}
 }
 const context=vm.createContext({VideoDecoder:Decoder,EncodedVideoChunk:class{constructor(options){Object.assign(this,options);}},Uint8Array,Map,Error,console});
 vm.runInContext(source,context);
 const canvas={width:0,height:0,getContext:()=>({drawImage:(...args)=>draws.push(args)})};
 const player=new context.AssiVideoDecoder(canvas,{onFrame:f=>frames.push(f),onKeyframe:()=>requests++,onError:e=>errors.push(e)});
 const push=(sequence,keyframe=false,overrides={})=>player.push({codec:'h264',width:64,height:48,timestamp_us:sequence*40000,sequence,screen_epoch:1,keyframe,bytes:h264(keyframe),...overrides});
 return {context,player,push,decoders,draws,frames,errors,get requests(){return requests;}};
}
test('capability probe checks actual WebCodecs support',async()=>{
 assert.equal(await fixture().context.AssiVideoDecoder.supported(),true);
 assert.equal(await fixture({supported:false}).context.AssiVideoDecoder.supported(),false);
});
test('delta before keyframe requests recovery once; gap discards dependent frames',()=>{
 const f=fixture();assert.equal(f.push(1),false);f.push(2);assert.equal(f.requests,1);
 assert.equal(f.push(3,true),true);assert.equal(f.push(4),true);
 assert.equal(f.push(6),false);assert.equal(f.decoders[0].state,'closed');assert.equal(f.requests,2);
 assert.equal(f.push(7,true),true);assert.equal(f.decoders.length,2);
});
test('draws decoded output and releases VideoFrame even after session reset',()=>{
 const f=fixture();f.push(1,true);let closed=0;
 f.decoders[0].callbacks.output({timestamp:40000,displayWidth:64,displayHeight:48,close:()=>closed++});
 assert.equal(f.draws.length,1);assert.equal(f.frames.length,1);assert.equal(closed,1);
 f.player.reset();f.decoders[0].callbacks.output({timestamp:40000,displayWidth:64,displayHeight:48,close:()=>closed++});
 assert.equal(f.draws.length,1);assert.equal(closed,2);
});
test('oversized dimensions reject before decoder allocation',()=>{
 const f=fixture();assert.equal(f.push(1,true,{width:4097}),false);assert.equal(f.decoders.length,0);assert.equal(f.errors.length,1);
});
test('decoder backpressure resets and requests a new keyframe',()=>{
 const f=fixture();f.push(1,true);f.decoders[0].decodeQueueSize=4;
 assert.equal(f.push(2),false);assert.equal(f.requests,1);assert.equal(f.decoders[0].state,'closed');
});
test('epoch change requires keyframe and stale decoder errors are ignored',()=>{
 const f=fixture();f.push(1,true);assert.equal(f.push(2,false,{screen_epoch:2}),false);
 assert.equal(f.requests,1);f.decoders[0].callbacks.error(new Error('old'));assert.equal(f.errors.length,0);
});
test('codec profile reads actual Annex B SPS and duplicate delivery is ignored',()=>{
 const f=fixture();f.push(1,true);assert.equal(f.decoders[0].config.codec,'avc1.42C032');
 assert.equal(f.push(1,true),false);assert.equal(f.decoders[0].chunks.length,1);
});
test('SPS dimensions are checked before decoder allocation, not only on output',()=>{
 const f=fixture();assert.equal(f.push(1,true,{bytes:h264(true,4096,4096)}),false);
 assert.equal(f.decoders.length,0);assert.equal(f.errors.length,1);
 const mismatch=fixture();assert.equal(mismatch.push(1,true,{bytes:h264(true,128,64)}),false);
 assert.equal(mismatch.decoders.length,0);
});
test('keyframe requires SPS, PPS and IDR, and truncated SPS cannot allocate decoder',()=>{
 const f=fixture();assert.equal(f.push(1,true,{bytes:h264(false)}),false);assert.equal(f.decoders.length,0);
 const broken=fixture();assert.equal(broken.push(1,true,{bytes:[0,0,0,1,0x67,0x42,0xc0,0x32]}),false);assert.equal(broken.decoders.length,0);
});
