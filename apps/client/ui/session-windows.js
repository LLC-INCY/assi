'use strict';
// The registry describes actual native windows. Each window has its own Remote engine.
const sessionWindows={current:'',viewerOnly:false,supported:false,windows:[],polling:false,opening:false,initialized:false,revision:0};
function applyWindowRegistry(snapshot){
 if(!snapshot||typeof snapshot.current!=='string'||!Array.isArray(snapshot.windows))throw new Error('Не удалось прочитать список окон сеансов');
 const windows=snapshot.windows.filter(item=>typeof item.label==='string').slice(0,5);
 Object.assign(sessionWindows,{current:snapshot.current,viewerOnly:!!snapshot.viewer_only,supported:!!snapshot.supported,windows});
 if(typeof state!=='undefined')state.viewerOnly=sessionWindows.viewerOnly;
 if(typeof document!=='undefined')document.body.classList.toggle('viewer-only',sessionWindows.viewerOnly);
}
function sessionWindowMarkup(){
 if(!sessionWindows.supported)return '';
 const labels={idle:'Новое подключение',connecting:'Подключение…',listening:'Приём включён',pending:'Ожидание',active:'Сеанс',closed:'Завершён',error:'Ошибка'};
 return `<nav class="session-window-switcher" aria-label="Окна подключений">${sessionWindows.windows.map(item=>`<div class="session-window-chip${item.current?' current':''}"><button type="button" data-window-focus="${esc(item.label)}" ${item.current?'aria-current="true"':''} title="${esc(labels[item.state]||'Сеанс')}"><span class="dot ${item.state==='active'?'':'unknown'}" aria-hidden="true"></span><span>${esc(item.peer_name||(item.label==='main'?'Главное окно':'Новое подключение'))}</span></button>${item.label==='main'?'':`<button type="button" class="window-close" data-window-close="${esc(item.label)}" aria-label="Завершить и закрыть ${esc(item.peer_name||'окно подключения')}" title="Завершить сеанс и закрыть окно">×</button>`}</div>`).join('')}<button type="button" id="session-window-new" class="session-window-new" ${sessionWindows.opening||sessionWindows.windows.length>=5?'disabled':''} title="${sessionWindows.windows.length>=5?'Открыто четыре дополнительных окна':'Открыть отдельное окно подключения'}">＋ Новое окно</button></nav>`;
}
function updateSessionWindows(){const target=$('#session-windows');if(!target)return;const focused=document.activeElement?.closest('[data-window-focus],[data-window-close]');const key=focused?.hasAttribute('data-window-close')?'data-window-close':'data-window-focus',label=focused?.getAttribute(key);const markup=sessionWindowMarkup();if(target.innerHTML===markup)return;target.innerHTML=markup;if(label){const replacement=[...target.querySelectorAll(`[${key}]`)].find(button=>button.getAttribute(key)===label);replacement?.focus({preventScroll:true});}}
async function pollSessionWindows(){if(sessionWindows.polling)return;sessionWindows.polling=true;const revision=++sessionWindows.revision;
 try{const snapshot=await invoke('session_windows');if(revision!==sessionWindows.revision)return;applyWindowRegistry(snapshot);updateSessionWindows();}catch{}finally{if(revision===sessionWindows.revision)sessionWindows.polling=false;}
}
async function initializeSessionWindows(){
 const snapshot=await invoke('session_windows');applyWindowRegistry(snapshot);
 if(sessionWindows.initialized)return sessionWindows.viewerOnly;sessionWindows.initialized=true;
 document.addEventListener('click',async event=>{const button=event.target.closest('[data-window-focus],[data-window-close],#session-window-new');if(!button||button.disabled)return;const label=button.dataset.windowFocus||button.dataset.windowClose;
  button.disabled=true;
  try{if(button.id==='session-window-new'){if(sessionWindows.opening)return;sessionWindows.opening=true;await invoke('session_window_open');}else if(button.dataset.windowClose){await invoke('session_window_close',{label});}else await invoke('session_window_focus',{label});await pollSessionWindows();}
  catch(error){toast(error);}finally{sessionWindows.opening=false;if(button.isConnected)button.disabled=false;updateSessionWindows();}
 });
 return sessionWindows.viewerOnly;
}
