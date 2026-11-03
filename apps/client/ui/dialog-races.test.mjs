import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source=readFileSync(new URL('./app.js',import.meta.url),'utf8');
const tasks=readFileSync(new URL('./dialog-tasks.js',import.meta.url),'utf8');
const handlers=source.slice(source.indexOf('function addDevice(){'),source.indexOf('function mobileDetails('));

// Execute the real IPC handlers with deferred native responses, not a second implementation.
function harness() {
  let resolve,reject;
  const pending=new Promise((yes,no)=>{resolve=yes;reject=no;});
  const ctx={state:{data:{devices:[{id:'test',name:'ПК',code:'123456789'}]},selected:'test'},messages:[],closed:[],nodes:{},current:null};
  ctx.$=key=>ctx.nodes[key];ctx.esc=String;ctx.formatCode=String;ctx.icon=()=>'';
  ctx.invoke=()=>pending;ctx.toast=text=>ctx.messages.push(String(text));ctx.render=()=>{};ctx.refreshDeviceBook=()=>{};ctx.setPage=()=>{};
  ctx.FormData=class { *[Symbol.iterator](){yield ['name','ПК'];yield ['code','123456789'];yield ['os','Windows'];yield ['group','work'];} };
  vm.createContext(ctx);vm.runInContext(tasks+'\nvar dialogTasks=createDialogTasks();',ctx);
  ctx.modal=(title)=>{
    ctx.dialogTasks.advance();ctx.current=title;
    const submit={disabled:false};const error={textContent:''};
    const field=value=>({value,setAttribute(){},focus(){}});const form={elements:{name:field('ПК'),code:field('123456789')},querySelector:selector=>selector==='[type=submit]'?submit:error};
    ctx.nodes={'#add-form':form,'#add-error':error,'#confirm-remove':submit,'#remove-error':error};
  };
  ctx.closeModal=()=>{ctx.dialogTasks.advance();ctx.closed.push(ctx.current);ctx.current=null;ctx.nodes={};};
  vm.runInContext(handlers,ctx);
  return {ctx,resolve,reject};
}

for(const operation of ['add','remove']) {
  for(const result of ['success','failure']) {
    test(`${operation}: late ${result} never closes or writes into a newer dialog`,async()=>{
      const {ctx,resolve,reject}=harness();
      if(operation==='add')ctx.addDevice();else ctx.confirmRemove('test');
      const form=ctx.nodes['#add-form'];const button=ctx.nodes['#confirm-remove'];
      const running=operation==='add'?form.onsubmit({preventDefault(){},target:form}):button.onclick({target:button});
      ctx.closeModal();ctx.modal('Новая форма');delete ctx.nodes['#remove-error'];
      ctx.nodes['#add-error'].textContent='Не менять';
      if(result==='success')resolve({id:'added',name:'ПК',code:'987654321'});else reject('Ошибка записи');
      await running;
      assert.equal(ctx.current,'Новая форма');assert.equal(ctx.closed.length,1);
      assert.equal(ctx.nodes['#add-error'].textContent,'Не менять');
      if(result==='failure')assert.deepEqual(ctx.messages,['Ошибка записи']);
      else assert.equal(ctx.state.data.devices.length,operation==='add'?2:0);
    });
  }
}
