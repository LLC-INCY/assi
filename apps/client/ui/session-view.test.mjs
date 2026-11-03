import test from 'node:test';
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const source=readFileSync(new URL('./session-view.js',import.meta.url),'utf8');
function setup(){
 class Element {
  constructor(){this.events={};this.style={};this.dataset={};this.clientWidth=800;this.clientHeight=450;this.scrollLeft=0;this.scrollTop=0;this.naturalWidth=1600;this.naturalHeight=900;this.classList={add(){},remove(){}};}
  addEventListener(type,fn){this.events[type]=fn;}setAttribute(){}replaceChildren(){}append(){}focus(){}setPointerCapture(){}hasPointerCapture(){return false;}getBoundingClientRect(){return {left:100,top:50,width:800,height:450};}
 }
 const els=new Map();const ctx={AbortController,ResizeObserver:class{observe(){}disconnect(){}},crypto:{randomUUID:()=> '12345678-1234-1234-1234-123456789abc'},performance:{now:()=>100},inputs:[],calls:[],snapshot:{state:'active',role:'viewer',control:true,request_id:'A'},remoteKeys:{a:'a'},stopSessionAudio(){},sessionAudio:{enabled:false,volume:.7},updateSessionAudio(){},toast(){},esc:String,remote:{frameUrl:null},remoteState:()=>ctx.snapshot};
 ctx.$=s=>{if(!els.has(s))els.set(s,new Element());return els.get(s);};ctx.window=new Element();ctx.document={addEventListener(){},querySelectorAll:()=>[],createElementNS:()=>new Element()};ctx.emitInput=x=>ctx.inputs.push(x);ctx.releaseRemoteInput=()=>{};
 ctx.invoke=async(command,args)=>{ctx.calls.push({command,args});if(command==='session_annotations')return {request_id:ctx.snapshot.request_id,revision:0,strokes:[]};};
 vm.createContext(ctx);vm.runInContext(source+'\nglobalThis.math=SessionViewMath;globalThis.view=sessionView;',ctx);return ctx;
}
const event=(extra={})=>({clientX:300,clientY:200,button:0,pointerId:1,preventDefault(){},...extra});
const flush=()=>new Promise(r=>setImmediate(r));
test('coordinates account for zoom and panning; letterbox clicks are rejected',()=>{
 const c=setup(),r={left:-100,top:50,width:1600,height:900};assert.equal(c.math.point(r,700,500).x,.5);assert.equal(c.math.point(r,700,500).y,.5);assert.equal(c.math.point(r,-200,500),null);assert.equal(c.math.point(r,2000,500,true).x,1);assert.equal(c.math.point({width:0,height:0},1,1),null);
});
test('fit can shrink to arbitrary viewport without fixed screen dimensions',()=>{const c=setup();assert.equal(c.math.scale(1920,1080,240,400,'fit'),.125);assert.equal(c.math.scale(1920,1080,800,450,1),1);});
test('drawing submits shared normalized stroke and never injects keyboard/mouse',async()=>{
 const c=setup();c.bindSessionViewer();await flush();c.view.mode='draw';const screen=c.$('#remote-screen');screen.events.pointerdown(event());screen.events.pointermove(event({clientX:500}));screen.events.keydown(event({key:'a'}));screen.events.pointerup(event({clientX:600}));await flush();
 assert.equal(c.inputs.length,0);const sent=c.calls.find(x=>x.command==='session_annotate');assert.equal(sent.args.requestId,'A');assert.equal(sent.args.command.type,'add');assert.equal(sent.args.command.stroke.points[0].x,.25);assert.equal(sent.args.command.stroke.points.at(-1).x,.625);
});
test('pan gestures move local viewport and do not inject remote control',()=>{const c=setup();c.bindSessionViewer();c.view.mode='pan';const screen=c.$('#remote-screen');screen.events.pointerdown(event());screen.events.pointermove(event({clientX:200,clientY:100}));screen.events.pointerup(event());assert.equal(screen.scrollLeft,100);assert.equal(screen.scrollTop,100);assert.equal(c.inputs.length,0);});
test('wheel is normalized and bounded; no remote wheel in viewing mode',()=>{const c=setup();c.bindSessionViewer();const screen=c.$('#remote-screen');screen.events.wheel(event({deltaX:0,deltaY:120,deltaMode:0}));assert.equal(c.inputs.at(-1).type,'wheel');assert.equal(c.inputs.at(-1).y,3);assert.equal(c.math.wheel(1e9,0,450),100);c.view.mode='pan';c.inputs=[];screen.events.wheel(event({deltaX:0,deltaY:120,deltaMode:0}));assert.equal(c.inputs.length,0);});
test('long strokes remain bounded while keeping endpoints',()=>{const c=setup(),p=Array.from({length:1000},(_,i)=>({x:i/999,y:0})),sample=c.math.sample(p);assert.equal(sample.length,128);assert.equal(sample[0].x,0);assert.equal(sample.at(-1).x,1);});
test('stale board response cannot overwrite replacement session',async()=>{const c=setup();let resolve;c.invoke=()=>new Promise(r=>resolve=r);const pending=c.refreshAnnotations();c.snapshot={...c.snapshot,request_id:'B'};resolve({request_id:'A',revision:7,strokes:[{id:'old'}]});await pending;assert.equal(c.view.board.strokes.length,0);});
test('cancelled drawing never submits partial stroke',()=>{const c=setup();c.bindSessionViewer();c.view.mode='draw';const screen=c.$('#remote-screen');screen.events.pointerdown(event());screen.events.pointercancel(event());screen.events.pointerup(event());assert.equal(c.calls.filter(x=>x.command==='session_annotate').length,0);assert.equal(c.inputs.length,0);});

test('precision trackpad accumulates fractional deltas symmetrically and cancels on blur',()=>{const c=setup();c.bindSessionViewer();const screen=c.$('#remote-screen');for(let i=0;i<20;i++)screen.events.wheel(event({deltaX:0,deltaY:5,deltaMode:0}));assert.equal(c.inputs.filter(x=>x.type==='wheel').reduce((n,x)=>n+x.y,0),2);screen.events.blur();c.inputs=[];for(let i=0;i<20;i++)screen.events.wheel(event({deltaX:0,deltaY:-5,deltaMode:0}));assert.equal(c.inputs.filter(x=>x.type==='wheel').reduce((n,x)=>n+x.y,0),-2);});

test('printable Unicode enters as text while control shortcuts release keys even after modifier release',()=>{const c=setup();c.bindSessionViewer();const screen=c.$('#remote-screen');screen.events.keydown(event({key:'я'}));assert.equal(c.inputs.at(-1).type,'text');assert.equal(c.inputs.at(-1).text,'я');screen.events.keydown(event({key:'a',ctrlKey:true}));screen.events.keyup(event({key:'a',ctrlKey:false}));assert.equal(c.inputs.at(-1).type,'key');assert.equal(c.inputs.at(-1).down,false);});
test('IME commit emits once and its followup beforeinput does not duplicate text',()=>{const c=setup();c.bindSessionViewer();const screen=c.$('#remote-screen'),proxy=c.$('#remote-input-proxy');screen.events.compositionstart(event());screen.events.keydown(event({key:'a',isComposing:true}));screen.events.compositionend(event({data:'你好'}));proxy.events.beforeinput(event({inputType:'insertFromComposition',data:'你好'}));assert.equal(c.inputs.length,1);assert.equal(c.inputs[0].text,'你好');});
test('input is paused while monitor switch is awaiting the displayed frame',()=>{const c=setup();c.bindSessionViewer();c.snapshot.screen_pending=true;const screen=c.$('#remote-screen');screen.events.keydown(event({key:'a'}));screen.events.pointerdown(event());assert.equal(c.inputs.length,0);});
