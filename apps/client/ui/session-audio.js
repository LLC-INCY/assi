'use strict';
const sessionAudio={enabled:false,volume:.7,context:null,gain:null,nodes:new Set(),next:0,epoch:null,request:null,polling:false,timer:null,generation:0};
function flushAudioPlayback(){for(const node of sessionAudio.nodes){try{node.stop();}catch{}}sessionAudio.nodes.clear();sessionAudio.next=0;}
function stopSessionAudio(){sessionAudio.generation++;sessionAudio.enabled=false;flushAudioPlayback();const context=sessionAudio.context;sessionAudio.context=null;sessionAudio.gain=null;sessionAudio.epoch=null;sessionAudio.request=null;if(context)context.close().catch(()=>{});}
async function enableSessionAudio(enabled){
 stopSessionAudio();if(!enabled)return;
 const Context=window.AudioContext||window.webkitAudioContext;if(!Context)throw Error('Воспроизведение звука недоступно');
 const generation=sessionAudio.generation,context=new Context();sessionAudio.context=context;sessionAudio.gain=context.createGain();sessionAudio.gain.gain.value=sessionAudio.volume;sessionAudio.gain.connect(context.destination);sessionAudio.request=remoteState().request_id;
 await context.resume();if(generation!==sessionAudio.generation)return;sessionAudio.enabled=true;pollSessionAudio();
}
function playAudioBatch(batch){
 const a=sessionAudio,s=remoteState();if(!a.enabled||!a.context||batch.request_id!==s.request_id||a.request!==s.request_id||s.state!=='active'||s.role!=='viewer')return;
 if(batch.audio_epoch<Math.max(a.epoch||0,s.audio_epoch||0))return;
 if(!batch.audio_enabled){flushAudioPlayback();a.epoch=batch.audio_epoch;return;}
 if(a.epoch!==batch.audio_epoch)flushAudioPlayback();
 if(!s.audio_enabled||batch.audio_epoch!==s.audio_epoch)return;
 if(a.epoch!==batch.audio_epoch){flushAudioPlayback();a.epoch=batch.audio_epoch;}
 if(!batch.audio_enabled){flushAudioPlayback();return;}
 const now=a.context.currentTime;if(a.next<now||a.next>now+.25){flushAudioPlayback();a.next=now+.04;}
 for(const packet of batch.packets.slice(-10)){
  if(packet.sample_rate!==48000||![1,2].includes(packet.channels)||packet.pcm.length!==960*packet.channels*2)continue;
  const data=new DataView(Uint8Array.from(packet.pcm).buffer),buffer=a.context.createBuffer(packet.channels,960,48000);
  for(let ch=0;ch<packet.channels;ch++){const samples=buffer.getChannelData(ch);for(let i=0;i<960;i++)samples[i]=data.getInt16((i*packet.channels+ch)*2,true)/32768;}
  const node=a.context.createBufferSource();node.buffer=buffer;node.connect(a.gain);node.onended=()=>{a.nodes.delete(node);node.disconnect();};a.nodes.add(node);node.start(a.next);a.next+=.02;
 }
}
async function pollSessionAudio(){
 if(sessionAudio.polling)return;const s=remoteState();if(!sessionAudio.enabled||s.state!=='active'||s.role!=='viewer')return;
 sessionAudio.polling=true;const generation=sessionAudio.generation;
 try{const batch=await invoke('session_audio',{requestId:s.request_id});if(generation===sessionAudio.generation)playAudioBatch(batch);}catch{if(generation===sessionAudio.generation)flushAudioPlayback();}
 finally{sessionAudio.polling=false;}
}
function updateSessionAudio(){
 const s=remoteState();if(sessionAudio.enabled&&(s.state!=='active'||sessionAudio.request!==s.request_id)){stopSessionAudio();}
 if(sessionAudio.epoch!==null&&sessionAudio.epoch!==s.audio_epoch)flushAudioPlayback();
 const overlay=$('#host-overlay');if(overlay){overlay.disabled=!remote.snapshot?.overlay_supported;overlay.checked=!!remote.snapshot?.overlay_enabled;}
 if($('#audio-hint'))$('#audio-hint').textContent=s.audio_enabled?'Владелец разрешил передачу звука.':'Передачу звука включает владелец экрана.';
 if($('#audio-share'))$('#audio-share').checked=!!s.audio_enabled;
 if($('#audio-error'))$('#audio-error').textContent=remote.snapshot?.audio_error||'';
 $('#audio-tool')?.classList.toggle('audio-on',s.role==='host'?!!s.audio_enabled:sessionAudio.enabled&&s.audio_enabled);
}
