'use strict';
// Video-only local recording: canvas capture never requests microphone/system audio.
class AssiRecordingQueue {
 constructor(invoke,requestId,recordingId){this.invoke=invoke;this.requestId=requestId;this.recordingId=recordingId;this.pending=0;this.tail=Promise.resolve();this.error=null;this.closed=false;}
 enqueue(blob){
  if(this.closed||this.error)return Promise.reject(this.error||new Error('Запись уже закрыта'));
  const count=Math.ceil(blob.size/1048576);if(!count)return Promise.resolve();
  if(count>4||this.pending+count>4){this.error=new Error('Диск не успевает сохранять видео. Запись остановлена');return Promise.reject(this.error);}
  const jobs=[];
  for(let offset=0;offset<blob.size;offset+=1048576){
   const part=blob.slice(offset,offset+1048576);this.pending++;
   const work=this.tail.then(async()=>{
    if(this.error)throw this.error;
    const bytes=new Uint8Array(await part.arrayBuffer());
    await this.invoke('recording_chunk',{requestId:this.requestId,recordingId:this.recordingId,bytes:Array.from(bytes)});
   });
   this.tail=work.catch(error=>{this.error=error;}).finally(()=>{this.pending--;});
   jobs.push(work);
  }
  return Promise.all(jobs);
 }
 async drain(){await this.tail;if(this.error)throw this.error;}
}
globalThis.AssiRecordingQueue=AssiRecordingQueue;
const sessionRecording={starting:false,current:null};
function recordingMime(){
 if(typeof MediaRecorder!=='function'||typeof MediaRecorder.isTypeSupported!=='function')return null;
 return ['video/webm;codecs=vp9','video/webm;codecs=vp8','video/mp4','video/webm'].find(type=>MediaRecorder.isTypeSupported(type))||null;
}
function updateSessionRecording(){
 const current=sessionRecording.current,button=document.getElementById('session-recording-toggle'),status=document.getElementById('session-recording-status');
 const supported=state.platform!=='Android'&&!!recordingMime()&&typeof HTMLCanvasElement!=='undefined'&&typeof HTMLCanvasElement.prototype.captureStream==='function';
 if(button){button.title=supported?'Запись видео с пометками, без звука':state.platform==='Android'?'Сохранение записи на Android пока недоступно':'Запись не поддерживается этим WebView';button.textContent=current?(current.stopping?'Сохраняем видео…':'Остановить запись'):'Записать видео без звука';button.disabled=sessionRecording.starting||!!current?.stopping||(!current&&!supported);}
 if(status){status.hidden=!current;if(current){const elapsed=Math.floor((performance.now()-current.started)/1000);status.textContent=`${Math.floor(elapsed/60).toString().padStart(2,'0')}:${(elapsed%60).toString().padStart(2,'0')}`;status.title='Запись видео без звука';status.setAttribute('aria-label',`Идёт запись видео без звука, ${elapsed} секунд`);}}
}
function paintRecordingFrame(current){
 const s=remoteState(),surface=remoteFrameSurface(),dimensions=remoteFrameDimensions();
 if(s.request_id!==current.requestId||s.state!=='active'||!surface||!surface.isConnected){void stopSessionRecording();return;}
 if(!dimensions.width||!dimensions.height)return;
 const ctx=current.canvas.getContext('2d',{alpha:false}),w=current.canvas.width,h=current.canvas.height;
 const scale=Math.min(w/dimensions.width,h/dimensions.height),width=dimensions.width*scale,height=dimensions.height*scale,x=(w-width)/2,y=(h-height)/2;
 ctx.fillStyle='#000';ctx.fillRect(0,0,w,h);ctx.drawImage(surface,x,y,width,height);
 const colors={mint:'#a3f3d5',amber:'#ffd166',rose:'#ff7e9d',blue:'#78b9ff',white:'#fff'};
 for(const stroke of (typeof sessionView==='undefined'?[]:sessionView.board.strokes)){
  const points=stroke.points;if(!points?.length)continue;const a=points[0],b=points.at(-1);
  ctx.save();ctx.translate(x,y);ctx.strokeStyle=colors[stroke.color]||colors.mint;ctx.fillStyle=ctx.strokeStyle;ctx.lineWidth=stroke.width*Math.min(width,height)/720;ctx.lineCap='round';ctx.lineJoin='round';ctx.beginPath();
  if(stroke.tool==='rectangle')ctx.rect(Math.min(a.x,b.x)*width,Math.min(a.y,b.y)*height,Math.abs(a.x-b.x)*width,Math.abs(a.y-b.y)*height);
  else if(stroke.tool==='arrow'){
   const ax=a.x*width,ay=a.y*height,bx=b.x*width,by=b.y*height,angle=Math.atan2(by-ay,bx-ax),len=Math.max(ctx.lineWidth*4,Math.min(width,height)*.018);
   ctx.moveTo(ax,ay);ctx.lineTo(bx,by);ctx.moveTo(bx-len*Math.cos(angle-.5),by-len*Math.sin(angle-.5));ctx.lineTo(bx,by);ctx.lineTo(bx-len*Math.cos(angle+.5),by-len*Math.sin(angle+.5));
  }else if(points.length===1){ctx.arc(a.x*width,a.y*height,ctx.lineWidth/2,0,Math.PI*2);ctx.fill();}
  else{ctx.moveTo(a.x*width,a.y*height);for(const p of points.slice(1))ctx.lineTo(p.x*width,p.y*height);}
  ctx.stroke();ctx.restore();
 }
}
async function startSessionRecording(){
 if(sessionRecording.starting||sessionRecording.current)return;
 const request=remoteState(),mime=recordingMime();
 if(request.state!=='active'){toast('Начните сеанс перед записью');return;}
 if(typeof state!=='undefined'&&state.platform==='Android'){toast('Сохранение записи на Android пока недоступно');return;}
 const dimensions=remoteFrameDimensions();
 if(!mime||typeof HTMLCanvasElement==='undefined'||typeof HTMLCanvasElement.prototype.captureStream!=='function'){toast('Запись видео недоступна в этом системном браузере');return;}
 if(!dimensions.width||!dimensions.height){toast('Дождитесь изображения экрана');return;}
 sessionRecording.starting=true;updateSessionRecording();let current=null,receipt=null;
 try{
  receipt=await invoke('recording_start',{requestId:request.request_id,mime});if(!receipt)return;
  if(remoteState().request_id!==request.request_id||remoteState().state!=='active'||!remoteFrameSurface()?.isConnected)throw new Error('Экран сеанса закрыт до начала записи');
  const canvas=document.createElement('canvas'),scale=Math.min(1,1920/Math.max(dimensions.width,dimensions.height));
  canvas.width=Math.max(2,Math.floor(dimensions.width*scale/2)*2);canvas.height=Math.max(2,Math.floor(dimensions.height*scale/2)*2);
  current={requestId:request.request_id,recordingId:receipt.id,canvas,started:performance.now(),encodedBytes:0,stopping:null,stream:null,recorder:null,timer:null,sink:new AssiRecordingQueue(invoke,request.request_id,receipt.id),error:null};
  sessionRecording.current=current;paintRecordingFrame(current);
  current.stream=canvas.captureStream(15);
  current.recorder=new MediaRecorder(current.stream,{mimeType:mime,videoBitsPerSecond:3_000_000});
  current.recorder.addEventListener('dataavailable',event=>{
   if(sessionRecording.current!==current||!event.data?.size)return;
   current.encodedBytes+=event.data.size;
   current.sink.enqueue(event.data).catch(error=>{current.error=error;if(sessionRecording.current===current)void stopSessionRecording({discard:true});});
   // Reserve a bounded event plus the final codec tail before the native 512 MiB cap.
   if(current.encodedBytes>=504*1048576&&!current.stopping)void stopSessionRecording();
  });
  current.recorder.addEventListener('error',event=>{current.error=event.error||new Error('Видеозапись прервана');if(sessionRecording.current===current)void stopSessionRecording({discard:true});});
  current.recorder.start(250);
  current.timer=setInterval(()=>{
   if(current.stopping)return;
   if(performance.now()-current.started>=29*60*1000+59*1000){void stopSessionRecording();return;}
   try{paintRecordingFrame(current);updateSessionRecording();}catch(error){current.error=error;void stopSessionRecording({discard:true});}
  },1000/15);
  toast('Идёт запись видео без звука');
 }catch(error){
  if(current)current.sink.closed=true;if(sessionRecording.current===current)sessionRecording.current=null;
  current?.stream?.getTracks().forEach(track=>track.stop());if(current?.timer)clearInterval(current.timer);
  if(receipt)try{await invoke('recording_abort',{requestId:request.request_id,recordingId:receipt.id});}catch{}
  toast(error);
 }finally{sessionRecording.starting=false;updateSessionRecording();}
}
async function stopSessionRecording({discard=false}={}){
 const current=sessionRecording.current;if(!current)return;
 if(discard)current.error??=new Error('Запись остановлена из-за ошибки');
 if(current.stopping)return current.stopping;
 clearInterval(current.timer);
 current.stopping=(async()=>{
  try{
   await invoke('recording_prepare_finish',{requestId:current.requestId,recordingId:current.recordingId});
   if(current.recorder&&current.recorder.state!=='inactive')await new Promise((resolve,reject)=>{
    const timeout=setTimeout(()=>reject(new Error('Видеокодек не завершил запись вовремя')),4000);
    current.recorder.addEventListener('stop',()=>{clearTimeout(timeout);resolve();},{once:true});current.recorder.stop();
   });
   await current.sink.drain();if(current.error)throw current.error;
   await invoke('recording_finish',{requestId:current.requestId,recordingId:current.recordingId});toast('Видео без звука сохранено');
  }catch(error){
   try{await invoke('recording_abort',{requestId:current.requestId,recordingId:current.recordingId});}catch{}
   toast(error);
  }finally{
   current.sink.closed=true;current.stream?.getTracks().forEach(track=>track.stop());
   if(current.recorder?.state!=='inactive')try{current.recorder?.stop();}catch{}
   if(sessionRecording.current===current)sessionRecording.current=null;updateSessionRecording();
  }
 })();updateSessionRecording();return current.stopping;
}
function toggleSessionRecording(){return sessionRecording.current?stopSessionRecording():startSessionRecording();}
function sessionRecordingCleanup(){return stopSessionRecording();}
if(typeof module!=='undefined')module.exports={AssiRecordingQueue};
