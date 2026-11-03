'use strict';
let fileActionBusy=false;
function fileSize(bytes){return bytes<1024?`${bytes} Б`:bytes<1024**2?`${(bytes/1024).toFixed(1)} КБ`:bytes<1024**3?`${(bytes/1024**2).toFixed(1)} МБ`:`${(bytes/1024**3).toFixed(1)} ГБ`;}
const fileQueue={request:null,items:[],polling:false};
const folderTransfer={request:null,snapshot:null,polling:false};
function transferMarkup(){return transferContentMarkup()+`<div id="folder-transfer-status">${folderTransferMarkup()}</div><div id="file-queue">${fileQueueMarkup()}</div>`;}
function transferContentMarkup(){
 const s=remoteState(),t=s.transfer,supported=remote.snapshot?.files_supported,folderHint=!!t?.name?.endsWith('.assi-folder');
 if(s.state!=='active')return '';
 const attr=`data-request-id="${esc(s.request_id)}"`, busy=fileActionBusy?'disabled':'';
 const offer=`<div class="transfer-add-actions"><button class="button" data-session-action="file-offer" ${attr} ${busy} ${supported?'':'disabled'}>${icon('folder')}Файлы</button><button class="button" data-session-action="file-offer-folder" ${attr} ${busy} ${supported?'':'disabled'}>Папка</button></div>`;
 if(!t)return `<div class="transfer-heading"><h3>Файлы</h3>${offer}</div>${supported?'<p class="field-hint">До 32 элементов в очереди. Файл или пакет папки — до 1 ГБ. Получатель подтверждает каждую передачу.</p>':'<p class="field-hint">Выбор и сохранение файлов на Android ещё недоступны.</p>'}`;
 const labels={offered:t.direction==='receiving'?'Входящий файл':'Ожидаем получателя',transferring:'Передача файла',paused:t.pause_ready?'Передача приостановлена':'Приостанавливаем передачу…',completed:'Файл передан',rejected:'Файл отклонён',cancelled:'Передача отменена',error:'Не удалось передать файл'};
 const ids=`${attr} data-transfer-id="${esc(t.id)}"`;
 const progress=Math.min(100,t.size?Math.round(t.transferred/t.size*100):t.state==='completed'?100:0);
 let actions='';
 if(t.state==='offered'&&t.direction==='receiving')actions=`<button class="button" data-session-action="file-reject" ${ids} ${busy}>Отклонить</button><button class="button ${folderHint?'':'primary'}" data-session-action="file-accept" ${ids} ${busy} ${supported?'':'disabled'}>${folderHint?'Сохранить пакет файлом':'Выбрать место и принять'}</button>${folderHint?`<button class="button primary" data-session-action="file-accept-folder" ${ids} ${busy} ${supported?'':'disabled'}>Принять и распаковать папку</button>`:''}`;
 else if(['offered','transferring','paused'].includes(t.state)){
  const pause=t.state==='transferring'?`<button class="button" data-session-action="file-pause" ${ids} ${busy}>Приостановить</button>`:'';
  const resume=t.state==='paused'?`<button class="button" data-session-action="file-resume" ${ids} ${busy} ${!t.pause_ready||t.direction==='sending'&&t.resume_requested?'disabled':''}>${!t.pause_ready?'Приостанавливаем…':t.direction==='receiving'?(t.resume_requested?'Разрешить продолжение':'Продолжить передачу'):(t.resume_requested?'Ожидаем разрешение':'Запросить продолжение')}</button>`:'';
  actions=pause+resume+`<button class="button" data-session-action="file-cancel" ${ids} ${busy}>Отменить передачу</button>`;
 }
 else actions=offer;
 return `<div class="transfer-heading"><h3>${esc(labels[t.state]||'Файлы')}</h3><span class="field-hint">${t.direction==='receiving'?'Получение':'Отправка'}</span></div><strong class="transfer-name">${esc(t.name)}</strong><p class="field-hint">${esc(fileSize(t.size))}</p>${['transferring','paused'].includes(t.state)?`<progress id="file-progress" aria-label="Передано" value="${progress}" max="100"></progress><p id="file-progress-text" class="field-hint">${fileSize(t.transferred)} из ${fileSize(t.size)}</p>`:''}${t.error?`<p role="alert" class="form-error">${esc(t.error)}</p>`:''}${t.state==='offered'&&t.direction==='receiving'?`<p class="field-hint">${folderHint?'«Принять и распаковать» создаст новую папку в выбранном месте после проверки пакета. Существующие файлы не заменяются.':'Файл не откроется автоматически. Выберите новое имя: существующие файлы не заменяются.'}</p>`:''}${t.state==='paused'?'<p class="field-hint">Продолжение доступно в этом сеансе. При завершении соединения незавершённый файл удаляется.</p>':''}<div class="session-actions">${actions}</div>`;
}
function transferShape(t){if(!t)return '';const {transferred,...rest}=t;return JSON.stringify(rest);}
function updateTransfer(previous){
 const panel=$('#transfer-panel');if(!panel)return;
 const t=remoteState().transfer;
 if(transferShape(previous)!==transferShape(t)){const focused=document.activeElement?.closest('[data-session-action]')?.dataset.sessionAction;panel.innerHTML=transferMarkup();if(focused?.startsWith('file-')){const fallback=focused==='file-pause'?'file-resume':focused==='file-resume'?'file-pause':'file-cancel';(panel.querySelector(`[data-session-action="${focused}"]:not(:disabled)`)||panel.querySelector(`[data-session-action="${fallback}"]:not(:disabled)`)||panel).focus({preventScroll:true});}}
 else if(t&&$('#file-progress')){$('#file-progress').value=Math.min(100,t.size?Math.round(t.transferred/t.size*100):0);$('#file-progress-text').textContent=`${fileSize(t.transferred)} из ${fileSize(t.size)}`;}
}
async function fileAction(action,button){
 if(fileActionBusy||!button)return;
 const requestId=button.dataset.requestId,transferId=button.dataset.transferId;
 const commands={'file-offer':'session_file_offer','file-offer-folder':'session_file_offer_folder','file-accept-folder':'session_file_accept_folder','file-accept':'session_file_accept','file-reject':'session_file_reject','file-cancel':'session_file_cancel','file-pause':'session_file_pause','file-resume':'session_file_resume'};
 if(action==='file-queue-remove'){try{await invoke('session_file_queue_remove',{requestId,id:Number(button.dataset.queueId)});await pollFileQueue();}catch(error){toast(error);}return;}
 if(!commands[action])return;
 fileActionBusy=true;button.disabled=true;
 try{await invoke(commands[action],{requestId,...(transferId?{transferId}:{})});await refreshSession();await pollFileQueue();await pollFolderTransfer();}
 catch(error){if(typeof remoteState!=='function'||remoteState().request_id===requestId)toast(error);}finally{fileActionBusy=false;if(button.isConnected)button.disabled=false;if(typeof remoteState!=='function'||remoteState().request_id===requestId){const panel=$('#transfer-panel');if(panel)panel.innerHTML=transferMarkup();}}
}

function fileQueueMarkup(){if(fileQueue.request!==remoteState().request_id||!fileQueue.items.length)return '';const labels={queued:'В очереди',starting:'Подготовка',active:'Передача',paused:'Приостановлен',completed:'Передан',cancelled:'Отменён',error:'Ошибка'};return `<ol class="file-queue-list" aria-label="Очередь файлов">${fileQueue.items.map(item=>`<li><span><strong>${esc(item.name)}</strong><small>${item.kind==='folder'?'Папка · ':''}${esc(labels[item.state]||item.state)} · ${fileSize(item.size)}${item.error?' · '+esc(item.error):''}</small></span>${['starting','active','paused'].includes(item.state)?'':`<button type="button" class="text-button" data-session-action="file-queue-remove" data-request-id="${esc(fileQueue.request)}" data-queue-id="${item.id}" aria-label="Убрать из очереди ${esc(item.name)}">×</button>`}</li>`).join('')}</ol>`;}
async function pollFileQueue(){const s=remoteState(),id=s.request_id;if(s.state!=='active'||!remote.snapshot?.files_supported||fileQueue.polling)return;if(fileQueue.request!==id){fileQueue.request=id;fileQueue.items=[];}fileQueue.polling=true;try{const items=await invoke('session_file_queue',{requestId:id});if(remoteState().state!=='active'||remoteState().request_id!==id)return;if(JSON.stringify(items)!==JSON.stringify(fileQueue.items)){fileQueue.items=items.slice(0,32);const panel=$('#file-queue');if(panel)panel.innerHTML=fileQueueMarkup();}}catch{}finally{fileQueue.polling=false;}}

function folderTransferMarkup(){const value=folderTransfer.snapshot;if(folderTransfer.request!==remoteState().request_id||!value||value.state==='idle')return '';const labels={receiving:'Получаем пакет папки',extracting:'Проверяем и распаковываем папку…',completed:'Папка сохранена',cancelled:'Получение папки отменено',error:'Не удалось сохранить папку'};return `<p class="field-hint${value.state==='error'?' form-error':''}" role="status">${esc(labels[value.state]||value.state)}${value.name?' · '+esc(value.name):''}${value.error?' · '+esc(value.error):''}</p>`;}
async function pollFolderTransfer(){const s=remoteState(),id=s.request_id;if(s.state!=='active'||!remote.snapshot?.files_supported||folderTransfer.polling)return;if(folderTransfer.request!==id){folderTransfer.request=id;folderTransfer.snapshot=null;}folderTransfer.polling=true;try{const next=await invoke('session_file_folder_status',{requestId:id});if(remoteState().state!=='active'||remoteState().request_id!==id||next?.request_id!==id)return;if(JSON.stringify(next)!==JSON.stringify(folderTransfer.snapshot)){folderTransfer.snapshot=next;const panel=$('#folder-transfer-status');if(panel)panel.innerHTML=folderTransferMarkup();}}catch{}finally{folderTransfer.polling=false;}}
