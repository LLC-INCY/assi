'use strict';
// Geometry is shared by drawing and input; all coordinates refer to the actual image.
const SessionViewMath = {
 point(rect,x,y,clamp=false){
  if(!rect.width||!rect.height)return null;
  const p={x:(x-rect.left)/rect.width,y:(y-rect.top)/rect.height};
  if(!Number.isFinite(p.x)||!Number.isFinite(p.y))return null;
  if(!clamp&&(p.x<0||p.y<0||p.x>1||p.y>1))return null;
  return {x:Math.max(0,Math.min(1,p.x)),y:Math.max(0,Math.min(1,p.y))};
 },
 scale(width,height,availableWidth,availableHeight,zoom){return zoom==='fit'?Math.min(availableWidth/width,availableHeight/height):Math.max(.25,Math.min(4,Number(zoom)));},
 sample(points,max=128){if(points.length<=max)return points;return Array.from({length:max},(_,i)=>points[Math.round(i*(points.length-1)/(max-1))]);},
 wheel(delta,mode,size){return Math.max(-100,Math.min(100,delta*(mode===1?1:mode===2?size/40:1/40)));}
};
const sessionView={request:null,mode:'pan',zoom:'fit',tool:'pen',color:'mint',width:3,board:{strokes:[],revision:0},gesture:null,polling:false,sending:false,expanded:false,observer:null,abort:null,revision:null,error:''};
const annotationPalette={mint:'#a3f3d5',amber:'#ffd166',rose:'#ff7e9d',blue:'#78b9ff',white:'#ffffff'};
const toolPaths={control:'M4 3l15 9-7 1-3 7z',pan:'M8 12V6a2 2 0 014 0v5-7a2 2 0 014 0v7-4a2 2 0 014 0v7c0 5-3 7-7 7h-1c-2 0-3-1-4-3l-4-5a2 2 0 013-2l3 3',draw:'M4 20l4-1L20 7l-3-3L5 16z M14 7l3 3',screen:'M3 4h18v13H3z M8 21h8 M12 17v4',keyboard:'M2 6h20v12H2z M5 9h1m3 0h1m3 0h1m3 0h1M5 12h1m3 0h1m3 0h1m3 0h1M7 15h10',sound:'M4 9h4l5-4v14l-5-4H4z M17 8a6 6 0 010 8m3-11a10 10 0 010 14',files:'M3 5h6l2 3h10v12H3z',expand:'M8 3H3v5m13-5h5v5M3 16v5h5m13-5v5h-5',stop:'M12 3v9 M6 5a9 9 0 1012 0',undo:'M8 4L3 9l5 5 M3 9h11a6 6 0 010 12',trash:'M3 6h18 M9 6V3h6v3 M5 6l1 15h12l1-15 M10 10v7m4-7v7',arrow:'M4 20L20 4 M9 4h11v11',rectangle:'M4 4h16v16H4z',camera:'M3 7h5l2-3h4l2 3h5v13H3z M16 13a4 4 0 11-8 0 4 4 0 018 0',more:'M5 12h.01M12 12h.01M19 12h.01'};
function toolIcon(name){if(name==='more')return '<svg viewBox="0 0 24 24" aria-hidden="true" fill="currentColor"><circle cx="5" cy="12" r="1.6"/><circle cx="12" cy="12" r="1.6"/><circle cx="19" cy="12" r="1.6"/></svg>';return `<svg viewBox="0 0 24 24" aria-hidden="true" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"><path d="${toolPaths[name]||toolPaths.more}"/></svg>`;}
function toolButton(id,label,name,attrs=''){return `<button ${id?`id="${id}"`:''} class="tool-icon" aria-label="${label}" title="${label}" ${attrs}>${toolIcon(name)}</button>`;}
function sessionViewportMarkup(){
 const s=remoteState(),viewer=s.role==='viewer';
 return `<section class="session-viewer" id="session-viewer" aria-label="Экран и совместные пометки">
 <div class="viewer-toolbar compact-toolbar" aria-label="Инструменты экрана"><div class="viewer-tools" role="group" aria-label="Режим работы">
 ${viewer?toolButton('','Управление','control',`data-view-mode="control" ${s.control?'':'disabled'}`):''}${toolButton('','Обзор','pan','data-view-mode="pan"')}${toolButton('','Рисовать','draw','data-view-mode="draw"')}</div><span class="tool-divider"></span>
 ${toolButton('','Экран и масштаб','screen','data-tool-menu="screen" aria-expanded="false"')}${viewer?toolButton('','Клавиатура','keyboard',`data-tool-menu="keyboard" aria-expanded="false" ${s.control?'':'disabled'}`):''}${toolButton('audio-tool','Звук','sound','data-tool-menu="audio" aria-expanded="false"')}${toolButton('viewer-files','Файлы','files',remote.snapshot?.files_supported?'':'disabled')}${toolButton('','Ещё','more','data-tool-menu="more" aria-expanded="false"')}
 <span class="toolbar-spacer"></span><span id="session-recording-status" class="recording-status" hidden role="status"></span>${toolButton('view-expand','Развернуть','expand','aria-pressed="false"')}${toolButton('','Завершить сеанс','stop','data-session-action="stop"')}
 <div class="tool-menu" id="menu-screen" hidden><strong>Экран</strong><label>Масштаб <select id="view-zoom" aria-label="Масштаб экрана"><option value="fit">Вписать</option>${[.5,.75,1,1.5,2,3,4].map(x=>`<option value="${x}">${x*100}%</option>`).join('')}</select></label>${!viewer&&state.platform!=='Android'?'<label>Качество <select id="capture-quality"><option value="low">Экономное</option><option value="balanced" selected>Сбалансированное</option><option value="high">Высокое</option></select></label><label>Монитор <select id="capture-display"><option value="">Загрузка…</option></select></label>':viewer?'<label>Монитор <select id="remote-display"><option value="">Загрузка…</option></select></label><p id="display-feedback">Во время смены экрана управление приостанавливается.</p>':'<p>На Android экран выбирается в системном запросе показа.</p>'}<p id="session-statistics" aria-live="off">Ждём данные видеопотока…</p></div>
 <div class="tool-menu" id="menu-keyboard" hidden><strong>Клавиатура</strong><div class="key-grid">${['Escape','Tab','Enter','Backspace','Delete','Insert','Home','End','PageUp','PageDown',...Array.from({length:12},(_,i)=>'F'+(i+1))].map(key=>`<button data-remote-key="${key}">${key.replace('Escape','Esc').replace('Backspace','⌫').replace('Page','Pg')}</button>`).join('')}</div><div class="key-shortcuts">${[['control,c','Ctrl+C'],['control,v','Ctrl+V'],['control,a','Ctrl+A'],['control,z','Ctrl+Z'],['alt,tab','Alt+Tab'],['meta,space','Win/⌘+Space']].map(([keys,label])=>`<button data-remote-chord="${keys}">${label}</button>`).join('')}</div><form id="remote-text-form"><label for="remote-text">Ввести текст · любая раскладка</label><textarea id="remote-text" rows="2" maxlength="2000" placeholder="Текст для удалённого устройства"></textarea><div class="text-send-actions"><button type="button" id="paste-remote-text">Из буфера</button><button type="submit">Отправить</button></div></form><p>Ctrl+Alt+Del и защищённые экраны требуют системной службы.</p></div>
 <div class="tool-menu" id="menu-audio" hidden><strong>Звук устройства</strong>${viewer?`<label class="compact-check"><input id="audio-listen" type="checkbox" ${sessionAudio.enabled?'checked':''}>Слушать звук</label><label>Громкость <input id="audio-volume" type="range" min="0" max="100" value="${sessionAudio.volume*100}"></label><p id="audio-hint">${s.audio_enabled?'Владелец разрешил передачу звука.':'Передачу звука включает владелец экрана.'}</p>`:`<label class="compact-check"><input id="audio-share" type="checkbox" ${s.audio_enabled?'checked':''} ${remote.snapshot?.audio_supported?'':'disabled'}>Передавать системный звук</label><p>${remote.snapshot?.audio_supported?'Микрофон не используется. Система может запросить разрешение.':'Передача системного звука на этой платформе пока недоступна.'}</p>`}<p id="audio-error" role="status">${esc(remote.snapshot?.audio_error||'')}</p></div>
 <div class="tool-menu" id="menu-more" hidden><strong>Сеанс</strong><button id="session-chat-open">Чат</button><button id="session-clipboard-open">Буфер обмена</button><button id="session-terminal-open">Терминал</button><button id="system-lock" hidden>Заблокировать удалённый экран</button><button id="system-restart" hidden>Перезагрузить удалённый компьютер</button><p id="system-feedback" role="status"></p><button id="session-recording-toggle">Записать видео без звука</button><button id="session-shot" ${state.platform==='Android'?'disabled title="Сохранение кадра на Android ещё не подключено"':''}>Сохранить кадр с пометками</button>${s.role==='host'?`<label class="compact-check"><input id="host-overlay" type="checkbox" ${remote.snapshot?.overlay_enabled?'checked':''} ${remote.snapshot?.overlay_supported?'':'disabled'}>Пометки поверх окон</label><p>${remote.snapshot?.overlay_supported?'Не перехватывают мышь.':'На этой платформе пометки видны в окне сеанса.'}</p>`:''}<p>Ctrl + Alt + Escape — завершить сеанс.</p></div>${sessionClipboardMarkup()}</div>
 <div class="drawing-toolbar compact-drawing" id="drawing-tools" hidden><div class="viewer-tools" role="group" aria-label="Фигура">${[['pen','Перо','draw'],['arrow','Стрелка','arrow'],['rectangle','Рамка','rectangle']].map(([id,label,icon])=>toolButton('',label,icon,`data-draw-tool="${id}"`)).join('')}</div><div class="viewer-tools" role="group" aria-label="Цвет пометок">${[['mint','Мятный'],['amber','Жёлтый'],['rose','Розовый'],['blue','Голубой'],['white','Белый']].map(([id,label])=>`<button class="color-swatch swatch-${id}" data-draw-color="${id}" aria-label="${label}" title="${label}"></button>`).join('')}</div><select id="draw-width" aria-label="Толщина линии"><option value="2">2</option><option value="3" selected>3</option><option value="6">6</option></select><span class="toolbar-spacer"></span>${toolButton('draw-undo','Отменить свою пометку','undo')}${toolButton('draw-clear','Очистить все пометки','trash')}</div>
 <div class="session-workspace" id="session-workspace"><div class="remote-screen" id="remote-screen" tabindex="0" role="region" aria-label="${s.role==='host'?'Предпросмотр своего экрана':'Удалённый экран'}. Режим выбирается на панели инструментов"><div class="remote-stage" id="remote-stage"><img id="remote-frame" alt="${s.role==='host'?'Предпросмотр своего экрана':'Удалённый экран'}" draggable="false"${remote.frameUrl?` src="${esc(remote.frameUrl)}"`:''}><canvas id="remote-video" width="0" height="0" hidden aria-label="Удалённый видеопоток"></canvas><svg id="annotation-layer" xmlns="http://www.w3.org/2000/svg" aria-hidden="true"></svg></div><p id="frame-wait" ${remote.frameUrl?'hidden':''}>Ждём изображение…</p><textarea id="remote-input-proxy" class="remote-input-proxy" aria-label="Ввод на удалённом устройстве" autocomplete="off" autocapitalize="off" spellcheck="false" tabindex="-1"></textarea></div>${sessionChatMarkup()}</div><section id="system-consent" class="terminal-panel" hidden></section><section id="terminal-panel" class="terminal-panel" hidden></section>
 <div class="viewer-footnote"><span id="view-hint"></span><span id="annotation-status" role="status"></span></div>
 ${remote.snapshot?.media_error?`<p class="form-error" role="status">${esc(remote.snapshot.media_error)}</p>`:''}</section>`;
}
function remoteFrameSurface(){return remote.videoVisible?$('#remote-video'):$('#remote-frame');}
function remoteFrameDimensions(){const surface=remoteFrameSurface();return {width:remote.videoVisible?surface?.width:surface?.naturalWidth,height:remote.videoVisible?surface?.height:surface?.naturalHeight};}
function resetSessionView(request){
 sessionView.abort?.abort();sessionView.observer?.disconnect();
 if(sessionView.expanded)leaveExpandedView();
 stopSessionAudio();
 if(typeof resetSessionTools==='function')resetSessionTools(request);
 if(typeof resetSessionTerminal==='function')resetSessionTerminal(request);
 if(typeof resetSessionSystem==='function')resetSessionSystem(request);
 if(typeof sessionRecordingCleanup==='function')sessionRecordingCleanup();
 if(typeof resetSessionVideo==='function')resetSessionVideo();
 Object.assign(sessionView,{request,mode:remoteState().role==='viewer'&&remoteState().control?'control':'pan',zoom:'fit',board:{strokes:[],revision:0},gesture:null,gestureEpoch:null,revision:null,error:''});
}
function sessionViewCleanup(){if(typeof resetSessionVideo==='function')resetSessionVideo();sessionView.abort?.abort();sessionView.observer?.disconnect();sessionView.gesture=null;releaseRemoteInput();if(sessionView.expanded)leaveExpandedView();}
function updateViewerTools(){
 const s=remoteState(),mode=sessionView.mode;
 document.querySelectorAll('[data-view-mode]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.viewMode===mode)));
 document.querySelectorAll('[data-draw-tool]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.drawTool===sessionView.tool)));
 document.querySelectorAll('[data-draw-color]').forEach(b=>b.setAttribute('aria-pressed',String(b.dataset.drawColor===sessionView.color)));
 if(!$('#drawing-tools'))return;
 $('#drawing-tools').hidden=mode!=='draw';$('#remote-screen').dataset.mode=mode;$('#view-zoom').value=String(sessionView.zoom);$('#draw-width').value=String(sessionView.width);
 $('#view-hint').textContent=mode==='draw'?'Пометки видны обоим. Escape — выйти из рисования.':mode==='pan'?'Перетаскивайте изображение. Колесо — перемещение, Ctrl + колесо — масштаб.': 'Мышь, прокрутка и клавиатура. Ctrl + Alt + Escape — завершить.';
 $('#annotation-status').textContent=sessionView.error||`Общие пометки: ${sessionView.board.strokes.length}`;
 $('#draw-undo').disabled=sessionView.sending||!sessionView.board.strokes.some(x=>x.author===s.role);
 $('#draw-clear').disabled=sessionView.sending||!sessionView.board.strokes.length;
 $('#view-expand').setAttribute('aria-label',sessionView.expanded?'Свернуть':'Развернуть');$('#view-expand').title=sessionView.expanded?'Свернуть':'Развернуть';$('#view-expand').setAttribute('aria-pressed',String(sessionView.expanded));
 layoutRemoteStage();
}
function layoutRemoteStage(){
 const screen=$('#remote-screen'),stage=$('#remote-stage'),{width,height}=remoteFrameDimensions();if(!screen||!width||!height)return;
 const scale=SessionViewMath.scale(width,height,screen.clientWidth,screen.clientHeight,sessionView.zoom);
 stage.style.width=`${Math.max(1,width*scale)}px`;stage.style.height=`${Math.max(1,height*scale)}px`;stage.style.marginTop=`${Math.max(0,(screen.clientHeight-height*scale)/2)}px`;
 drawAnnotations();
}
function drawAnnotations(){
 const svg=$('#annotation-layer'),{width:w,height:h}=remoteFrameDimensions();if(!svg||!w||!h)return;
 svg.replaceChildren();svg.setAttribute('viewBox',`0 0 ${w} ${h}`);
 const strokes=[...sessionView.board.strokes];if(sessionView.gesture?.stroke)strokes.push(sessionView.gesture.stroke);
 const node=(tag,attrs)=>{const el=document.createElementNS('http://www.w3.org/2000/svg',tag);for(const [k,v]of Object.entries(attrs))el.setAttribute(k,String(v));svg.append(el);return el;};
 for(const stroke of strokes){
  const pts=stroke.points.map(p=>({x:p.x*w,y:p.y*h}));if(!pts.length)continue;
  const a=pts[0],b=pts.at(-1),width=stroke.width*Math.min(w,h)/720;
  const attrs={fill:'none',stroke:annotationPalette[stroke.color]||annotationPalette.mint,'stroke-width':width,'stroke-linecap':'round','stroke-linejoin':'round'};
  if(stroke.tool==='rectangle')node('rect',{...attrs,x:Math.min(a.x,b.x),y:Math.min(a.y,b.y),width:Math.abs(a.x-b.x),height:Math.abs(a.y-b.y)});
  else if(stroke.tool==='arrow'){
   const angle=Math.atan2(b.y-a.y,b.x-a.x),len=Math.max(width*4,Math.min(w,h)*.018);
   node('path',{...attrs,d:`M ${a.x} ${a.y} L ${b.x} ${b.y} M ${b.x-len*Math.cos(angle-.5)} ${b.y-len*Math.sin(angle-.5)} L ${b.x} ${b.y} L ${b.x-len*Math.cos(angle+.5)} ${b.y-len*Math.sin(angle+.5)}`});
  }else if(pts.length===1)node('circle',{cx:a.x,cy:a.y,r:width/2,fill:attrs.stroke});
  else node('polyline',{...attrs,points:pts.map(p=>`${p.x},${p.y}`).join(' ')});
 }
}
async function refreshAnnotations(){
 const s=remoteState();if(sessionView.polling||s.state!=='active')return;
 sessionView.polling=true;
 try{const board=await invoke('session_annotations',{requestId:s.request_id});if(remoteState().state!=='active'||remoteState().request_id!==s.request_id||board.request_id!==s.request_id)return;
 if(sessionView.request!==s.request_id)resetSessionView(s.request_id);
 if(sessionView.revision!==board.revision){sessionView.board=board;sessionView.revision=board.revision;drawAnnotations();updateViewerTools();}
 }catch{}finally{sessionView.polling=false;}
}
async function submitAnnotation(command){
 const s=remoteState();if(s.state!=='active'||sessionView.sending)return;
 sessionView.sending=true;sessionView.error='';updateViewerTools();
 try{await invoke('session_annotate',{requestId:s.request_id,screenEpoch:sessionView.gestureEpoch??remote.displayedEpoch??s.screen_epoch,command});await refreshAnnotations();}
 catch(error){if(remoteState().request_id===s.request_id){sessionView.error=String(error);toast(error);}}
 finally{sessionView.gestureEpoch=null;sessionView.sending=false;updateViewerTools();}
}
async function leaveExpandedView(){
 sessionView.expanded=false;$('#session-viewer')?.classList.remove('expanded');
 try{await invoke('session_fullscreen',{enabled:false,requestId:remoteState().request_id||''});}catch{}
 updateViewerTools();
}
async function toggleExpandedView(){
 if(sessionView.expanded){await leaveExpandedView();return;}
 sessionView.expanded=true;$('#session-viewer')?.classList.add('expanded');updateViewerTools();
 try{await invoke('session_fullscreen',{enabled:true,requestId:remoteState().request_id});}catch{ /* The full-window viewer still works on unsupported runtimes. */ }
 $('#remote-screen')?.focus();
}
function bindSessionViewer(){
 const screen=$('#remote-screen');if(!screen)return;
 const proxy=$('#remote-input-proxy');let composing=false,compositionCommit=null;const heldKeys=new Set();
 const s=remoteState();if(sessionView.request!==s.request_id)resetSessionView(s.request_id);
 sessionView.abort?.abort();sessionView.observer?.disconnect();sessionView.abort=new AbortController();const signal=sessionView.abort.signal;
 const on=(el,event,fn,options={})=>el?.addEventListener(event,fn,{...options,signal});
 const point=(e,clamp=false)=>{const image=remoteFrameSurface();return remoteFrameDimensions().width?SessionViewMath.point(image.getBoundingClientRect(),e.clientX,e.clientY,clamp):null;};
 const control=()=>sessionView.mode==='control'&&remoteState().control&&remoteState().role==='viewer'&&!remoteState().screen_pending&&(remote.displayedEpoch===undefined||remote.displayedEpoch===remoteState().screen_epoch);
 let lastMove=0,wheelX=0,wheelY=0;
 on(screen,'contextmenu',e=>e.preventDefault());
 on(screen,'pointerdown',e=>{
  if(sessionView.gesture||!point(e))return;e.preventDefault();screen.focus();screen.setPointerCapture(e.pointerId);
  if(sessionView.mode==='draw'){
   if(e.button!==0||sessionView.sending)return;
   const p=point(e);sessionView.gestureEpoch=remote.displayedEpoch??s.screen_epoch;sessionView.gesture={pointer:e.pointerId,stroke:{id:crypto.randomUUID(),tool:sessionView.tool,color:sessionView.color,width:sessionView.width,points:sessionView.tool==='pen'?[p]:[p,p]}};drawAnnotations();
  }else if(sessionView.mode==='pan'){sessionView.gesture={pointer:e.pointerId,x:e.clientX,y:e.clientY,left:screen.scrollLeft,top:screen.scrollTop};}
  else if(control()){const p=point(e);emitInput({type:'move',...p});emitInput({type:'button',button:['left','middle','right'][e.button]||'left',down:true});}
 });
 on(screen,'pointermove',e=>{
  const g=sessionView.gesture;if(g&&g.pointer!==e.pointerId)return;
  if(g?.stroke){const p=point(e,true);if(!p)return;if(g.stroke.tool==='pen'){g.stroke.points.push(p);if(g.stroke.points.length>256)g.stroke.points=SessionViewMath.sample(g.stroke.points);}else g.stroke.points[1]=p;drawAnnotations();}
  else if(g){screen.scrollLeft=g.left+g.x-e.clientX;screen.scrollTop=g.top+g.y-e.clientY;}
  else if(control()&&performance.now()-lastMove>=40){lastMove=performance.now();const p=point(e);if(p)emitInput({type:'move',...p});}
 });
 on(screen,'pointerup',e=>{
  const g=sessionView.gesture;if(g&&g.pointer!==e.pointerId)return;sessionView.gesture=null;
  if(g?.stroke){const p=point(e,true);if(p){if(g.stroke.tool==='pen')g.stroke.points.push(p);else g.stroke.points[1]=p;}g.stroke.points=SessionViewMath.sample(g.stroke.points);submitAnnotation({type:'add',stroke:g.stroke});drawAnnotations();}
  else if(control())emitInput({type:'button',button:['left','middle','right'][e.button]||'left',down:false});
  if(screen.hasPointerCapture(e.pointerId))screen.releasePointerCapture(e.pointerId);
 });
 const cancel=()=>{sessionView.gestureEpoch=null;heldKeys.clear();wheelX=0;wheelY=0;sessionView.gesture=null;releaseRemoteInput();drawAnnotations();};
 on(screen,'pointercancel',cancel);on(screen,'blur',event=>{if(event?.relatedTarget!==proxy)cancel();});on(proxy,'blur',cancel);on(window,'blur',cancel);
 on(screen,'wheel',e=>{
  if(e.ctrlKey&&sessionView.mode!=='control'){e.preventDefault();const {width,height}=remoteFrameDimensions();if(!width)return;const current=SessionViewMath.scale(width,height,screen.clientWidth,screen.clientHeight,sessionView.zoom);const levels=[.5,.75,1,1.5,2,3,4];sessionView.zoom=e.deltaY<0?(levels.find(x=>x>current+.01)||4):([...levels].reverse().find(x=>x<current-.01)||.5);updateViewerTools();}
  else if(control()){e.preventDefault();const p=point(e);if(!p)return;emitInput({type:'move',...p});wheelX+=SessionViewMath.wheel(e.deltaX,e.deltaMode,screen.clientWidth);wheelY+=SessionViewMath.wheel(e.deltaY,e.deltaMode,screen.clientHeight);const x=Math.trunc(wheelX),y=Math.trunc(wheelY);wheelX-=x;wheelY-=y;if(x||y)emitInput({type:'wheel',x,y});}
 },{passive:false});
 const inputFocused=e=>e.target===screen||e.target===proxy||!e.target;
 on(screen,'focus',()=>{if(control())proxy?.focus({preventScroll:true});});
 on(screen,'compositionstart',()=>{composing=true;compositionCommit=null;releaseRemoteInput();});
 on(screen,'compositionend',event=>{composing=false;if(control()&&event.data){compositionCommit=event.data;emitInput({type:'text',text:event.data});}if(proxy)proxy.value='';});
 on(proxy,'beforeinput',event=>{if(!control()||event.isComposing||composing||event.inputType==='insertCompositionText')return;if(event.inputType==='insertText'||event.inputType==='insertFromComposition'){event.preventDefault();if(event.data&&event.data!==compositionCommit)emitInput({type:'text',text:event.data});compositionCommit=null;if(proxy)proxy.value='';}});
 on(screen,'keydown',e=>{
  if(!inputFocused(e))return;
  if(e.ctrlKey&&e.altKey&&e.key==='Escape'){e.preventDefault();sessionAction('stop');return;}
  if(e.key==='Escape'&&!control()){e.preventDefault();cancel();if(sessionView.mode==='draw'){sessionView.mode='pan';updateViewerTools();}else if(sessionView.expanded)leaveExpandedView();return;}
  if(!control()||e.isComposing||composing||e.key==='Process'||e.key==='Dead')return;
  compositionCommit=null;
  if(e.key?.length===1&&(!e.ctrlKey&&!e.altKey&&!e.metaKey||e.getModifierState?.('AltGraph'))){e.preventDefault();emitInput({type:'text',text:e.key});return;}
  const key=remoteKeys[e.key]||(/^[a-z0-9]$/i.test(e.key)?e.key.toLowerCase():null);if(key){e.preventDefault();if(!e.repeat){heldKeys.add(key);emitInput({type:'key',key,down:true});}}
 });
 on(screen,'keyup',e=>{if(!inputFocused(e)||!control()||e.isComposing||composing)return;const key=remoteKeys[e.key]||(/^[a-z0-9]$/i.test(e.key)?e.key.toLowerCase():null);if(key&&heldKeys.has(key)){heldKeys.delete(key);e.preventDefault();emitInput({type:'key',key,down:false});}});
 document.querySelectorAll('[data-view-mode]').forEach(b=>on(b,'click',()=>{cancel();sessionView.mode=b.dataset.viewMode;updateViewerTools();}));
 document.querySelectorAll('[data-draw-tool]').forEach(b=>on(b,'click',()=>{sessionView.tool=b.dataset.drawTool;updateViewerTools();}));
 document.querySelectorAll('[data-draw-color]').forEach(b=>on(b,'click',()=>{sessionView.color=b.dataset.drawColor;updateViewerTools();}));
 on($('#draw-width'),'change',e=>{sessionView.width=Number(e.target.value);});
 on($('#view-zoom'),'change',e=>{sessionView.zoom=e.target.value==='fit'?'fit':Number(e.target.value);layoutRemoteStage();});
 on($('#draw-undo'),'click',()=>submitAnnotation({type:'undo'}));on($('#draw-clear'),'click',()=>submitAnnotation({type:'clear'}));on($('#view-expand'),'click',toggleExpandedView);
 on($('#host-overlay'),'change',async e=>{const input=e.target;input.disabled=true;try{await invoke('session_overlay_enable',{requestId:s.request_id,enabled:input.checked});}catch(error){input.checked=!input.checked;toast(error);}finally{input.disabled=false;}});
 on($('#remote-frame'),'load',layoutRemoteStage);sessionView.observer=new ResizeObserver(layoutRemoteStage);sessionView.observer.observe(screen);updateViewerTools();refreshAnnotations();bindSessionTools(on);
}
function positionToolMenu(menu){const toolbar=$('.compact-toolbar');if(!toolbar||!menu)return;const rect=toolbar.getBoundingClientRect(),boundary=sessionView.expanded?(window.innerHeight||900):Math.min(window.innerHeight||900,$('#main')?.getBoundingClientRect().bottom||900),below=boundary-rect.bottom-12;menu.style.top='calc(100% + 6px)';menu.style.bottom='auto';menu.style.maxHeight=`${Math.max(100,Math.min(500,below))}px`;if(below<160&&rect.top>220){menu.style.top='auto';menu.style.bottom='calc(100% + 6px)';menu.style.maxHeight=`${Math.max(100,Math.min(500,rect.top-90))}px`;}}
function closeToolMenus(){document.querySelectorAll('.tool-menu').forEach(menu=>menu.hidden=true);document.querySelectorAll('[data-tool-menu]').forEach(b=>b.setAttribute('aria-expanded','false'));}
function sendRemoteChord(keys){if(remoteState().role!=='viewer'||!remoteState().control)return;releaseRemoteInput();for(const key of keys)emitInput({type:'key',key,down:true});for(const key of [...keys].reverse())emitInput({type:'key',key,down:false});}
async function loadCaptureOptions(){
 const id=remoteState().request_id;
 try{const options=await invoke('session_capture_options',{requestId:id});if(remoteState().request_id!==id||!$('#capture-display'))return;
 $('#capture-display').innerHTML=options.displays.map(d=>`<option value="${esc(d.id)}">${esc(d.name)} · ${d.width}×${d.height}${d.primary?' · основной':''}</option>`).join('');
 $('#capture-display').disabled=!!remoteState().screen_pending;$('#capture-display').value=options.selected||options.displays.find(d=>d.primary)?.id||'';$('#capture-quality').value=options.quality;
 }catch(error){toast(error);}
}
async function saveSessionShot(){
 const image=remoteFrameSurface(),{width,height}=remoteFrameDimensions();if(!width||!height)return;
 const id=remoteState().request_id,canvas=document.createElement('canvas');canvas.width=width;canvas.height=height;const ctx=canvas.getContext('2d');ctx.drawImage(image,0,0);
 const svg=$('#annotation-layer').cloneNode(true);svg.setAttribute('width',String(canvas.width));svg.setAttribute('height',String(canvas.height));
 const url=URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(svg)],{type:'image/svg+xml'}));
 try{const layer=new Image();layer.src=url;await layer.decode();ctx.drawImage(layer,0,0);const blob=await new Promise(r=>canvas.toBlob(r,'image/png'));if(!blob||remoteState().request_id!==id)return;
 const saved=await invoke('session_save_image',{requestId:id,bytes:Array.from(new Uint8Array(await blob.arrayBuffer()))});if(saved)toast('Кадр сохранён');
 }catch(error){toast(error);}finally{URL.revokeObjectURL(url);}
}
function bindSessionTools(on){
 document.querySelectorAll('[data-tool-menu]').forEach(b=>on(b,'click',()=>{const menu=$('#menu-'+b.dataset.toolMenu),open=menu.hidden;closeToolMenus();menu.hidden=!open;if(open)positionToolMenu(menu);b.setAttribute('aria-expanded',String(open));if(open){releaseRemoteInput();if(b.dataset.toolMenu==='screen'){if(remoteState().role==='host'&&$('#capture-display'))loadCaptureOptions();else updateRemoteDisplays();}}}));
 on(document,'pointerdown',e=>{if(!e.target.closest('.compact-toolbar'))closeToolMenus();});
 on(document,'keydown',e=>{if(e.key==='Tab'&&sessionView.expanded){const viewer=$('#session-viewer');const items=[...viewer.querySelectorAll('button:not(:disabled),input:not(:disabled),select:not(:disabled),textarea,[tabindex="0"]')].filter(x=>x.getClientRects().length);const first=items[0],last=items.at(-1);if(first&&((e.shiftKey&&document.activeElement===first)||(!e.shiftKey&&document.activeElement===last)||!viewer.contains(document.activeElement))){e.preventDefault();(e.shiftKey?last:first).focus();}}if(e.key==='Escape'&&e.target.closest('.tool-menu')){e.preventDefault();closeToolMenus();$('#remote-screen').focus();}});
 document.querySelectorAll('[data-remote-key]').forEach(b=>on(b,'click',()=>{const key=remoteKeys[b.dataset.remoteKey]||b.dataset.remoteKey.toLowerCase();sendRemoteChord([key]);}));
 document.querySelectorAll('[data-remote-chord]').forEach(b=>on(b,'click',()=>sendRemoteChord(b.dataset.remoteChord.split(','))));
 on($('#remote-text-form'),'submit',e=>{e.preventDefault();const input=$('#remote-text'),text=input.value.replace(/\r\n?/g,'\n');if(!text)return;if(new TextEncoder().encode(text).length>4096){toast('Не более 4096 байт текста за раз');return;}releaseRemoteInput();emitInput({type:'text',text});input.value='';});
 on($('#paste-remote-text'),'click',async()=>{const id=remoteState().request_id;try{const text=await navigator.clipboard.readText();if(id===remoteState().request_id&&$('#remote-text')){$('#remote-text').value=text.slice(0,2000);$('#remote-text').focus();}}catch{toast('Вставьте текст в поле сочетанием Ctrl/Cmd + V.');}});
 on($('#viewer-files'),'click',async()=>{closeToolMenus();if(sessionView.expanded)await leaveExpandedView();$('#transfer-panel')?.scrollIntoView({block:'start'});$('#transfer-panel')?.focus();});
 on($('#audio-listen'),'change',async e=>{const input=e.target;try{await enableSessionAudio(input.checked);updateSessionAudio();}catch(error){input.checked=false;toast(error);}});
 on($('#audio-volume'),'input',e=>{sessionAudio.volume=Number(e.target.value)/100;if(sessionAudio.gain)sessionAudio.gain.gain.value=sessionAudio.volume;});
 on($('#audio-share'),'change',async e=>{const input=e.target;input.disabled=true;try{await invoke('session_audio_enable',{requestId:remoteState().request_id,enabled:input.checked});await refreshSession();}catch(error){input.checked=!input.checked;toast(error);}finally{input.disabled=!remote.snapshot?.audio_supported;}});
 for(const id of ['capture-display','capture-quality'])on($('#'+id),'change',async e=>{const input=e.target;input.disabled=true;try{await invoke('session_capture_set',{requestId:remoteState().request_id,display:$('#capture-display').value,quality:$('#capture-quality').value});}catch(error){toast(error);}finally{input.disabled=false;await loadCaptureOptions();}});
 on($('#remote-display'),'change',async event=>{const input=event.target,id=remoteState().request_id;input.disabled=true;releaseRemoteInput();try{await invoke('session_display',{requestId:id,display:input.value});await refreshSession();}catch(error){toast(error);}finally{updateRemoteDisplays();}});
 on($('#session-shot'),'click',saveSessionShot);updateSessionAudio();if(typeof bindSharedTools==='function')bindSharedTools(on);if(typeof bindTerminalTools==='function')bindTerminalTools(on);if(typeof bindSystemTools==='function')bindSystemTools(on);
 on($('#session-recording-toggle'),'click',()=>{if(typeof toggleSessionRecording==='function')toggleSessionRecording();});
 if(typeof updateSessionRecording==='function')updateSessionRecording();updateRemoteDisplays();
}

function updateRemoteDisplays(){const input=$('#remote-display');if(!input)return;const s=remoteState(),displays=s.displays||[];input.innerHTML=displays.length?displays.map(d=>`<option value="${esc(d.id)}">${esc(d.name)} · ${d.width}×${d.height}</option>`).join(''):'<option value="">Нет доступных мониторов</option>';input.value=s.selected_display||'';input.disabled=displays.length<2||!!s.screen_pending;const hint=$('#display-feedback');if(hint)hint.textContent=s.screen_pending?'Переключаем экран. Управление приостановлено.':'Во время смены экрана управление приостанавливается.';}
