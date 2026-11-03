import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const files=readFileSync(new URL('./files-ui.js',import.meta.url),'utf8');
const sessions=readFileSync(new URL('./session-ui.js',import.meta.url),'utf8');
test('file progress cannot recreate focused remote screen',async()=>{
 const old={role:'viewer',state:'active',request_id:'same',control:true,transfer:{id:'file',state:'transferring',transferred:10,size:100}};
 let renders=0,updates=0;
 const ctx={state:{data:{},page:'sessions'},remote:{snapshot:{session:old,media_error:null},polling:false,generation:0},remoteState:()=>ctx.remote.snapshot.session,$:q=>q==='#dialog'?{open:false}:{textContent:''},invoke:async()=>({session:{...old,transfer:{...old.transfer,transferred:90}},media_error:null}),paintSessionChrome(){},updateSessionAudio(){},renderLiveSessions(){renders++;},updateTransfer(previous){updates++;assert.equal(previous.transferred,10);},setTimeout(){},clearTimeout(){}};
 vm.createContext(ctx);vm.runInContext(sessions.slice(sessions.indexOf('async function refreshSession('),sessions.indexOf('function clearRemoteFrame(')),ctx);await ctx.refreshSession();
 assert.equal(renders,0);assert.equal(updates,1);assert.equal(ctx.remote.generation,0);
});
test('file consent sends IDs displayed on the clicked button',async()=>{
 const calls=[];
 const ctx={remoteState:()=>({request_id:'replacement'}),$:()=>null,invoke:async(command,args)=>calls.push({command,args}),refreshSession:async()=>{},toast(){}};
 vm.createContext(ctx);vm.runInContext(files,ctx);
 await ctx.fileAction('file-accept',{dataset:{requestId:'shown-session',transferId:'shown-file'},isConnected:false});
 assert.equal(calls[0].command,'session_file_accept');assert.equal(calls[0].args.requestId,'shown-session');assert.equal(calls[0].args.transferId,'shown-file');
});
test('concurrent file picker action cannot start a second native picker',async()=>{
 let finish;let calls=0;const wait=new Promise(resolve=>finish=resolve);
 const ctx={$:()=>null,invoke:async()=>{calls++;await wait;},refreshSession:async()=>{},toast(){}};
 vm.createContext(ctx);vm.runInContext(files,ctx);const button={dataset:{requestId:'same'},isConnected:false};
 const first=ctx.fileAction('file-offer',button);await ctx.fileAction('file-offer',button);assert.equal(calls,1);finish();await first;
});

function fileContext(transfer){
 const ctx={remote:{snapshot:{files_supported:true}},snapshot:{state:'active',request_id:'A',transfer},esc:value=>String(value).replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),icon:()=>'',remoteState:()=>ctx.snapshot};
 vm.createContext(ctx);vm.runInContext(files+'\nglobalThis.queue=fileQueue;globalThis.folder=folderTransfer;',ctx);return ctx;
}
test('pause waits for acknowledgement and sender continuation requires receiver permission',()=>{const c=fileContext({id:'F',name:'file.txt',state:'paused',direction:'sending',size:100,transferred:50,pause_ready:false,resume_requested:false});let html=c.transferContentMarkup();assert.match(html,/Приостанавливаем…/);assert.match(html,/data-session-action="file-resume"[^>]*disabled/);c.snapshot.transfer.pause_ready=true;html=c.transferContentMarkup();assert.match(html,/Запросить продолжение/);assert.doesNotMatch(html,/data-session-action="file-resume"[^>]*disabled/);c.snapshot.transfer.resume_requested=true;html=c.transferContentMarkup();assert.match(html,/Ожидаем разрешение/);assert.match(html,/data-session-action="file-resume"[^>]*disabled/);});
test('receiver explicitly approves continuation and paused transfer keeps visible progress',()=>{const c=fileContext({id:'F',name:'file.txt',state:'paused',direction:'receiving',size:100,transferred:50,pause_ready:true,resume_requested:true});const html=c.transferContentMarkup();assert.match(html,/Разрешить продолжение/);assert.match(html,/value="50"/);assert.match(html,/data-session-action="file-cancel"/);});
test('folder bundle requires explicit unpack action and offers saving untouched bundle',()=>{const c=fileContext({id:'F',name:'Documents.assi-folder',state:'offered',direction:'receiving',size:100,transferred:0});const html=c.transferContentMarkup();assert.match(html,/data-session-action="file-accept-folder"/);assert.match(html,/Принять и распаковать папку/);assert.match(html,/Сохранить пакет файлом/);c.snapshot.transfer.name='Documents.zip';assert.doesNotMatch(c.transferContentMarkup(),/data-session-action="file-accept-folder"/);});
test('paused queue entry cannot be removed as though already finished',()=>{const c=fileContext(null);c.queue.request='A';c.queue.items=[{id:2,name:'Docs',kind:'folder',size:100,state:'paused'}];const html=c.fileQueueMarkup();assert.match(html,/Папка · Приостановлен/);assert.doesNotMatch(html,/file-queue-remove/);});
test('late folder status cannot replace a different session',async()=>{const c=fileContext(null);let resolve;c.$=()=>null;c.invoke=()=>new Promise(r=>resolve=r);const pending=c.pollFolderTransfer();c.snapshot.request_id='B';resolve({request_id:'A',state:'completed',name:'old'});await pending;assert.equal(c.folder.snapshot,null);});
test('late picker failure cannot rerender or announce in replacement session',async()=>{let reject;const wait=new Promise((_,no)=>reject=no),c=fileContext(null);let renders=0,toasts=0;c.$=()=>({set innerHTML(value){renders++;}});c.invoke=()=>wait;c.refreshSession=async()=>{};c.toast=()=>toasts++;const pending=c.fileAction('file-offer-folder',{dataset:{requestId:'A'},isConnected:false});c.snapshot.request_id='B';reject('old picker');await pending;assert.equal(renders,0);assert.equal(toasts,0);});
