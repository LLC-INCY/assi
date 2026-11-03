'use strict';
// H.264 Annex B: SPS/PPS travel in keyframes; no avcC description is supplied.
// Encoding is software; WebCodecs chooses the available local decoder.
class AssiVideoDecoder {
 static async supported(){
  if(typeof globalThis.VideoDecoder!=='function'||typeof globalThis.EncodedVideoChunk!=='function')return false;
  try{return !!(await globalThis.VideoDecoder.isConfigSupported({codec:'avc1.42C032',optimizeForLatency:true})).supported;}catch{return false;}
 }
 constructor(canvas,{onFrame=()=>{},onKeyframe=()=>{},onError=()=>{}}={}){
  this.canvas=canvas;this.onFrame=onFrame;this.onKeyframe=onKeyframe;this.onError=onError;
  this.decoder=null;this.generation=0;this.closed=false;this.pending=new Map();this.waiting=true;this.requested=false;this.lastSequence=null;this.lastTimestamp=null;this.epoch=null;this.width=null;this.height=null;
 }
 reset(){
  this.generation++;const decoder=this.decoder;this.decoder=null;
  if(decoder&&decoder.state!=='closed')decoder.close();
  this.pending.clear();this.waiting=true;this.requested=false;this.lastSequence=null;this.lastTimestamp=null;this.epoch=null;this.width=null;this.height=null;
 }
 close(){this.reset();this.closed=true;}
 requestKeyframe(){if(!this.requested&&!this.closed){this.requested=true;this.onKeyframe();}}
 fail(error){this.reset();if(!this.closed)this.onError(error instanceof Error?error:new Error(String(error)));}
 static codec(bytes){
  // Only read the fixed three SPS profile bytes; the browser parses the bitstream.
  for(let i=0;i+7<bytes.length;i++){
   let header=-1;
   if(bytes[i]===0&&bytes[i+1]===0&&bytes[i+2]===1)header=i+3;
   else if(bytes[i]===0&&bytes[i+1]===0&&bytes[i+2]===0&&bytes[i+3]===1)header=i+4;
   if(header>=0&&(bytes[header]&31)===7&&header+3<bytes.length){
    return `avc1.${Array.from(bytes.subarray(header+1,header+4),v=>v.toString(16).padStart(2,'0')).join('').toUpperCase()}`;
   }
  }
  return null;
 }
 static inspect(bytes){
  const starts=[];
  for(let i=0;i+3<bytes.length;i++){
   if(bytes[i]===0&&bytes[i+1]===0){
    if(bytes[i+2]===1){starts.push([i,i+3]);i+=2;}
    else if(bytes[i+2]===0&&bytes[i+3]===1){starts.push([i,i+4]);i+=3;}
    if(starts.length>256)throw new Error('Слишком много видеоблоков');
   }
  }
  if(!starts.length||starts[0][0]!==0)throw new Error('Некорректный H.264 поток');
  let sps=null,idr=false,pps=false;
  for(let n=0;n<starts.length;n++){
   const begin=starts[n][1],end=n+1<starts.length?starts[n+1][0]:bytes.length;
   if(begin>=end||bytes[begin]&128)throw new Error('Некорректный H.264 блок');
   const type=bytes[begin]&31;idr||=type===5;pps||=type===8;
   if(type!==7)continue;
   if(end-begin>1024)throw new Error('Некорректные параметры видео');
   const rbsp=[];
   for(let i=begin+1;i<end;i++){
    if(bytes[i]===3&&i>=begin+3&&bytes[i-1]===0&&bytes[i-2]===0)continue;
    rbsp.push(bytes[i]);
   }
   let position=0;
   const bits=count=>{if(count>32||position+count>rbsp.length*8)throw new Error('Неполные параметры видео');let value=0;for(let i=0;i<count;i++,position++)value=value*2+((rbsp[position>>3]>>(7-(position&7)))&1);return value;};
   const ue=()=>{let zeros=0;while(bits(1)===0){if(++zeros>24)throw new Error('Некорректные параметры видео');}return 2**zeros-1+bits(zeros);};
   const bounded=(limit)=>{const value=ue();if(value>limit)throw new Error('Параметры видео превышают лимит');return value;};
   const profile=bits(8),constraints=bits(8),level=bits(8);
   // The sender emits Baseline/I420 only; accepting other profiles would require
   // their chroma-format and bit-depth branches before allocating a decoder.
   if(profile!==66||level>50)throw new Error('Неподдерживаемый профиль H.264');
   bounded(31);bounded(12);const order=bounded(2);
   if(order===0)bounded(12);
   else if(order===1){bits(1);bounded(1<<20);bounded(1<<20);const cycle=bounded(255);for(let i=0;i<cycle;i++)bounded(1<<20);}
   bounded(16);bits(1);const w=bounded(255),h=bounded(255),frameOnly=bits(1);
   if(!frameOnly)bits(1);bits(1);
   let left=0,right=0,top=0,bottom=0;
   if(bits(1)){left=bounded(4096);right=bounded(4096);top=bounded(4096);bottom=bounded(4096);}
   const codedWidth=(w+1)*16,codedHeight=(h+1)*16*(2-frameOnly);
   const width=codedWidth-2*(left+right),height=codedHeight-2*(2-frameOnly)*(top+bottom);
   if(width<1||height<1||codedWidth>4096||codedHeight>4096||codedWidth*codedHeight>8388608)throw new Error('Размер H.264 превышает лимит');
   const value={width,height,codec:`avc1.${[profile,constraints,level].map(v=>v.toString(16).padStart(2,'0')).join('').toUpperCase()}`};
   if(sps&&(sps.width!==width||sps.height!==height||sps.codec!==value.codec))throw new Error('Неоднозначные параметры видео');
   sps=value;
  }
  return {sps,idr,pps};
 }
 push(frame){
  if(this.closed)return false;
  if(!frame||frame.codec!=='h264')return false;
  const {width,height,timestamp_us:timestamp,sequence,screen_epoch:epoch,keyframe}=frame;
  if(!Number.isInteger(width)||!Number.isInteger(height)||width<1||height<1||width>4096||height>4096||width*height>8388608
    ||!Number.isSafeInteger(timestamp)||timestamp<0||!Number.isSafeInteger(sequence)||sequence<0||!Number.isSafeInteger(epoch)||epoch<0
    ||typeof keyframe!=='boolean'||!frame.bytes||frame.bytes.length<4||frame.bytes.length>4194304){this.fail(new Error('Недопустимый видеокадр'));return false;}
  const bytes=frame.bytes instanceof Uint8Array?frame.bytes:new Uint8Array(frame.bytes);
  let inspected;
  try{
   inspected=AssiVideoDecoder.inspect(bytes);
   if(inspected.sps&&(inspected.sps.width!==width||inspected.sps.height!==height))throw new Error('Размер H.264 не соответствует кадру');
   if(keyframe&&(!inspected.sps||!inspected.idr||!inspected.pps))throw new Error('Ключевой кадр H.264 неполон');
   if(!keyframe&&inspected.idr)throw new Error('Тип H.264 не соответствует кадру');
  }catch(error){this.fail(error);return false;}
  if(this.epoch===epoch&&this.lastSequence!==null&&sequence<=this.lastSequence)return false;
  const changed=this.epoch!==null&&(epoch!==this.epoch||width!==this.width||height!==this.height);
  const gap=this.lastSequence!==null&&sequence!==this.lastSequence+1;
  if(changed||gap||this.decoder?.decodeQueueSize>=4||this.pending.size>=8){this.reset();}
  if(this.lastTimestamp!==null&&timestamp<=this.lastTimestamp){this.reset();this.requestKeyframe();return false;}
  if(this.waiting&&!keyframe){this.requestKeyframe();return false;}
  try{
   if(!this.decoder){
    const codec=inspected.sps?.codec;
    if(!keyframe||!codec){this.requestKeyframe();return false;}
    const generation=this.generation;
    this.decoder=new globalThis.VideoDecoder({
     output:decoded=>{
      try{
       if(this.closed||this.generation!==generation)return;
       const meta=this.pending.get(decoded.timestamp);if(!meta)return;
       this.pending.delete(decoded.timestamp);
       if(decoded.displayWidth!==meta.width||decoded.displayHeight!==meta.height){this.fail(new Error('Размер видеокадра изменился'));return;}
       if(this.canvas.width!==meta.width)this.canvas.width=meta.width;
       if(this.canvas.height!==meta.height)this.canvas.height=meta.height;
       const context=this.canvas.getContext('2d',{alpha:false});if(!context)throw new Error('Видеоповерхность недоступна');
       context.drawImage(decoded,0,0,meta.width,meta.height);this.onFrame(meta);
      }catch(error){this.fail(error);}finally{decoded.close();}
     },
     error:error=>{if(this.generation===generation&&!this.closed)this.fail(error);}
    });
    this.decoder.configure({codec,codedWidth:width,codedHeight:height,optimizeForLatency:true});
   }
   this.pending.set(timestamp,{codec:'h264',width,height,timestamp_us:timestamp,sequence,screen_epoch:epoch,keyframe});
   this.decoder.decode(new globalThis.EncodedVideoChunk({type:keyframe?'key':'delta',timestamp,data:bytes}));
   this.waiting=false;this.requested=false;this.lastSequence=sequence;this.lastTimestamp=timestamp;this.epoch=epoch;this.width=width;this.height=height;
   return true;
  }catch(error){this.fail(error);return false;}
 }
}
globalThis.AssiVideoDecoder=AssiVideoDecoder;
if(typeof module!=='undefined')module.exports={AssiVideoDecoder};
