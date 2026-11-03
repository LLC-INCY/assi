import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const code=readFileSync(new URL('./session-audio.js',import.meta.url),'utf8');
function setup(){let scheduled=0,stopped=0;const ctx={DataView,Uint8Array,Set,snapshot:{state:'active',role:'viewer',request_id:'s',audio_enabled:true,audio_epoch:2},$:()=>null,remote:{snapshot:{}},remoteState:()=>ctx.snapshot};vm.createContext(ctx);vm.runInContext(code+'\nglobalThis.audio=sessionAudio;',ctx);Object.assign(ctx.audio,{enabled:true,request:'s',gain:{},context:{currentTime:1,createBuffer:()=>({getChannelData:()=>new Float32Array(960)}),createBufferSource:()=>({connect(){},disconnect(){},start(){scheduled++;},stop(){stopped++;}})}});return {ctx,scheduled:()=>scheduled,stopped:()=>stopped};}
const batch=(epoch=2)=>({request_id:'s',audio_epoch:epoch,audio_enabled:true,packets:[{sample_rate:48000,channels:2,pcm:Array(3840).fill(0)}]});
test('delayed enabled batch cannot restart sound after owner revocation',()=>{const {ctx,scheduled}=setup();ctx.snapshot.audio_enabled=false;ctx.snapshot.audio_epoch=3;ctx.updateSessionAudio();ctx.playAudioBatch(batch(2));assert.equal(scheduled(),0);});
test('disable/re-enable epoch rejects all old audio',()=>{const {ctx,scheduled}=setup();ctx.snapshot.audio_epoch=4;ctx.playAudioBatch(batch(2));assert.equal(scheduled(),0);ctx.playAudioBatch(batch(4));assert.equal(scheduled(),1);});
test('valid packet schedules; replacement request and malformed PCM do not',()=>{const {ctx,scheduled}=setup();ctx.playAudioBatch(batch());assert.equal(scheduled(),1);ctx.playAudioBatch({...batch(),request_id:'old'});ctx.playAudioBatch({...batch(),packets:[{sample_rate:44100,channels:2,pcm:[0]}]});assert.equal(scheduled(),1);});
test('audio backlog stays bounded to ten 20ms packets',()=>{const {ctx,scheduled}=setup();ctx.playAudioBatch({...batch(),packets:Array(100).fill(batch().packets[0])});assert.equal(scheduled(),10);assert.ok(ctx.audio.next<=1.241);});
test('closed session cannot schedule a queued response',()=>{const {ctx,scheduled}=setup();ctx.snapshot.state='closed';ctx.playAudioBatch(batch());assert.equal(scheduled(),0);});

test('new disabled batch flushes playback before slow snapshot catches up',()=>{const {ctx,scheduled,stopped}=setup();ctx.playAudioBatch(batch());assert.equal(scheduled(),1);ctx.playAudioBatch({...batch(3),audio_enabled:false,packets:[]});assert.equal(stopped(),1);ctx.playAudioBatch(batch(2));assert.equal(scheduled(),1);});
