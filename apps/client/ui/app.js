'use strict';
const $ = selector => document.querySelector(selector);
const esc = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const state = { page:'devices', selected:'local', query:'', group:'all', view:'detail', tab:'active', data:null, platform:'', version:'', themeBusy:false, nameDraft:null,viewerOnly:false };
let toastTimer, lastFocus;
const dialogTasks = createDialogTasks();
const invoke = (command,args={}) => {
  if (!window.__TAURI__?.core?.invoke) return Promise.reject('Откройте установленное приложение Асси. Эта страница работает с Rust-ядром и не запускается отдельно в браузере.');
  return window.__TAURI__.core.invoke(command,args);
};
function toast(message){ $('#toast').textContent=String(message);$('#toast').hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('#toast').hidden=true,4500); }
function osKind(os){return os==='Android'?'phone':os==='macOS'?'laptop':os==='Linux'?'server':'monitor';}
function devices(){if(!state.data)return [];return [{id:'local',name:state.data.settings.device_name,os:state.platform,group:'personal',code:'',local:true},...state.data.devices].map(d=>({...d,kind:osKind(d.os),art:d.os==='macOS'}));}
function matches(d){const q=state.query.trim().toLocaleLowerCase('ru');return (!q||`${d.name} ${d.os}`.toLocaleLowerCase('ru').includes(q)||d.code.includes(q.replace(/ /g,'')))&&(state.group==='all'||state.group===d.group);}
const formatCode = value => String(value).replace(/(\d{3})(?=\d)/g,'$1 ');
function status(d,pill=false){return `<span class="status${pill?' pill':''}"><span class="dot ${d.local?'local':'unknown'}"></span>${d.local?'Это устройство':'Статус неизвестен'}</span>`;}
function selectButton(d){return `<button class="connect-arrow" data-select="${esc(d.id)}" aria-label="Открыть устройство: ${esc(d.name)}">${icon('arrow')}</button>`;}
function filters(){return `<div class="group-filter" aria-label="Группы устройств">${[['all','Все'],['personal','Личные'],['work','Рабочие']].map(([value,label])=>`<button data-group="${value}" aria-pressed="${state.group===value}">${label}</button>`).join('')}</div>`;}
function emptyResults(){return '<div class="empty"><h3>Ничего не найдено</h3><p>Измените запрос или добавьте устройство по его коду.</p><button class="button" data-action="reset">Сбросить поиск</button></div>';}
function mobileRow(d){return `<article class="mobile-device${d.id===state.selected?' selected':''}"><span class="device-picture">${d.art?'<img src="assets/laptop.png" alt="" width="62" height="47">':icon(d.kind)}</span><button class="select-device" data-select="${esc(d.id)}"><span class="row-caption"><strong>${esc(d.name)}</strong><small>${esc(d.os)}</small>${status(d)}</span></button>${selectButton(d)}</article>`;}
function renderSidebar(){
  $('#sidebar-devices').innerHTML=filters()+['personal','work'].map(group=>{
    const list=devices().filter(d=>d.group===group&&matches(d));
    return `<section class="device-group"><h2>${group==='personal'?'Личные':'Рабочие'}</h2>${list.length?list.map(d=>`<button class="side-device" data-select="${esc(d.id)}" aria-pressed="${d.id===state.selected}">${icon(d.kind)}<span class="device-name">${esc(d.name)}</span><span class="dot ${d.local?'local':'offline'}" aria-hidden="true"></span><span class="sr-only">${d.local?'Это устройство':'Статус неизвестен'}</span></button>`).join(''):'<p class="side-caption">Нет устройств</p>'}</section>`;
  }).join('');
}
function renderDevices(){
  const list=devices().filter(matches), d=devices().find(x=>x.id===state.selected)||devices()[0];
  const catalogue=`<section class="desktop-catalogue"><div class="catalogue-header"><h2>${state.query?'Результаты поиска':state.group==='work'?'Рабочие устройства':state.group==='personal'?'Личные устройства':'Все устройства'}</h2><small class="muted">${list.length}</small></div>${list.length?list.map(d=>`<div class="catalogue-row">${icon(d.kind)}<button class="select-device" data-select="${esc(d.id)}"><span class="row-caption"><strong>${esc(d.name)}</strong><small>${esc(d.os)} · ${d.group==='work'?'Рабочие':'Личные'}</small></span></button>${status(d)}${selectButton(d)}</div>`).join(''):emptyResults()}</section>`;
  const detail=`<section class="desktop-detail"><div class="detail-title"><div><h2>${esc(d.name)}</h2><p>${esc(d.os)} · ${d.group==='work'?'Рабочие':'Личные'}</p></div>${status(d,true)}</div><div class="device-art${d.art?'':' generic'}">${d.art?'<img src="assets/laptop.png" alt="Ноутбук с бирюзовыми обоями" width="1536" height="1024">':icon(d.kind)}</div><div class="device-actions"><button class="button primary" data-action="connection">${icon('monitor')}${d.local?'Предоставить доступ':'Подключиться'}</button><button class="button" data-action="files">${icon('folder')}Передать файлы</button></div><p class="info-note local-note">${d.local?'Включите приём и передайте приглашение. Каждый запрос требует вашего подтверждения.':'Устройство сохранено в адресной книге. Для прямого подключения получите свежее приглашение у владельца.'}</p><section class="recent"><h3>${d.local?'Сохранённые устройства':'Об устройстве'}</h3>${d.local?(state.data.devices.length?state.data.devices.slice(0,3).map(x=>`<div class="recent-row">${icon(osKind(x.os))}<div class="row-caption"><strong>${esc(x.name)}</strong><small>${esc(x.os)}</small></div>${selectButton(x)}</div>`).join(''):'<p class="info-note">Добавьте устройство по коду — оно останется в списке после перезапуска.</p>'):`<p class="info-note">Код ${esc(formatCode(d.code))}</p><div class="device-management"><button class="text-button danger" data-remove="${esc(d.id)}">Убрать из списка</button></div>`}</section></section>`;
  $('#main').innerHTML=(state.query||state.view==='list'?catalogue:detail)+`<section class="mobile-devices">${filters()}${list.length?list.map(mobileRow).join(''):emptyResults()}<button class="button mobile-add" data-action="add">${icon('plus')}Добавить устройство</button></section>`;
  $('#result-status').textContent=state.query?`Найдено устройств: ${list.length}`:'';
}
function renderSessions(){renderLiveSessions();}
function renderSettings(){
  $('#main').innerHTML=`<div class="page-heading"><h2>Настройки</h2><p>Ваше устройство и оформление приложения.</p></div><section class="settings-section"><h3>Это устройство</h3><form id="settings-form" class="setting-form"><label class="field">Название устройства<input name="device_name" maxlength="60" required aria-describedby="settings-error" value="${esc(state.nameDraft??state.data.settings.device_name)}"></label><p id="settings-error" class="form-error" role="alert"></p><div class="setting-actions"><button class="button" type="submit">Сохранить название</button><span id="settings-feedback" role="status"></span></div></form></section><section class="settings-section"><h3>Внешний вид</h3><div class="setting-row"><div><strong>Тема приложения</strong><p>Сохраняется на этом устройстве.</p></div><select id="theme-select" aria-label="Тема приложения"><option value="dark" ${state.data.settings.theme==='dark'?'selected':''}>Тёмная</option><option value="light" ${state.data.settings.theme==='light'?'selected':''}>Светлая</option></select></div></section><section class="settings-section" id="access-settings"><h3>Постоянный доступ</h3><p class="field-hint">Загрузка…</p></section><section class="settings-section"><h3>О приложении</h3><div class="setting-row"><div><strong>Асси ${esc(state.version)}</strong><p>${esc(state.platform)} · ранняя сборка</p></div></div><p class="field-hint">Устройства и настройки сохраняются локально. Подключения работают напрямую или через свой сервер. Передача файлов требует отдельного согласия получателя.</p><p class="field-hint">Локальный идентификатор установки</p><code>${esc(state.data.installation_id)}</code></section>`;
  loadAccessSettings();
  $('#theme-select').onchange=e=>changeTheme(e.target.value);
  $('#settings-form').elements.device_name.oninput=e=>{state.nameDraft=e.target.value;e.target.removeAttribute('aria-invalid');$('#settings-error').textContent='';};
  $('#settings-form').onsubmit=async e=>{
    e.preventDefault();if(state.themeBusy)return;const form=e.target;const field=form.elements.device_name;if(!field.value.trim()){field.setAttribute('aria-invalid','true');$('#settings-error').textContent='Введите название устройства';field.focus();return;}state.themeBusy=true;const button=form.querySelector('button');button.disabled=true;$('#theme-toggle').disabled=true;$('#theme-select').disabled=true;
    try{state.data.settings=await invoke('save_settings',{settings:{...state.data.settings,device_name:new FormData(form).get('device_name')}});state.nameDraft=null;const feedback=$('#settings-feedback');if(feedback)feedback.textContent='Сохранено';renderSidebar();}
    catch(error){const target=form.querySelector('#settings-error');target.textContent=String(error);field.setAttribute('aria-invalid','true');field.focus();}finally{state.themeBusy=false;button.disabled=false;$('#theme-toggle').disabled=false;if($('#theme-select'))$('#theme-select').disabled=false;}
  };
}
function render(){
 if(typeof sessionViewCleanup==='function')sessionViewCleanup();
  if(!state.data)return;
  const active=document.activeElement;const key=['data-select','data-group','data-tab'].find(k=>active?.hasAttribute(k));const value=key?active.getAttribute(key):null;
  document.querySelectorAll('[data-page]').forEach(b=>{if(b.dataset.page===state.page)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current');});
  $('#detail-view').setAttribute('aria-pressed',String(state.view==='detail'));$('#list-view').setAttribute('aria-pressed',String(state.view==='list'));
  renderSidebar();if(state.page==='devices')renderDevices();else if(state.page==='sessions')renderSessions();else renderSettings();
  if(key){const el=[...document.querySelectorAll(`[${key}="${CSS.escape(value)}"]`)].find(x=>x.getClientRects().length);(el||$('#main')).focus({preventScroll:true});}
}
function setPage(page){if(state.viewerOnly&&page!=='sessions')page='sessions';state.page=page;state.query='';$('#search').value='';render();$('#main').focus({preventScroll:true});}
function applyTheme(){document.body.classList.toggle('light',state.data.settings.theme==='light');$('#theme-toggle').setAttribute('aria-label',state.data.settings.theme==='light'?'Включить тёмную тему':'Включить светлую тему');}
async function changeTheme(theme){
  if(state.themeBusy){if($('#theme-select'))$('#theme-select').value=state.data.settings.theme;return;}state.themeBusy=true;$('#theme-toggle').disabled=true;
  try{state.data.settings=await invoke('save_settings',{settings:{...state.data.settings,theme}});applyTheme();}
  catch(error){toast(error);}finally{state.themeBusy=false;$('#theme-toggle').disabled=false;if($('#theme-select'))$('#theme-select').value=state.data.settings.theme;}
}
function modal(title,content){dialogTasks.advance();lastFocus=document.activeElement;$('#dialog-title').textContent=title;$('#dialog-content').innerHTML=content;$('#dialog').showModal();hydrate($('#dialog'));}
function closeModal(){dialogTasks.advance();$('#dialog').close();if(lastFocus?.isConnected)lastFocus.focus();else $('#main').focus();}
function connectionInfo(){if(state.selected==='local')openHosting();else openConnection();}
function refreshDeviceBook(){renderSidebar();if(state.page==='devices')renderDevices();}
function addDevice(){
  modal('Добавить устройство',`<p class="dialog-subtitle">Сохраните устройство по его коду. Для прямого подключения используйте приглашение во вкладке «Сеансы».</p><form id="add-form" novalidate><label class="field">Название<input name="name" aria-describedby="name-error" maxlength="60" placeholder="Например, рабочий ноутбук" required><span id="name-error" class="form-error"></span></label><label class="field">Код устройства<input name="code" aria-describedby="code-error" inputmode="numeric" maxlength="11" placeholder="000 000 000" required><span id="code-error" class="form-error"></span></label><label class="field">Система<select name="os"><option>Windows</option><option>macOS</option><option>Linux</option><option>Android</option></select></label><label class="field">Группа<select name="group"><option value="personal">Личные</option><option value="work">Рабочие</option></select></label><p id="add-error" class="form-error" role="alert"></p><div class="dialog-actions"><button class="button" type="button" data-action="close">Отмена</button><button class="button primary" type="submit">Сохранить устройство</button></div></form>`);
  const form=$('#add-form');
  form.onsubmit=async e=>{
    e.preventDefault();let invalid=null;for(const [name,valid,message] of [['name',!!form.elements.name.value.trim(),'Введите название устройства'],['code',/^\d{9}$/.test(form.elements.code.value.replace(/ /g,'')),'Введите код из 9 цифр']]){const field=form.elements[name];field.setAttribute('aria-invalid',String(!valid));form.querySelector('#'+name+'-error').textContent=valid?'':message;if(!valid&&!invalid)invalid=field;}if(invalid){invalid.focus();return;}const button=form.querySelector('[type=submit]');const current=dialogTasks.capture();button.disabled=true;$('#add-error').textContent='';
    try{const d=await invoke('add_device',{input:Object.fromEntries(new FormData(form))});state.data.devices.push(d);if(current()){state.selected=d.id;state.view='detail';closeModal();setPage('devices');}else refreshDeviceBook();toast('Устройство сохранено');}
    catch(error){const target=current()?form.querySelector('#add-error'):null;if(target)target.textContent=String(error);else toast(error);}finally{button.disabled=false;}
  };
}
function confirmRemove(id){const d=state.data.devices.find(x=>x.id===id);if(!d)return;
  modal('Убрать устройство?',`<p class="dialog-subtitle">${esc(d.name)} будет удалено из вашего списка. Его можно снова добавить по коду ${esc(formatCode(d.code))}.</p><p id="remove-error" class="form-error" role="alert"></p><div class="dialog-actions"><button class="button" data-action="close">Отмена</button><button class="button danger" id="confirm-remove">Убрать из списка</button></div>`);
  $('#confirm-remove').onclick=async e=>{const current=dialogTasks.capture();const button=e.target;button.disabled=true;try{await invoke('remove_device',{id});state.data.devices=state.data.devices.filter(x=>x.id!==id);if(state.selected===id)state.selected='local';if(current())closeModal();refreshDeviceBook();toast('Устройство убрано из списка');}catch(error){const target=current()?$('#remove-error'):null;if(target)target.textContent=String(error);else toast(error);}finally{button.disabled=false;}};
}
function mobileDetails(d){modal(d.name,`<p class="dialog-subtitle">${esc(d.os)} · ${d.local?'Это устройство':`Код ${esc(formatCode(d.code))}`}</p><p class="dialog-subtitle">Для подключения используется приглашение от владельца устройства.</p><div class="dialog-actions">${d.local?'':`<button class="button danger" data-remove="${esc(d.id)}">Убрать из списка</button>`}<button class="button primary" data-session-action="${d.local?'host':'connect'}">${d.local?'Предоставить доступ':'Подключиться'}</button><button class="button" data-action="close">Закрыть</button></div>`);}
$('#close-dialog').onclick=closeModal;
$('#dialog').addEventListener('cancel',()=>{dialogTasks.advance();if(lastFocus?.isConnected)lastFocus.focus();});
$('#search').oninput=e=>{state.query=e.target.value;state.page='devices';render();};
$('#search-form').onsubmit=e=>{e.preventDefault();if(/^\d{9}$/.test(state.query.replace(/ /g,'')))connectionInfo();else {const list=devices().filter(matches);if(list.length===1){state.selected=list[0].id;state.view='detail';setPage('devices');}}};
$('#theme-toggle').onclick=()=>changeTheme(state.data.settings.theme==='light'?'dark':'light');
$('#detail-view').onclick=()=>{state.view='detail';setPage('devices');};$('#list-view').onclick=()=>{state.view='list';setPage('devices');};$('#add-device').onclick=addDevice;
$('.brand').onclick=e=>{e.preventDefault();state.selected='local';state.group='all';setPage('devices');};
document.addEventListener('keydown',e=>{
  if(e.target.closest('#remote-screen'))return;
  if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='k'&&!$('#dialog').open){e.preventDefault();$('#search').focus();$('#search').select();}
  if(e.target.matches('[data-tab]')&&['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){e.preventDefault();state.tab=e.key==='Home'?'active':e.key==='End'?'history':state.tab==='active'?'history':'active';render();document.querySelector(`[data-tab="${state.tab}"]`).focus();}
});
document.addEventListener('click',e=>{
  const b=e.target.closest('button');if(!b||b.disabled||!state.data)return;
  if(b.dataset.page){setPage(b.dataset.page);if(b.dataset.focusTransfer==='true'){const panel=$('#transfer-panel');panel?.scrollIntoView({block:'start'});panel?.focus({preventScroll:true});}return;}
  if(b.dataset.select){state.selected=b.dataset.select;state.view='detail';const mobile=document.querySelector('.mobile-devices')?.getClientRects().length;setPage('devices');if(mobile)mobileDetails(devices().find(d=>d.id===state.selected));return;}
  if(b.dataset.remove){if($('#dialog').open)closeModal();confirmRemove(b.dataset.remove);return;}
  if(b.dataset.group){state.group=b.dataset.group;state.view='list';render();return;}
  if(b.dataset.tab){state.tab=b.dataset.tab;render();return;}
  switch(b.dataset.action){case 'files':state.tab='active';setPage('sessions');if(remoteState().state!=='active')toast('Сначала установите соединение с устройством');break;case 'connection':connectionInfo();break;case 'access':openHosting();break;case 'stop-session':sessionAction('stop',b);break;case 'add':addDevice();break;case 'close':closeModal();break;case 'reset':state.group='all';setPage('devices');break;}
});
async function boot(){
  document.querySelectorAll('#frame button,#frame input').forEach(x=>x.disabled=true);
  try{const result=await invoke('load_snapshot');state.data=result.data;state.platform=result.platform;state.version=result.version;const viewerOnly=await initializeSessionWindows();if(viewerOnly)state.page='sessions';applyTheme();render();document.querySelectorAll('.app-header button,.app-header input,.sidebar>button,.view-switch button,.desktop-nav button,.mobile-nav button,.access-bar button').forEach(x=>x.disabled=false);initSessionUI();if(viewerOnly)openConnection();}
  catch(error){$('#main').innerHTML=`<div class="empty boot-error"><h2>Не удалось открыть Асси</h2><p>${esc(error)}</p><button id="retry" class="button">Повторить</button></div>`;$('#retry').onclick=boot;}
  finally{$('#main').setAttribute('aria-busy','false');hydrate();}
}
boot();
