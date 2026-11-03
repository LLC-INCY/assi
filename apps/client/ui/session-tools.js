'use strict';
// Session text stays in memory and is never applied to the OS clipboard implicitly.
const SessionToolsPolicy={
 text(value,limit){const text=String(value).replace(/\r\n?/g,'\n');return text.length&&new TextEncoder().encode(text).length<=limit&&!/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(text)?text:null;},
 current(snapshot,id){return snapshot.state==='active'&&snapshot.request_id===id;},
 escape(value){return String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));},
 chatMarkup(messages){return messages.map(message=>`<article class="chat-message"><span>${message.author==='host'?'Владелец экрана':'Участник'}</span><p>${SessionToolsPolicy.escape(message.text)}</p></article>`).join('');}
};
const sessionTools={request:null,polling:false,chat:[],chatOpen:false,chatSeen:0,clipboardText:'',clipboardEpoch:0,clipboardAllowed:null,error:'',statistics:{at:0,frames:0,bytes:0,fps:0,kbps:0,width:0,height:0,codec:'jpeg'}};
function resetSessionTools(request){Object.assign(sessionTools,{request,polling:false,chat:[],chatOpen:false,chatSeen:0,clipboardText:'',clipboardEpoch:0,clipboardAllowed:null,error:'',statistics:{at:0,frames:0,bytes:0,fps:0,kbps:0,width:0,height:0,codec:'jpeg'}});}
function sessionChatMarkup(){return `<aside id="session-chat" class="session-chat" aria-label="Чат сеанса" ${sessionTools.chatOpen?'':'hidden'}><header><strong>Чат</strong><button type="button" id="chat-close" aria-label="Закрыть чат">×</button></header><div id="chat-messages" role="log" aria-label="Сообщения" aria-live="polite">${SessionToolsPolicy.chatMarkup(sessionTools.chat)}</div><form id="chat-form"><label class="sr-only" for="chat-text">Сообщение</label><textarea id="chat-text" rows="2" maxlength="2048" placeholder="Сообщение участнику…"></textarea><div class="text-send-actions"><span id="chat-feedback" role="status"></span><button type="submit">Отправить</button></div></form></aside>`;}
function sessionClipboardMarkup(){const s=remoteState();return `<div class="tool-menu" id="menu-clipboard" hidden><strong>Буфер обмена</strong>${s.role==='host'?`<label class="compact-check"><input id="clipboard-enable" type="checkbox" ${s.clipboard_enabled?'checked':''}>Разрешить обмен текстом</label>`:'<p id="clipboard-permission">Обмен текстом разрешает владелец экрана.</p>'}<form id="clipboard-form"><label for="clipboard-outgoing">Текст для другого устройства</label><textarea id="clipboard-outgoing" rows="2" maxlength="4096" placeholder="Вставьте текст здесь"></textarea><div class="text-send-actions"><button type="button" id="clipboard-paste">Из моего буфера</button><button type="submit" id="clipboard-send">Передать</button></div></form><label for="clipboard-incoming">Получено от участника</label><textarea id="clipboard-incoming" rows="2" readonly placeholder="Пока ничего нет">${esc(sessionTools.clipboardText)}</textarea><button type="button" id="clipboard-copy" ${sessionTools.clipboardText?'':'disabled'}>Скопировать в мой буфер</button><p id="clipboard-feedback" role="status">Обмен только по кнопке. Полученный текст не вводится в приложения автоматически.</p></div>`;}
function clipboardAllowed(){const s=remoteState();return !!s.clipboard_enabled&&sessionTools.clipboardEpoch===(s.clipboard_epoch||0)&&sessionTools.clipboardAllowed!==false;}
function updateSessionTools(){
 const s=remoteState(),epoch=s.clipboard_epoch||0;
 if(epoch>sessionTools.clipboardEpoch||!s.clipboard_enabled&&epoch===sessionTools.clipboardEpoch){sessionTools.clipboardText='';sessionTools.clipboardEpoch=epoch;sessionTools.clipboardAllowed=!!s.clipboard_enabled;}
 const enabled=clipboardAllowed();
 const incoming=$('#clipboard-incoming');if(incoming)incoming.value=sessionTools.clipboardText;
 const copy=$('#clipboard-copy');if(copy)copy.disabled=!enabled||!sessionTools.clipboardText;
 const send=$('#clipboard-send');if(send)send.disabled=!enabled;
 const toggle=$('#clipboard-enable');if(toggle)toggle.checked=enabled;
 const hint=$('#clipboard-permission');if(hint)hint.textContent=enabled?'Обмен текстом разрешён.':'Обмен текстом разрешает владелец экрана.';
 const chat=$('#session-chat');if(chat)chat.hidden=!sessionTools.chatOpen;
 $('#session-workspace')?.classList.toggle('with-chat',sessionTools.chatOpen);
 const unseen=sessionTools.chat.filter(x=>x.sequence>sessionTools.chatSeen).length;
 const button=$('#session-chat-open');if(button)button.textContent=`Чат${unseen&&!sessionTools.chatOpen?' · '+unseen:''}`;
 const info=$('#session-statistics');if(info){const x=sessionTools.statistics;info.textContent=x.width?(s.role==='host'?`Локальный предпросмотр · ${x.width}×${x.height}`:`${x.codec.toUpperCase()} · ${x.width}×${x.height} · ${x.fps.toFixed(1)} кадр/с · ${Math.round(x.kbps)} КБ/с видео`):'Ждём данные видеопотока…';}
}
async function pollSessionTools(){
 const s=remoteState(),id=s.request_id;if(s.state!=='active'||sessionTools.polling)return;
 if(sessionTools.request!==id)resetSessionTools(id);sessionTools.polling=true;
 try{
  const [chat,clipboard]=await Promise.all([invoke('session_chat',{requestId:id}),s.clipboard_enabled?invoke('session_clipboard_read',{requestId:id}):Promise.resolve(null)]);
  if(!SessionToolsPolicy.current(remoteState(),id)||sessionTools.request!==id)return;
  const messages=Array.isArray(chat?.messages)?chat.messages.slice(-100):[];
  if(JSON.stringify(messages)!==JSON.stringify(sessionTools.chat)){sessionTools.chat=messages;const log=$('#chat-messages');if(log){const atEnd=log.scrollHeight-log.scrollTop-log.clientHeight<40;log.innerHTML=SessionToolsPolicy.chatMarkup(messages);if(atEnd)log.scrollTop=log.scrollHeight;}if(sessionTools.chatOpen)sessionTools.chatSeen=messages.at(-1)?.sequence||0;}
  if(clipboard&&clipboard.request_id===id&&clipboard.epoch>=sessionTools.clipboardEpoch){if(clipboard.epoch!==sessionTools.clipboardEpoch||!clipboard.enabled)sessionTools.clipboardText='';sessionTools.clipboardEpoch=clipboard.epoch;sessionTools.clipboardAllowed=clipboard.enabled;if(clipboard.enabled&&typeof clipboard.text==='string')sessionTools.clipboardText=clipboard.text;}
  updateSessionTools();
 }catch(error){if(SessionToolsPolicy.current(remoteState(),id)){sessionTools.error=String(error);const hint=$('#chat-feedback');if(hint)hint.textContent='Не удалось обновить сообщения.';}}
 finally{if(sessionTools.request===id)sessionTools.polling=false;}
}
function noteVideoFrame(frame){const x=sessionTools.statistics,now=performance.now();if(!x.at)x.at=now;x.frames++;x.bytes+=frame.byte_length||frame.bytes?.length||0;x.width=frame.width;x.height=frame.height;x.codec=frame.codec;const elapsed=now-x.at;if(elapsed>=1000){x.fps=x.frames*1000/elapsed;x.kbps=x.bytes/elapsed;x.frames=0;x.bytes=0;x.at=now;}updateSessionTools();}
function bindSharedTools(on){
 const openChat=()=>{closeToolMenus();releaseRemoteInput();sessionTools.chatOpen=true;sessionTools.chatSeen=sessionTools.chat.at(-1)?.sequence||0;updateSessionTools();layoutRemoteStage();$('#chat-text')?.focus();};
 on($('#session-chat-open'),'click',openChat);
 on($('#chat-close'),'click',()=>{sessionTools.chatOpen=false;updateSessionTools();layoutRemoteStage();$('#remote-screen')?.focus();});
 on($('#session-clipboard-open'),'click',()=>{closeToolMenus();releaseRemoteInput();$('#menu-clipboard').hidden=false;positionToolMenu($('#menu-clipboard'));updateSessionTools();$('#clipboard-outgoing')?.focus();});
 on($('#chat-form'),'submit',async event=>{event.preventDefault();const input=$('#chat-text'),text=SessionToolsPolicy.text(input.value,2048),id=remoteState().request_id,button=event.target.querySelector('[type=submit]');if(!text){$('#chat-feedback').textContent='Введите сообщение до 2048 байт.';return;}button.disabled=true;try{await invoke('session_chat_send',{requestId:id,text});if(SessionToolsPolicy.current(remoteState(),id)&&$('#chat-text')===input){input.value='';$('#chat-feedback').textContent='';await pollSessionTools();}}catch(error){if($('#chat-text')===input)$('#chat-feedback').textContent=String(error);}finally{button.disabled=false;}});
 on($('#clipboard-enable'),'change',async event=>{const input=event.target,id=remoteState().request_id;input.disabled=true;try{await invoke('session_clipboard_enable',{requestId:id,enabled:input.checked});await refreshSession();updateSessionTools();}catch(error){input.checked=!input.checked;toast(error);}finally{input.disabled=false;}});
 on($('#clipboard-paste'),'click',async()=>{const id=remoteState().request_id,input=$('#clipboard-outgoing');try{const text=await navigator.clipboard.readText();if(SessionToolsPolicy.current(remoteState(),id)&&input===$('#clipboard-outgoing')){if(!SessionToolsPolicy.text(text,4096)){toast('Буфер должен содержать текст до 4096 байт.');return;}input.value=text;input.focus();}}catch{toast('Вставьте текст в поле сочетанием Ctrl/Cmd + V.');}});
 on($('#clipboard-form'),'submit',async event=>{event.preventDefault();const id=remoteState().request_id,input=$('#clipboard-outgoing'),text=SessionToolsPolicy.text(input.value,4096),button=$('#clipboard-send');if(!clipboardAllowed()||!text){toast('Нужно разрешение владельца и текст до 4096 байт.');return;}button.disabled=true;try{await invoke('session_clipboard_send',{requestId:id,text});if(SessionToolsPolicy.current(remoteState(),id)&&input===$('#clipboard-outgoing'))$('#clipboard-feedback').textContent='Текст передан. Участник сможет скопировать его у себя.';}catch(error){toast(error);}finally{button.disabled=!clipboardAllowed();}});
 on($('#clipboard-copy'),'click',async()=>{const id=remoteState().request_id,text=sessionTools.clipboardText;if(!clipboardAllowed()||!text)return;try{await navigator.clipboard.writeText(text);if(SessionToolsPolicy.current(remoteState(),id)&&$('#clipboard-feedback'))$('#clipboard-feedback').textContent='Скопировано в ваш буфер.';}catch{const field=$('#clipboard-incoming');field?.focus();field?.select();toast('Скопируйте выделенный текст сочетанием Ctrl/Cmd + C.');}});
 updateSessionTools();
}
if(typeof module!=='undefined')module.exports={SessionToolsPolicy};

// Text transcript deliberately does not interpret remote terminal escape commands.
const SessionTerminalPolicy={
 strip(text,state){let output='';for(const char of text){
  if(state.mode==='escape'){state.mode=char==='['?'csi':char===']'?'osc':'text';continue;}
  if(state.mode==='csi'){if(char>='@'&&char<='~')state.mode='text';continue;}
  if(state.mode==='osc'){if(char==='\x07')state.mode='text';else if(char==='\x1b')state.mode='osc_escape';continue;}
  if(state.mode==='osc_escape'){state.mode=char==='\\'?'text':'osc';continue;}
  if(char==='\x1b'){state.mode='escape';continue;}
  if(char==='\n'||char==='\t'||char>=' '&&char!=='\x7f')output+=char;
 }return output;}
};
const sessionTerminal={request:null,polling:false,open:false,requested:false,enabled:false,epoch:0,text:'',decoder:new TextDecoder(),ansi:{mode:'text'},error:'',announced:false};
function resetSessionTerminal(request){Object.assign(sessionTerminal,{request,polling:false,open:false,requested:false,enabled:false,epoch:0,text:'',decoder:new TextDecoder(),ansi:{mode:'text'},error:'',announced:false});}
function terminalMarkup(){const s=remoteState(),host=s.role==='host',t=sessionTerminal;return `<header class="terminal-header"><strong>Терминал · текстовый вывод</strong><button type="button" id="terminal-hide" aria-label="Скрыть терминал">×</button></header><p>Команды выполняются с правами владельца устройства. Полноэкранные консольные приложения здесь не поддерживаются.</p>${host?`<p>${t.requested?'Участник запросил доступ к командной строке. Разрешайте только доверенному человеку.':'Доступ к командной строке выключен, пока вы его не разрешите.'}</p><button type="button" id="terminal-allow">${t.enabled?'Отозвать доступ':'Разрешить терминал'}</button>`:`<button type="button" id="terminal-request" ${t.requested||t.enabled?'disabled':''}>${t.enabled?'Терминал подключён':t.requested?'Ожидаем разрешение владельца':'Запросить терминал'}</button>`}<label class="sr-only" for="terminal-output">Вывод команд</label><textarea id="terminal-output" class="terminal-output" readonly spellcheck="false">${esc(t.text)}</textarea>${!host?`<form id="terminal-form" class="terminal-command"><label class="sr-only" for="terminal-command">Команда</label><input id="terminal-command" maxlength="2048" autocomplete="off" spellcheck="false" placeholder="Введите команду" ${t.enabled?'':'disabled'}><button type="submit" ${t.enabled?'':'disabled'}>Выполнить</button></form><div class="terminal-actions"><button type="button" id="terminal-interrupt" ${t.enabled?'':'disabled'}>Ctrl+C</button><button type="button" id="terminal-close">Завершить терминал</button></div>`:''}<p id="terminal-feedback" role="status">${esc(t.error)}</p>`;}
function renderTerminal(){const panel=$('#terminal-panel');if(!panel)return;panel.hidden=!sessionTerminal.open;if(!sessionTerminal.open)return;panel.innerHTML=terminalMarkup();bindTerminalPanel();}
async function pollTerminal(){const s=remoteState(),id=s.request_id;if(s.state!=='active'||sessionTerminal.polling)return;if(sessionTerminal.request!==id)resetSessionTerminal(id);sessionTerminal.polling=true;
 try{const next=await invoke('session_terminal_read',{requestId:id});if(!SessionToolsPolicy.current(remoteState(),id)||sessionTerminal.request!==id||next?.request_id!==id)return;
  const changed=next.enabled!==sessionTerminal.enabled||next.requested!==sessionTerminal.requested||next.epoch!==sessionTerminal.epoch;
  if(next.epoch!==sessionTerminal.epoch){sessionTerminal.decoder=new TextDecoder();sessionTerminal.ansi={mode:'text'};sessionTerminal.text='';}
  Object.assign(sessionTerminal,{enabled:next.enabled,requested:next.requested,epoch:next.epoch});
  if(next.output?.length){const decoded=sessionTerminal.decoder.decode(new Uint8Array(next.output),{stream:true});sessionTerminal.text=(sessionTerminal.text+SessionTerminalPolicy.strip(decoded,sessionTerminal.ansi)).slice(-65536);}
  if(changed)renderTerminal();else{const output=$('#terminal-output');if(output&&sessionTerminal.open){const atEnd=output.scrollHeight-output.scrollTop-output.clientHeight<40;output.value=sessionTerminal.text;if(atEnd)output.scrollTop=output.scrollHeight;}}
  const button=$('#session-terminal-open');if(button)button.textContent=next.requested&&!next.enabled?'Терминал · запрос':'Терминал';
  if(s.role==='host'&&next.requested&&!next.enabled&&!sessionTerminal.announced){sessionTerminal.announced=true;toast('Участник запросил терминал. Разрешение находится в меню сеанса.');}
  if(!next.requested)sessionTerminal.announced=false;
 }catch(error){if(SessionToolsPolicy.current(remoteState(),id)){sessionTerminal.error=String(error);const feedback=$('#terminal-feedback');if(feedback)feedback.textContent=String(error);}}
 finally{if(sessionTerminal.request===id)sessionTerminal.polling=false;}
}
async function terminalAction(command,args={}){const id=remoteState().request_id;try{await invoke(command,{requestId:id,...args});if(SessionToolsPolicy.current(remoteState(),id)){sessionTerminal.error='';await pollTerminal();}}catch(error){if(SessionToolsPolicy.current(remoteState(),id)){sessionTerminal.error=String(error);const feedback=$('#terminal-feedback');if(feedback)feedback.textContent=String(error);}}}
function bindTerminalPanel(){
 const on=(selector,event,fn)=>$(selector)?.addEventListener(event,fn,{signal:sessionView.abort.signal});
 on('#terminal-hide','click',()=>{sessionTerminal.open=false;renderTerminal();$('#remote-screen')?.focus();});
 on('#terminal-request','click',()=>terminalAction('session_terminal_request'));
 on('#terminal-allow','click',()=>terminalAction('session_terminal_allow',{enabled:!sessionTerminal.enabled}));
 on('#terminal-close','click',()=>terminalAction('session_terminal_close'));
 on('#terminal-interrupt','click',()=>terminalAction('session_terminal_input',{bytes:[3]}));
 on('#terminal-form','submit',async event=>{event.preventDefault();const input=$('#terminal-command'),text=SessionToolsPolicy.text(input.value,4095),id=remoteState().request_id;if(!text||!sessionTerminal.enabled)return;const bytes=Array.from(new TextEncoder().encode(text+'\r'));await terminalAction('session_terminal_input',{bytes});if(SessionToolsPolicy.current(remoteState(),id)&&input===$('#terminal-command')&&!sessionTerminal.error)input.value='';});
}
function bindTerminalTools(on){on($('#session-terminal-open'),'click',()=>{closeToolMenus();releaseRemoteInput();sessionTerminal.open=true;renderTerminal();$('#terminal-panel')?.scrollIntoView({block:'nearest'});pollTerminal();});if(sessionTerminal.open)renderTerminal();}

const sessionSystem={request:null,polling:false,supported:{lock:false,restart:false},pending:null,latest:null};
function resetSessionSystem(request){Object.assign(sessionSystem,{request,polling:false,supported:{lock:false,restart:false},pending:null,latest:null});}
function updateSystemTools(){const s=remoteState(),host=s.role==='host';for(const action of ['lock','restart']){const button=$('#system-'+action);if(button){button.hidden=host||!sessionSystem.supported[action];button.disabled=!!sessionSystem.pending;}}
 const panel=$('#system-consent');if(!panel)return;const pending=sessionSystem.pending;panel.hidden=!host||!pending;if(host&&pending&&panel.dataset.actionId!==pending.id){panel.dataset.actionId=pending.id;const restart=pending.action==='restart';panel.innerHTML=`<strong>${restart?'Участник просит перезагрузить компьютер':'Участник просит заблокировать экран'}</strong><p>${restart?'Несохранённая работа может быть потеряна. Текущий сеанс прервётся.':'Экран будет заблокирован. Текущий сеанс может прерваться.'}</p><div class="terminal-actions"><button type="button" data-system-decision="reject" data-action-id="${esc(pending.id)}">Отклонить</button><button type="button" data-system-decision="approve" data-action-id="${esc(pending.id)}">${restart?'Перезагрузить':'Заблокировать'}</button></div>`;}
 if(!pending)panel.dataset.actionId='';
 const feedback=$('#system-feedback');if(feedback){const labels={requested:'Ожидаем подтверждение владельца.',approved:'Запрос одобрен. Сеанс может прерваться.',rejected:'Владелец отклонил запрос.',dispatched:'Команда передана системе.',failed:'Не удалось выполнить системное действие.'};feedback.textContent=sessionSystem.latest?labels[sessionSystem.latest.state]||'':'';}
}
async function pollSystemTools(){const s=remoteState(),id=s.request_id;if(s.state!=='active'||sessionSystem.polling)return;if(sessionSystem.request!==id)resetSessionSystem(id);sessionSystem.polling=true;try{const next=await invoke('session_system_snapshot',{requestId:id});if(!SessionToolsPolicy.current(remoteState(),id)||next?.request_id!==id)return;Object.assign(sessionSystem,{supported:next.supported,pending:next.pending,latest:next.latest});updateSystemTools();}catch{}finally{if(sessionSystem.request===id)sessionSystem.polling=false;}}
function bindSystemTools(on){
 for(const action of ['lock','restart'])on($('#system-'+action),'click',async event=>{const button=event.target,id=remoteState().request_id;button.disabled=true;try{await invoke('session_system_request',{requestId:id,action});await pollSystemTools();}catch(error){toast(error);}finally{button.disabled=!!sessionSystem.pending;}});
 on($('#system-consent'),'click',async event=>{const button=event.target.closest('[data-system-decision]');if(!button)return;const id=remoteState().request_id,actionId=button.dataset.actionId;button.disabled=true;try{await invoke('session_system_decide',{requestId:id,id:actionId,approved:button.dataset.systemDecision==='approve'});await pollSystemTools();}catch(error){toast(error);}finally{button.disabled=false;}});
 updateSystemTools();
}
