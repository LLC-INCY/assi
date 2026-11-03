'use strict';
const $ = selector => document.querySelector(selector);
const esc = value => String(value).replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const paths = {
  search:'<circle cx="10.5" cy="10.5" r="7.5"/><path d="m16 16 5 5"/>',
  sun:'<circle cx="12" cy="12" r="4"/><path d="M12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5"/>',
  laptop:'<path d="M4 17V4h16v13M2 17h20l-1 3H3z"/>',
  monitor:'<rect x="2" y="3" width="20" height="14" rx="1.5"/><path d="M12 17v4m-5 0h10"/>',
  phone:'<rect x="6" y="2" width="12" height="20" rx="2.5"/><path d="M11 18h2"/>',
  server:'<rect x="5" y="2" width="14" height="20" rx="2"/><path d="M9 7h6m-6 4h6m-6 6h2"/>',
  arrow:'<path d="M4 12h16m-6-6 6 6-6 6"/>',
  grid:'<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',
  list:'<path d="M8 5h13M8 12h13M8 19h13M3 5h.1M3 12h.1M3 19h.1"/>',
  plus:'<path d="M12 4v16M4 12h16"/>',
  copy:'<rect x="8" y="7" width="12" height="14" rx="2"/><path d="M15 7V3H3v13h5"/>',
  link:'<path d="m9 15 6-6m-5-3 1-1a5 5 0 0 1 7 7l-2 2m-2 4-1 1a5 5 0 0 1-7-7l2-2"/>',
  folder:'<path d="M3 6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  home:'<path d="m3 10 9-8 9 8v11h-6v-7H9v7H3z"/>',
  layers:'<path d="m12 2 10 5-10 5L2 7zm-10 10 10 5 10-5m-20 5 10 5 10-5"/>',
  settings:'<path d="m9 3 .5-2h5L15 3l2 1 2-.5 2.5 4-1.5 1.5v3L21.5 14 19 18l-2-.5-2 1-.5 2h-5L9 18.5l-2-1-2 .5-2.5-4L4 12V9L2.5 7.5l2.5-4L7 4z" transform="translate(0 1)"/><circle cx="12" cy="11.5" r="3"/>',
  close:'<path d="m6 6 12 12M6 18 18 6"/>',
  shield:'<path d="m12 2 8 4v6c0 5-8 10-8 10S4 17 4 12V6zM8 12l3 3 5-6"/>',
  clock:'<circle cx="12" cy="12" r="9"/><path d="M12 6v6l4 2"/>',
  check:'<path d="m5 12 4 4L19 6"/>'
};
const icon = name => `<svg viewBox="0 0 24 24" aria-hidden="true">${paths[name] || paths.monitor}</svg>`;
function hydrate(root=document) { root.querySelectorAll('[data-icon]').forEach(el => el.innerHTML=icon(el.dataset.icon)); }
const devices = [
  {id:'mac',name:'MacBook Pro',os:'macOS',group:'personal',code:'482916305',online:true,kind:'laptop',art:true},
  {id:'home',name:'Домашний ПК',os:'Windows',group:'personal',code:'139475286',online:true,kind:'monitor'},
  {id:'pixel',name:'Pixel 9',os:'Android',group:'personal',code:'265813047',online:false,kind:'phone'},
  {id:'office',name:'Офисный ПК',os:'Windows',group:'work',code:'731408562',online:true,kind:'monitor'},
  {id:'linux',name:'Рабочая станция',os:'Linux',group:'work',code:'928415603',online:true,kind:'server'}
];
document.body.classList.toggle('preview-desktop',new URLSearchParams(location.search).has('desktop'));
const OWN_CODE='584209176';
const state={page:'devices',selected:'mac',query:'',group:'all',view:'detail',sessionTab:'active',sessions:[],history:[],mobile:new URLSearchParams(location.search).has('mobile')};
let toastTimer, lastFocus, nextSession=1;
function formatCode(value){return value.match(/.{1,3}/g).join(' ')}
function toast(text){$('#toast').textContent=text;$('#toast').hidden=false;clearTimeout(toastTimer);toastTimer=setTimeout(()=>$('#toast').hidden=true,3500)}
function matches(d){const q=state.query.trim().toLocaleLowerCase('ru');const compact=q.replace(/\s/g,'');return (!q||`${d.name} ${d.os}`.toLocaleLowerCase('ru').includes(q)||d.code.includes(compact))&&(state.group==='all'||state.group===d.group)}
function status(d,pill=false){return `<span class="status${pill?' pill':''}"><span class="dot${d.online?'':' offline'}"></span>${d.online?'В сети':'Не в сети'}</span>`}
function connectButton(d,primary=false){return `<button class="connect-arrow${primary?' primary':''}" data-connect="${esc(d.id)}" ${d.online?'':'disabled'} aria-label="Подключиться: ${esc(d.name)}" title="${d.online?'Подключиться':'Устройство не в сети'}">${icon('arrow')}</button>`}
function renderSidebar(){
 const html=['personal','work'].map(group=>{const list=devices.filter(d=>d.group===group&&matches(d));return list.length?`<section class="device-group"><h2>${group==='personal'?'Личные':'Рабочие'}</h2>${list.map(d=>`<button class="side-device" data-select="${d.id}" aria-pressed="${d.id===state.selected}" title="${esc(d.name)} · ${d.online?'В сети':'Не в сети'}">${icon(d.kind)}<span class="device-name">${esc(d.name)}</span><span class="dot${d.online?'':' offline'}" aria-hidden="true"></span><span class="sr-only">${d.online?'В сети':'Не в сети'}</span></button>`).join('')}</section>`:''}).join('');
 $('#sidebar-devices').innerHTML=html||'<p class="empty">Ничего не найдено</p>';
}
function recentRow(d){return `<div class="recent-row"><span>${icon(d.kind)}</span><div class="row-caption"><strong>${esc(d.name)}</strong><small>${d.os}</small></div>${connectButton(d)}</div>`}
function mobileRow(d){return `<article class="mobile-device${d.id===state.selected?' selected':''}"><span class="device-picture">${d.art?'<img src="assets/laptop.png" alt="" width="62" height="47">':icon(d.kind)}</span><div class="row-caption"><strong>${esc(d.name)}</strong><small>${d.os}</small>${status(d)}</div>${connectButton(d,d.id===state.selected)}</article>`}
function filters(){return `<div class="group-filter" aria-label="Группы устройств">${[['all','Все'],['personal','Личные'],['work','Рабочие']].map(([value,label])=>`<button data-group="${value}" aria-pressed="${state.group===value}">${label}</button>`).join('')}</div>`}
function noDevices(){return '<div class="empty"><h3>Устройства не найдены</h3><p>Попробуйте другое имя или выберите другую группу.</p><button class="button" data-action="reset-search">Сбросить поиск</button></div>'}
function codeResult(){const q=state.query.replace(/\s/g,'');if(!/^\d{9}$/.test(q))return '';return `<div class="info-note search-code"><p>Подключение по коду ${formatCode(q)}</p><button class="button primary" data-action="connect-code" style="margin-top:12px">Подключиться по коду ${icon('arrow')}</button></div>`}
function renderDevices(){
 const d=devices.find(d=>d.id===state.selected)||devices[0];const list=devices.filter(matches);
 let desktop;
 if(state.query||state.view==='list'){
  desktop=`<section class="desktop-catalogue"><div class="catalogue-header"><h2>${state.query?'Результаты поиска':'Все устройства'}</h2><small class="muted">${list.length}</small></div>${codeResult()}${list.length?list.map(x=>`<div class="catalogue-row"><span>${icon(x.kind)}</span><div class="row-caption"><strong>${esc(x.name)}</strong><small>${x.os} · ${x.group==='work'?'Рабочие':'Личные'}</small></div>${status(x)}${connectButton(x)}</div>`).join(''):noDevices()}</section>`;
 }else{
  desktop=`<section class="desktop-detail"><div class="detail-title"><div><h2>${esc(d.name)}</h2><p>${d.os} · ${d.group==='work'?'Рабочие':'Личные'}</p></div>${status(d,true)}</div><div class="device-art${d.art?'':' generic'}">${d.art?'<img src="assets/laptop.png" alt="MacBook Pro с бирюзовыми обоями" width="1536" height="1024">':icon(d.kind)}</div><div class="device-actions"><button class="button primary" data-connect="${d.id}" ${d.online?'':'disabled'}>${icon('monitor')}Подключиться</button><button class="button" data-files="${d.id}" ${d.online?'':'disabled'}>${icon('folder')}Передать файлы</button></div>${!d.online?'<p class="info-note" style="margin-bottom:20px">Устройство не в сети. Откройте Асси на нём и проверьте интернет.</p>':''}<section class="recent"><h3>Недавние подключения</h3>${['office','linux'].map(id=>recentRow(devices.find(x=>x.id===id))).join('')}</section></section>`;
 }
 const mobile=`<section class="mobile-devices">${filters()}${codeResult()}${list.length?list.map(mobileRow).join(''):noDevices()}<button class="button mobile-add" data-action="add">${icon('plus')}Добавить устройство</button></section>`;
 $('#main').innerHTML=desktop+mobile;
 $('#result-status').textContent=state.query?`Найдено устройств: ${list.length}`:'';
}
function renderSessions(){
 const history=state.sessionTab==='history';const list=history?state.history:state.sessions;
 $('#main').innerHTML=`<div class="page-heading"><h2>Сеансы</h2><p>Управляйте подключениями и просматривайте историю.</p></div><div class="session-tabs" role="tablist" aria-label="Список сеансов"><button role="tab" data-session-tab="active" aria-selected="${!history}">Активные · ${state.sessions.length}</button><button role="tab" data-session-tab="history" aria-selected="${history}">История · ${state.history.length}</button></div>${list.length?list.map(s=>`<article class="session-item"><div class="row">${icon('monitor')}<div class="row-caption"><strong>${esc(s.name)}</strong><small>${history?'Завершён':'Демонстрационный сеанс'} · ${s.time}</small></div></div><p>Экран и ввод не передаются. Это проверка сценария интерфейса.</p>${history?'':`<button class="button" data-end-session="${s.id}">Завершить демосеанс</button>`}</article>`).join(''):`<div class="empty">${icon('layers')}<h3 style="margin-top:14px">${history?'История пока пуста':'Нет активных сеансов'}</h3><p>${history?'Завершённые демосеансы появятся здесь.':'Выберите устройство, чтобы посмотреть сценарий подключения.'}</p><button class="button" data-page="devices">К устройствам</button></div>`}`;
}
function renderSettings(){
 $('#main').innerHTML=`<div class="page-heading"><h2>Настройки</h2><p>Оформление и информация об этом макете.</p></div><section class="settings-section"><h3>Внешний вид</h3><div class="setting-row"><div><strong>Тема приложения</strong><p>Применяется к текущему просмотру.</p></div><select id="theme-select" aria-label="Тема приложения"><option value="dark" ${document.body.classList.contains('light')?'':'selected'}>Тёмная</option><option value="light" ${document.body.classList.contains('light')?'selected':''}>Светлая</option></select></div></section><section class="settings-section"><h3>Доступ</h3><div class="setting-row"><div><strong>С подтверждением</strong><p>Владелец разрешает каждый сеанс отдельно.</p></div>${icon('shield')}</div></section><div class="info-note">Интерактивный макет с вымышленными устройствами. Удалённый доступ, передача файлов и постоянное хранение данных пока не реализованы. Изменения сбросятся при перезагрузке.</div>`;
 $('#theme-select').onchange=e=>setTheme(e.target.value==='light');
}
function render(){
 const focused=document.activeElement;
 const focusKey=['data-select','data-group','data-session-tab','data-end-session'].find(key=>focused?.hasAttribute?.(key));
 const focusValue=focusKey?focused.getAttribute(focusKey):null;
 document.querySelectorAll('[data-page]').forEach(b=>{if(b.dataset.page===state.page)b.setAttribute('aria-current','page');else b.removeAttribute('aria-current')});
 document.querySelectorAll('.session-count').forEach(b=>{b.textContent=state.sessions.length;b.hidden=!state.sessions.length});
 $('#detail-view').setAttribute('aria-pressed',String(state.view==='detail'));$('#list-view').setAttribute('aria-pressed',String(state.view==='list'));
 renderSidebar();
 if(state.page==='devices')renderDevices();else if(state.page==='sessions')renderSessions();else renderSettings();
 if(focusKey){const replacement=document.querySelector(`[${focusKey}="${CSS.escape(focusValue)}"]`);(replacement||$('#main')).focus({preventScroll:true})}
}
function setPage(page){const changed=state.page!==page;state.page=page;if(page!=='devices'){state.query='';$('#search').value=''}render();if(changed)$('#main').focus({preventScroll:true})}
function setTheme(light){document.body.classList.toggle('light',light);$('#theme-toggle').setAttribute('aria-label',light?'Включить тёмную тему':'Включить светлую тему')}
function modal(title,content){lastFocus=document.activeElement;$('#dialog-title').textContent=title;$('#dialog-content').innerHTML=content;if(!$('#dialog').open)$('#dialog').showModal();hydrate($('#dialog'))}
function closeModal(){$('#dialog').close();if(lastFocus?.isConnected)lastFocus.focus()}
function startRequest(d){
 if(!d.online){toast('Устройство не в сети');return}
 modal('Подключение к устройству',`<h3 class="dialog-device">${esc(d.name)}</h3><p class="dialog-subtitle">Код ${formatCode(d.code)}</p><div class="permission-note">${icon('shield')}<div>Подтверждение на удалённом устройстве<small>В приложении владелец увидит запрос и выберет разрешения.</small></div></div><p class="dialog-subtitle">В макете запрос не отправляется. Можно открыть демонстрационный сеанс, чтобы проверить интерфейс.</p><div class="dialog-actions"><button class="button" data-action="close">Отмена</button><button class="button primary" id="demo-start">Открыть демосеанс</button></div>`);
 $('#demo-start').onclick=()=>{const existing=state.sessions.find(s=>s.deviceId===d.id);if(!existing)state.sessions.push({id:nextSession++,deviceId:d.id,name:d.name,time:new Intl.DateTimeFormat('ru',{hour:'2-digit',minute:'2-digit',timeZone:'Asia/Vladivostok'}).format(new Date())});closeModal();state.sessionTab='active';setPage('sessions');toast(existing?'Демосеанс уже открыт':'Открыт демонстрационный сеанс')};
}
function connectCode(){const q=state.query.replace(/\s/g,'');if(!/^\d{9}$/.test(q)){toast('Введите 9 цифр кода устройства');return}if(q===OWN_CODE){toast('Это ваш код. Введите код другого устройства.');return}const d=devices.find(x=>x.code===q)||{id:'code-'+q,name:'Удалённое устройство',code:q,online:true};startRequest(d)}
function access(){modal('Предоставить доступ',`<p class="dialog-subtitle">Передайте код человеку, которому хотите разрешить подключение.</p><p class="share-code">${formatCode(OWN_CODE)}</p><div class="permission-note">${icon('shield')}<div>Каждый запрос требует подтверждения<small>Вы сможете выбрать разрешения и завершить сеанс в любой момент.</small></div></div><p class="dialog-subtitle">Это демо-код. Он не предоставляет доступ к вашему устройству.</p><div class="dialog-actions"><button class="button" data-action="close">Закрыть</button><button class="button primary" data-action="copy">${icon('copy')}Скопировать код</button></div>`)}
async function copyCode(){try{if(!navigator.clipboard?.writeText)throw new Error('unavailable');await navigator.clipboard.writeText(formatCode(OWN_CODE));toast('Демо-код скопирован')}catch{toast('Код для копирования: '+formatCode(OWN_CODE))}}
function addDevice(){
 modal('Добавить устройство',`<p class="dialog-subtitle">Устройство появится в вашем демосписке. Реальное подключение не выполняется.</p><form id="add-form" novalidate><label class="field">Название<input id="device-name" name="name" maxlength="60" placeholder="Например, рабочий ноутбук" required></label><label class="field">Код устройства<input id="device-code" name="code" inputmode="numeric" maxlength="11" placeholder="000 000 000" required></label><label class="field">Система<select name="os"><option>Windows</option><option>macOS</option><option>Linux</option><option>Android</option></select></label><label class="field">Группа<select name="group"><option value="personal">Личные</option><option value="work">Рабочие</option></select></label><p class="form-error" id="add-error" role="alert"></p><div class="dialog-actions"><button class="button" type="button" data-action="close">Отмена</button><button class="button primary" type="submit">Добавить</button></div></form>`);
 $('#device-code').oninput=e=>e.target.value=e.target.value.replace(/\D/g,'').slice(0,9).replace(/(\d{3})(?=\d)/g,'$1 ');
 $('#add-form').onsubmit=e=>{e.preventDefault();const values=new FormData(e.target);const name=String(values.get('name')).trim();const code=String(values.get('code')).replace(/\D/g,'');const os=String(values.get('os'));const group=String(values.get('group'));const error=$('#add-error');if(!name){error.textContent='Укажите название устройства.';$('#device-name').focus();return}if(code.length!==9){error.textContent='Код должен содержать 9 цифр.';$('#device-code').focus();return}if(code===OWN_CODE){error.textContent='Это код текущего устройства.';return}if(devices.some(d=>d.code===code)){error.textContent='Устройство с таким кодом уже добавлено.';return}const id='local-'+code;devices.push({id,name,code,os,group,online:false,kind:os==='Android'?'phone':os==='macOS'?'laptop':'monitor'});state.selected=id;state.query='';state.group='all';$('#search').value='';closeModal();setPage('devices');toast('Устройство добавлено в демосписок')};
}
function files(d){
 modal('Передача файлов',`<h3 class="dialog-device">${esc(d.name)}</h3><p class="dialog-subtitle">Можно проверить выбор файлов. Они останутся на вашем устройстве: макет ничего не загружает и не отправляет.</p><label class="button file-label">${icon('folder')}Выбрать файлы<input class="sr-only" id="file-picker" type="file" multiple></label><ul id="files-list" class="files-list"></ul><p class="info-note">Передача будет доступна после реализации Rust-клиента и сетевого соединения.</p><div class="dialog-actions"><button class="button" data-action="close">Закрыть</button></div>`);
 $('#file-picker').onchange=e=>{const selected=[...e.target.files];$('#files-list').innerHTML=selected.map(f=>`<li>${esc(f.name)} <span class="muted">· ${Math.max(1,Math.round(f.size/1024))} КБ · не отправлен</span></li>`).join('')};
}
$('#search').oninput=e=>{state.query=e.target.value;state.page='devices';render()};
$('#search-form').onsubmit=e=>{e.preventDefault();const q=state.query.replace(/\s/g,'');if(/^\d+$/.test(q)){connectCode();return}const results=devices.filter(matches);if(results.length===1){state.selected=results[0].id;state.query='';$('#search').value='';state.view='detail';render()}else if(!state.query.trim())$('#search').focus()};
$('#theme-toggle').onclick=()=>{setTheme(!document.body.classList.contains('light'));if(state.page==='settings')renderSettings()};
$('#detail-view').onclick=()=>{state.view='detail';setPage('devices')};$('#list-view').onclick=()=>{state.view='list';setPage('devices')};$('#add-device').onclick=addDevice;
$('#close-dialog').onclick=closeModal;$('#dialog').addEventListener('cancel',()=>{if(lastFocus?.isConnected)lastFocus.focus()});
$('#preview-toggle').onclick=()=>{state.mobile=!state.mobile;document.body.classList.toggle('preview-desktop',!state.mobile&&new URLSearchParams(location.search).has('desktop'));$('#frame').classList.toggle('preview-mobile',state.mobile);$('#preview-toggle').textContent=state.mobile?'Десктопный вид':'Мобильный вид'};
$('.brand').onclick=e=>{e.preventDefault();state.query='';state.group='all';$('#search').value='';setPage('devices')};
document.addEventListener('keydown',e=>{
 if(e.target.matches('[data-session-tab]')&&['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){
  e.preventDefault();state.sessionTab=e.key==='Home'?'active':e.key==='End'?'history':state.sessionTab==='active'?'history':'active';render();document.querySelector(`[data-session-tab="${state.sessionTab}"]`).focus();return;
 }
 if((e.metaKey||e.ctrlKey)&&e.key.toLowerCase()==='k'&&!$('#dialog').open){e.preventDefault();$('#search').focus();$('#search').select()}});
document.addEventListener('click',e=>{
 const button=e.target.closest('button');if(!button||button.disabled)return;
 if(button.dataset.page){setPage(button.dataset.page);return}
 if(button.dataset.select){state.selected=button.dataset.select;state.query='';$('#search').value='';state.view='detail';setPage('devices');return}
 if(button.dataset.connect){const d=devices.find(x=>x.id===button.dataset.connect);if(d)startRequest(d);return}
 if(button.dataset.files){const d=devices.find(x=>x.id===button.dataset.files);if(d?.online)files(d);return}
 if(button.dataset.group){state.group=button.dataset.group;render();return}
 if(button.dataset.sessionTab){state.sessionTab=button.dataset.sessionTab;render();return}
 if(button.dataset.endSession){const id=Number(button.dataset.endSession);const s=state.sessions.find(x=>x.id===id);if(s){state.history.unshift(s);state.sessions=state.sessions.filter(x=>x.id!==id);render();toast('Демосеанс завершён')}return}
 switch(button.dataset.action){case 'copy':copyCode();break;case 'access':access();break;case 'close':closeModal();break;case 'add':addDevice();break;case 'connect-code':connectCode();break;case 'reset-search':state.query='';state.group='all';$('#search').value='';render();break}
});
$('#frame').classList.toggle('preview-mobile',state.mobile);$('#preview-toggle').textContent=state.mobile?'Десктопный вид':'Мобильный вид';hydrate();render();
