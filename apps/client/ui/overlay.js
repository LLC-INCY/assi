'use strict';
// This isolated webview only receives validated, session-scoped board snapshots.
// It has no input, filesystem, clipboard, window-control or session-mutation IPC.
const palette = { mint:'#a3f3d5', amber:'#ffd166', rose:'#ff7e9d', blue:'#78b9ff', white:'#ffffff' };
const svg = document.getElementById('marks');
const ns = 'http://www.w3.org/2000/svg';
let lastSnapshot = null;
let lastKey = '';
let stopped = false;

function clearMarks() { svg.replaceChildren(); lastKey = ''; lastSnapshot = null; }
function element(tag, attributes) {
  const node = document.createElementNS(ns, tag);
  for (const [key, value] of Object.entries(attributes)) node.setAttribute(key, String(value));
  return node;
}
function draw(snapshot) {
  if (!snapshot?.request_id || !Array.isArray(snapshot.strokes) || snapshot.strokes.length > 128) {
    clearMarks(); return;
  }
  const width = Math.max(1, window.innerWidth), height = Math.max(1, window.innerHeight);
  const key = `${snapshot.request_id}:${snapshot.revision}:${width}:${height}`;
  if (key === lastKey) return;
  lastKey = key; lastSnapshot = snapshot;
  const fragment = document.createDocumentFragment();
  for (const stroke of snapshot.strokes) {
    if (!palette[stroke.color] || !['pen','arrow','rectangle'].includes(stroke.tool) ||
        !Number.isInteger(stroke.width) || stroke.width < 1 || stroke.width > 12 ||
        !Array.isArray(stroke.points) || !stroke.points.length || stroke.points.length > 128 ||
        stroke.points.some(p => !Number.isFinite(p.x) || !Number.isFinite(p.y) || p.x < 0 || p.x > 1 || p.y < 0 || p.y > 1)) continue;
    const points = stroke.points.map(p => ({x:p.x * width, y:p.y * height}));
    const first = points[0], last = points.at(-1);
    const lineWidth = stroke.width * Math.min(width, height) / 720;
    const attributes = {fill:'none', stroke:palette[stroke.color], 'stroke-width':lineWidth, 'stroke-linecap':'round', 'stroke-linejoin':'round'};
    if (stroke.tool === 'rectangle') {
      fragment.append(element('rect', {...attributes, x:Math.min(first.x,last.x), y:Math.min(first.y,last.y), width:Math.abs(last.x-first.x), height:Math.abs(last.y-first.y)}));
    } else if (stroke.tool === 'arrow') {
      const angle = Math.atan2(last.y-first.y, last.x-first.x), head = Math.max(lineWidth*4, Math.min(width,height)*.018);
      const a = {x:last.x-head*Math.cos(angle-.5), y:last.y-head*Math.sin(angle-.5)};
      const b = {x:last.x-head*Math.cos(angle+.5), y:last.y-head*Math.sin(angle+.5)};
      fragment.append(element('path', {...attributes, d:`M ${first.x} ${first.y} L ${last.x} ${last.y} M ${a.x} ${a.y} L ${last.x} ${last.y} L ${b.x} ${b.y}`}));
    } else if (points.length === 1) {
      fragment.append(element('circle', {cx:first.x, cy:first.y, r:lineWidth/2, fill:palette[stroke.color]}));
    } else {
      fragment.append(element('polyline', {...attributes, points:points.map(p=>`${p.x},${p.y}`).join(' ')}));
    }
  }
  svg.replaceChildren(fragment);
}
async function poll() {
  if (stopped) return;
  let expired = false;
  const timeout = setTimeout(() => { expired = true; clearMarks(); }, 1500);
  try {
    // At most one invocation in flight, including a stalled native runtime.
    const snapshot = await window.__TAURI__.core.invoke('session_overlay_snapshot');
    if (!stopped && !expired) draw(snapshot);
  } catch { clearMarks(); }
  finally { clearTimeout(timeout); if (!stopped) setTimeout(poll, 200); }
}
window.addEventListener('resize', () => { if (lastSnapshot) draw(lastSnapshot); });
window.addEventListener('pagehide', () => { stopped = true; clearMarks(); });
poll();
