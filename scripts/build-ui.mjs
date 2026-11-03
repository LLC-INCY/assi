import { mkdir, copyFile, cp, readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
const root = fileURLToPath(new URL('../', import.meta.url));
const out = `${root}apps/client/dist`;
await mkdir(out, { recursive: true });
for (const name of ['index.html', 'app.js', 'native.css', 'dialog-tasks.js', 'session-ui.js', 'files-ui.js', 'session-view.js', 'session-audio.js', 'session-video.js', 'session-tools.js', 'session-access.js', 'session-recording.js', 'session-windows.js', 'overlay.html', 'overlay.js', 'overlay.css']) {
  await copyFile(`${root}apps/client/ui/${name}`, `${out}/${name}`);
}
await copyFile(`${root}prototype/styles.css`, `${out}/styles.css`);
await cp(`${root}prototype/assets`, `${out}/assets`, { recursive: true });
// Keep the approved icon vocabulary in one source during the first implementation stage.
const prototype = await readFile(`${root}prototype/app.js`, 'utf8');
const icons = prototype.slice(prototype.indexOf('const paths ='), prototype.indexOf('const devices ='));
if (!icons.includes('function hydrate')) throw new Error('Prototype icon source changed; update the UI build.');
await writeFile(`${out}/icons.js`, icons);
console.log('Built apps/client/dist from native UI + approved design assets.');
