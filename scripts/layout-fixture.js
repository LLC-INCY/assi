// Development-only UI state. No user data, native capture, file writes or network sessions.
const mode=new URLSearchParams(location.search).get('scenario')||'idle';
const receiving=mode==='file-offer',file=receiving||mode==='file-progress';
let board={request_id:'fixture',revision:1,strokes:[{id:'test-arrow',author:'viewer',tool:'arrow',color:'mint',width:3,points:[{x:.57,y:.6},{x:.73,y:.4}]}]};
window.fixtureInputs=[];
window.__TAURI__={core:{invoke:async(command,args={})=>{
 if(command==='session_snapshot')return {session:{role:mode==='viewer'||file?'viewer':mode==='pending'||mode==='host'||mode==='android-pending'?'host':'idle',state:mode==='viewer'||mode==='host'||file?'active':mode==='pending'||mode==='android-pending'?'pending':'idle',control:mode==='viewer',peer_name:'Тестовый компьютер',request_id:'fixture',transfer:file?{id:'fixture-file',name:'Документы для согласования — версия 2026.zip',size:104857600,transferred:receiving?0:52428800,direction:receiving?'receiving':'sending',state:receiving?'offered':'transferring',error:null}:null},capabilities:{capture:true,control:mode!=='android-pending'},audio_supported:true,audio_error:null,overlay_supported:mode!=='android-pending',overlay_enabled:true,files_supported:mode!=='android-pending',media_error:null};
 if(command==='session_frame')return fetch('screen-fixture.jpg').then(r=>r.ok?r.arrayBuffer():[]);
 if(command==='session_annotations')return structuredClone(board);
 if(command==='session_annotate'){board.revision++;if(args.command.type==='add')board.strokes.push({...args.command.stroke,author:mode==='host'?'host':'viewer'});else if(args.command.type==='clear')board.strokes=[];else board.strokes.pop();return;}
 if(command==='session_audio')return {request_id:'fixture',audio_epoch:0,audio_enabled:false,packets:[]};
 if(command==='session_capture_options')return {displays:[{id:'1',name:'Встроенный экран',width:1920,height:1080,primary:true}],selected:'1',quality:'balanced'};
 if(command==='session_capture_set')return;
 if(command==='session_input'){window.fixtureInputs.push(args.input);return;}
 if(command==='session_fullscreen'||command==='session_overlay_enable')return;

 if(command!=='load_snapshot')throw 'Тестовый стенд: действие не выполняется';
 return {platform:mode==='android-pending'?'Android':'macOS',version:'layout-test',data:{installation_id:'layout-fixture',settings:{device_name:'Рабочий ноутбук с длинным названием',theme:'dark'},devices:Array.from({length:24},(_,i)=>({id:'test-'+i,name:i===0?'ОченьДлинноеНазваниеБезПробеловДляПроверкиПереполнения':'Рабочая станция '+(i+1),code:String(123456789+i),os:i%2?'Windows':'Linux',group:i%3?'work':'personal'}))}};
}}};
