'use strict';
// Native session state is authoritative. This module never simulates a connection.
const remote = { snapshot:null, invitation:'', polling:false, frameBusy:false, frameUrl:null, frameTimer:null, history:[], stopped:false, inputQueue:[], sending:false, generation:0, pollTimer:null, initialized:false, dirty:false, relayConfig:'', videoSequence:0, displayedEpoch:0, ackedEpoch:0, videoVisible:false, decoder:null, videoRequest:null, negotiated:null, videoSizes:new Map(),hostPreviewAt:0 };
const sessionLabels={idle:'Не подключено',listening:'Ожидание подключения',connecting:'Подключаемся…',pending:'Ожидание подтверждения',active:'Сеанс активен',closed:'Сеанс завершён',error:'Ошибка подключения'};
function remoteState(){return remote.snapshot?.session||{state:'idle',role:'idle',control:false};}
function remoteNotice(){const s=remoteState();if(s.transfer?.state==='offered'&&s.transfer.direction==='receiving')return 'Входящий файл — требуется подтверждение';return s.state==='active'?(s.role==='host'?'Ваш экран виден другому участнику':'Вы просматриваете удалённый экран'):s.state==='pending'&&s.role==='host'?'Входящий запрос на доступ':sessionLabels[s.state]||'Не подключено';}
function liveSessionMarkup(){
 const s=remoteState(),hasHistory=state.tab==='history';
 const tabs=`<div class="session-tabs" role="tablist" aria-label="Список сеансов">${[['active','Текущий'],['history','История']].map(([id,label])=>`<button id="tab-${id}" role="tab" aria-controls="session-panel" data-tab="${id}" aria-selected="${state.tab===id}" tabindex="${state.tab===id?0:-1}">${label}</button>`).join('')}</div>`;
 let content;
 if(hasHistory){content=remote.history.length?remote.history.map(x=>`<article class="session-item"><strong>${esc(x.peer)}</strong><p>${esc(x.time)} · Завершён</p></article>`).join(''):'<div class="empty"><h3>История пока пуста</h3><p>Здесь появятся завершённые сеансы этого запуска приложения.</p></div>';}
 else if(s.state==='pending'&&s.role==='host'){
 content=`<section class="consent-card"><h3>Разрешить доступ к экрану?</h3><p><strong>${esc(s.peer_name||'Другой участник')}</strong></p><p class="field-hint">Имя указано отправителем. Подтверждайте только ожидаемый запрос. ${state.platform==='Android'?'Система предложит выбрать экран или приложение для показа.':'Будет виден основной экран целиком.'}</p><label class="check-field"><input id="allow-control" type="checkbox" ${remote.snapshot?.capabilities?.control?'':'disabled'}><span>Разрешить управление мышью и клавиатурой</span></label><p class="field-hint">Без этой отметки доступен только просмотр. Доступ можно прекратить в любой момент.</p><div class="session-actions"><button class="button" data-session-action="reject" data-request-id="${esc(s.request_id)}">Отклонить</button><button class="button primary" data-session-action="accept" data-request-id="${esc(s.request_id)}">Разрешить</button></div></section>`;
 }else if(s.state==='active'){
 content=`<section class="live-session"><div class="session-status"><span class="status"><span class="dot"></span>${s.role==='host'?'Ваш экран виден другому участнику':'Вы просматриваете удалённый экран'}</span><span>${s.control?'Просмотр и управление':'Только просмотр'}</span></div>${sessionViewportMarkup()}</section><section id="transfer-panel" class="transfer-panel" tabindex="-1">${transferMarkup()}</section>`;
 }else if(['connecting','pending','listening'].includes(s.state)){
 content=`<div class="empty"><h3>${esc(sessionLabels[s.state])}</h3><p>${s.state==='listening'?'Передайте приглашение человеку, которому хотите показать экран.':s.state==='pending'?'Владелец устройства должен подтвердить запрос.':'Проверяем приглашение и устанавливаем защищённое соединение.'}</p><div class="session-actions">${s.state==='listening'?'<button class="button" data-session-action="share">Показать приглашение</button>':''}<button class="button" data-session-action="stop">Отменить</button></div></div>`;
 }else{content=`<div class="empty"><h3>${s.state==='error'?'Не удалось подключиться':'Начните подключение'}</h3>${s.error?`<p class="form-error" role="alert">${esc(s.error)}</p>`:''}${remote.snapshot?.media_error?`<p class="form-error" role="alert">${esc(remote.snapshot.media_error)}</p>`:''}<p>${state.viewerOnly?'Вставьте приглашение с другого устройства.':'Вставьте приглашение с другого устройства или разрешите просмотр своего экрана.'}</p><div class="session-actions"><button class="button primary" data-session-action="connect">Подключиться</button>${state.viewerOnly?'':'<button class="button" data-session-action="host">Предоставить доступ</button>'}</div></div>`;}
 return `<div class="page-heading"><h2>Сеансы</h2><p>Защищённое соединение с подтверждением владельца.</p></div><div id="session-windows">${typeof sessionWindowMarkup==='function'?sessionWindowMarkup():''}</div>${tabs}<div id="session-panel" role="tabpanel" aria-labelledby="tab-${state.tab}">${content}</div>`;
}
function renderLiveSessions(){ sessionViewCleanup();resetSessionVideo();$('#main').innerHTML=liveSessionMarkup();bindRemoteInput(); }
function paintSessionChrome(){
 const s=remoteState(),banner=$('#session-banner');
 const visible=s.state==='active'||(s.state==='pending'&&s.role==='host');
 banner.hidden=!visible;banner.querySelector('[data-session-status]').textContent=remoteNotice();
 const incoming=s.transfer?.state==='offered'&&s.transfer.direction==='receiving';const open=banner.querySelector('button');open.textContent=incoming?'Открыть файл':'Открыть сеанс';open.dataset.focusTransfer=String(incoming);
 const own=$('#own-code');own.textContent=s.role==='host'&&['listening','pending','active'].includes(s.state)?'Приём включён':'Приём выключен';
 $('.code-label').textContent='Доступ';
 const receiving=s.role==='host'&&['listening','pending'].includes(s.state);
 $('#access-control').textContent=s.state==='active'?'Завершить сеанс':receiving?'Выключить приём':'Предоставить доступ';
 $('#access-control').dataset.action=s.state==='active'||receiving?'stop-session':'access';
 $('#access-control').classList.toggle('danger',s.state==='active'||receiving);
}
async function refreshSession(){
 if(remote.polling||!state.data)return;remote.polling=true;
 try{const previous=remoteState(),previousError=remote.snapshot?.media_error;const next=await invoke('session_snapshot');remote.snapshot=next;const s=remoteState();$('#session-network-error').textContent='';if(previous.request_id!==s.request_id||previous.state!==s.state){remote.generation++;remote.inputQueue=[];}
 if(previous.state==='active'&&(s.state!=='active'||previous.request_id!==s.request_id)){remote.history.unshift({peer:previous.peer_name||'Удалённое устройство',time:new Date().toLocaleTimeString('ru',{hour:'2-digit',minute:'2-digit'})});remote.history=remote.history.slice(0,50);clearRemoteFrame();resetSessionView(null);}
 updateSessionAudio();
 const oldTransfer=previous.transfer,newTransfer=s.transfer;
 const stable=value=>[value.state,value.role,value.request_id,value.peer_name,value.control,value.error];
 const changed=JSON.stringify(stable(previous))!==JSON.stringify(stable(s))||previousError!==next.media_error;
 if(previous.screen_epoch!==s.screen_epoch||previous.screen_pending!==s.screen_pending){releaseRemoteInput();updateRemoteDisplays();}
 if(typeof updateSessionTools==='function')updateSessionTools();
 if(typeof pollSessionTools==='function')pollSessionTools();
 if(typeof pollTerminal==='function')pollTerminal();
 if(typeof pollSystemTools==='function')pollSystemTools();
 if(typeof pollFileQueue==='function')pollFileQueue();
 if(typeof pollFolderTransfer==='function')pollFolderTransfer();
 if(typeof pollSessionWindows==='function')pollSessionWindows();
 if(changed){paintSessionChrome();if(state.page==='sessions'){if($('#dialog').open)remote.dirty=true;else renderLiveSessions();}}else if(JSON.stringify(oldTransfer)!==JSON.stringify(newTransfer)){paintSessionChrome();if($('#dialog').open)remote.dirty=true;else updateTransfer(oldTransfer);}
 }catch(error){$('#session-network-error').textContent='Не удалось обновить состояние соединения. Повторяем…';}
 finally{remote.polling=false;clearTimeout(remote.pollTimer);remote.pollTimer=setTimeout(refreshSession,700);}
}
function resetSessionVideo(){remote.decoder?.close();remote.decoder=null;remote.videoSequence=0;remote.displayedEpoch=0;remote.ackedEpoch=0;remote.videoVisible=false;remote.videoRequest=null;remote.negotiated=null;remote.videoSizes.clear();}
function clearRemoteFrame(){resetSessionVideo();if(remote.frameUrl)URL.revokeObjectURL(remote.frameUrl);remote.frameUrl=null;const image=$('#remote-frame');if(image)image.removeAttribute('src');}
async function requestVideoKeyframe(){const id=remoteState().request_id;try{if(remoteState().role==='viewer'&&remoteState().state==='active')await invoke('session_video_keyframe',{requestId:id});}catch{}}
async function fallBackToJpeg(){const id=remoteState().request_id;remote.decoder?.close();remote.decoder=null;remote.negotiated='jpeg';try{if(remoteState().role==='viewer'&&remoteState().state==='active')await invoke('session_video_codec',{requestId:id,codec:'jpeg'});}catch(error){if(remoteState().request_id===id)toast(error);}}
function frameDisplayed(frame,id){
 if(remoteState().state!=='active'||remoteState().request_id!==id)return;
 remote.displayedEpoch=frame.screen_epoch;$('#frame-wait').hidden=true;layoutRemoteStage();
 if(typeof noteVideoFrame==='function')noteVideoFrame(frame);
 if(remoteState().role==='viewer'&&remote.ackedEpoch!==frame.screen_epoch){remote.ackedEpoch=frame.screen_epoch;invoke('session_screen_ack',{requestId:id,screenEpoch:frame.screen_epoch}).catch(()=>{if(remoteState().request_id===id&&remote.ackedEpoch===frame.screen_epoch)remote.ackedEpoch=0;});}
}
async function showJpeg(bytes,id,frame){
 const image=$('#remote-frame');if(!image||!bytes?.length)return;
 const url=URL.createObjectURL(new Blob([new Uint8Array(bytes)],{type:'image/jpeg'})),old=remote.frameUrl;
 image.src=url;
 try{await image.decode();if(remoteState().state!=='active'||remoteState().request_id!==id||image!==$('#remote-frame')){URL.revokeObjectURL(url);return;}remote.frameUrl=url;remote.videoVisible=false;image.hidden=false;const canvas=$('#remote-video');if(canvas)canvas.hidden=true;if(old)URL.revokeObjectURL(old);frameDisplayed(frame||{codec:'jpeg',width:image.naturalWidth,height:image.naturalHeight,bytes,screen_epoch:remoteState().screen_epoch},id);}catch{URL.revokeObjectURL(url);}
}
async function refreshFrame(){
 if(remote.frameBusy)return;const s=remoteState();if(s.state!=='active'||state.page!=='sessions'||state.tab!=='active')return;
 remote.frameBusy=true;
 try{
  if(s.role==='host'){if(performance.now()-remote.hostPreviewAt<200)return;remote.hostPreviewAt=performance.now();const bytes=await invoke('session_frame');if(remoteState().request_id===s.request_id)await showJpeg(bytes,s.request_id);return;}
  if(remote.videoRequest!==s.request_id){resetSessionVideo();remote.videoRequest=s.request_id;}
  if(!remote.negotiated){const supported=typeof AssiVideoDecoder!=='undefined'&&await AssiVideoDecoder.supported();if(remoteState().request_id!==s.request_id)return;remote.negotiated=supported?'h264':'jpeg';await invoke('session_video_codec',{requestId:s.request_id,codec:remote.negotiated});if(remote.negotiated==='h264')await requestVideoKeyframe();}
  const frame=await invoke('session_video',{requestId:s.request_id,afterSequence:remote.videoSequence});if(!frame||remoteState().state!=='active'||remoteState().request_id!==s.request_id)return;
  if(frame.sequence<=remote.videoSequence)return;remote.videoSequence=frame.sequence;
  if(frame.codec==='jpeg'){remote.decoder?.reset();await showJpeg(frame.bytes,s.request_id,frame);}
  else if(frame.codec==='h264'){
   const canvas=$('#remote-video');if(!canvas)return;
   if(!remote.decoder){remote.decoder=new AssiVideoDecoder(canvas,{onFrame:meta=>{if(remoteState().request_id!==s.request_id||$('#remote-video')!==canvas)return;remote.videoVisible=true;canvas.hidden=false;$('#remote-frame').hidden=true;const byte_length=remote.videoSizes.get(meta.sequence)||0;remote.videoSizes.delete(meta.sequence);frameDisplayed({...meta,byte_length},s.request_id);},onKeyframe:requestVideoKeyframe,onError:fallBackToJpeg});}
   remote.videoSizes.set(frame.sequence,frame.bytes?.length||0);while(remote.videoSizes.size>8)remote.videoSizes.delete(remote.videoSizes.keys().next().value);remote.decoder.push(frame);
  }
 }catch(error){if(remoteState().request_id===s.request_id&&remote.negotiated==='h264')await fallBackToJpeg();}
 finally{remote.frameBusy=false;}
}
function openConnection(){
 if($('#dialog').open)closeModal();
 modal('Подключиться по приглашению',`<p class="dialog-subtitle">Получите приглашение у владельца устройства. Для прямого приглашения нужна общая сеть. Приглашение через сервер работает между разными сетями, если сервер доступен обоим устройствам.</p><form id="connect-form"><label class="field">Приглашение<textarea name="invitation" id="session-invitation" aria-describedby="connect-error" rows="4" autocomplete="off" spellcheck="false" placeholder="assi://…" required></textarea></label><p id="connect-error" class="form-error" role="alert"></p><div class="dialog-actions"><button class="button" type="button" data-action="close">Отмена</button><button class="button primary" type="submit">Подключиться</button></div></form>`);
 const form=$('#connect-form');form.onsubmit=async e=>{e.preventDefault();const ticket=dialogTasks.capture(),button=form.querySelector('[type=submit]');button.disabled=true;button.textContent='Подключаемся…';try{await invoke('session_connect',{invitation:new FormData(form).get('invitation')});if(ticket()){closeModal();state.tab='active';setPage('sessions');}await refreshSession();}catch(error){if(ticket()){const field=form.querySelector('textarea');field.setAttribute('aria-invalid','true');field.focus();$('#connect-error').textContent=String(error);}else toast(error);}finally{button.disabled=false;button.textContent='Подключиться';}};
}
function openHosting(){
 if(state.viewerOnly)return;
 if($('#dialog').open)closeModal();
 const s=remoteState();if(['listening','pending','active'].includes(s.state)&&s.role==='host'){showInvitation();return;}
 if(!remote.snapshot?.capabilities?.capture){modal('Просмотр этого экрана недоступен',`<p class="dialog-subtitle">${esc(remote.snapshot?.capabilities?.reason||'На этой платформе доступно подключение к другим устройствам.')}</p><div class="dialog-actions"><button class="button" data-action="close">Закрыть</button></div>`);return;}
 modal('Предоставить доступ',`<p class="dialog-subtitle">Передайте приглашение нужному человеку. Показ экрана начнётся после вашего подтверждения и разрешения системы.</p><form id="host-form"><label class="field">Способ подключения<select name="route" id="host-route"><option value="direct">Напрямую — общая сеть</option><option value="relay">Через сервер — разные сети</option></select></label><div id="direct-fields"><label class="field">IP этого устройства<input name="ip" autocomplete="off" spellcheck="false" value="127.0.0.1" required aria-describedby="host-ip-hint"></label><p id="host-ip-hint" class="field-hint">Укажите IP устройства в общей сети. 127.0.0.1 подходит только для проверки на одном компьютере.</p></div><div id="relay-fields" hidden><label class="field">Конфигурация сервера<textarea name="config" rows="3" spellcheck="false" autocomplete="off" placeholder="assi-relay://…" disabled>${esc(remote.relayConfig)}</textarea></label><p class="field-hint">Получите конфигурацию у администратора своего сервера Асси. Общедоступный сервер ещё не запущен.</p></div><p id="host-error" class="form-error" role="alert"></p><div class="dialog-actions"><button class="button" type="button" data-action="close">Отмена</button><button class="button primary" type="submit">Включить приём</button></div></form>`);
 const form=$('#host-form');
 form.elements.route.onchange=()=>{const relay=form.elements.route.value==='relay';$('#relay-fields').hidden=!relay;$('#direct-fields').hidden=relay;form.elements.ip.disabled=relay;form.elements.config.disabled=!relay;form.elements.config.required=relay;};
 form.onsubmit=async e=>{e.preventDefault();const ticket=dialogTasks.capture(),button=form.querySelector('[type=submit]');button.disabled=true;button.textContent='Включаем…';try{const data=new FormData(form),relay=data.get('route')==='relay';const config=String(data.get('config')||'').trim();remote.invitation=await invoke(relay?'session_host_relay':'session_host',relay?{config}:{ip:data.get('ip')});if(relay)remote.relayConfig=config;if(ticket()){closeModal();showInvitation();}await refreshSession();}catch(error){if(ticket()){$('#host-error').textContent=String(error);}else toast(error);}finally{button.disabled=false;button.textContent='Включить приём';}};
}
function showInvitation(){
 modal('Приглашение на подключение',`<p class="dialog-subtitle">Передайте его только тому, кого ожидаете. Приглашение содержит секрет доступа, действует до выключения приёма и всё равно требует вашего подтверждения.</p><label class="field">Приглашение<textarea id="share-invitation" readonly rows="4" spellcheck="false">${esc(remote.invitation)}</textarea></label><p id="share-feedback" role="status" class="field-hint"></p><div class="dialog-actions"><button class="button" data-action="close">Закрыть</button><button class="button primary" id="copy-invitation" ${remote.invitation?'':'disabled'}>Скопировать</button></div>`);
 $('#copy-invitation').onclick=async()=>{const field=$('#share-invitation'),ticket=dialogTasks.capture();try{await navigator.clipboard.writeText(remote.invitation);if(ticket())$('#share-feedback').textContent='Приглашение скопировано';}catch{if(ticket()){field.focus();field.select();$('#share-feedback').textContent='Скопируйте выделенное приглашение сочетанием Ctrl/Cmd + C.';}}};
}
async function sessionAction(action,button){
 if(action.startsWith('file-')){await fileAction(action,button);return;}
 if(action==='host'){openHosting();return;}if(action==='connect'){openConnection();return;}if(action==='share'){showInvitation();return;}
 const requestId=button?.dataset.requestId;
 if(button)button.disabled=true;
 try{if(action==='stop'){remote.generation++;remote.inputQueue=[];await invoke('session_stop');remote.invitation='';clearRemoteFrame();resetSessionView(null);}
 else if(action==='accept'){await invoke('session_accept',{requestId,control:!!$('#allow-control')?.checked});}
 else if(action==='reject'){await invoke('session_reject',{requestId});}
 await refreshSession();}catch(error){toast(error);}finally{if(button)button.disabled=false;}
}
const remoteKeys={Insert:'insert',CapsLock:'caps_lock',...Object.fromEntries(Array.from({length:12},(_,i)=>['F'+(i+1),'f'+(i+1)])),Enter:'enter',Escape:'escape',Backspace:'backspace',Tab:'tab',ArrowUp:'arrow_up',ArrowDown:'arrow_down',ArrowLeft:'arrow_left',ArrowRight:'arrow_right',Control:'control',Alt:'alt',Shift:'shift',Meta:'meta',Delete:'delete',Home:'home',End:'end',PageUp:'page_up',PageDown:'page_down',' ':'space'};
function emitInput(input){
 if(remoteState().role!=='viewer'||!remoteState().control||remoteState().state!=='active')return;
 if(input.type!=='release_all'&&(remoteState().screen_pending||remote.displayedEpoch!==remoteState().screen_epoch))return;
 if(input.type==='move'&&remote.inputQueue.at(-1)?.type==='move')remote.inputQueue[remote.inputQueue.length-1]={...input,screenEpoch:remote.displayedEpoch};
 else if(remote.inputQueue.length<64)remote.inputQueue.push({...input,screenEpoch:remote.displayedEpoch});
 else {remote.inputQueue=[];sessionAction('stop');return;}
 drainInput();
}
async function drainInput(){
 if(remote.sending)return;remote.sending=true;const generation=remote.generation,requestId=remoteState().request_id;
 try{while(remote.inputQueue.length&&generation===remote.generation){const {screenEpoch,...input}=remote.inputQueue.shift();await invoke('session_input',{input,requestId,screenEpoch});}}
 catch{if(generation===remote.generation){remote.inputQueue=[];sessionAction('stop');}}
 finally{remote.sending=false;if(remote.inputQueue.length)drainInput();}
}
function releaseRemoteInput(){remote.inputQueue=[];emitInput({type:'release_all'});}
function bindRemoteInput(){bindSessionViewer();}
function flushSessionView(){if(remote.dirty&&state.page==='sessions'){remote.dirty=false;renderLiveSessions();}}
function initSessionUI(){
 if(remote.initialized)return;remote.initialized=true;paintSessionChrome();
 window.addEventListener('blur',releaseRemoteInput);
 $('#dialog').addEventListener('close',flushSessionView);
 document.addEventListener('click',e=>{const b=e.target.closest('[data-session-action]');if(b&&!b.disabled)sessionAction(b.dataset.sessionAction,b);});
 refreshSession();sessionAudio.timer=setInterval(pollSessionAudio,40);remote.frameTimer=setInterval(refreshFrame,25);setInterval(refreshAnnotations,200);
}
