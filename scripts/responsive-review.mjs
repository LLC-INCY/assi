// Development-only viewport harness. Never copied into the application bundle.
import {cp,readFile,writeFile,mkdir} from 'node:fs/promises';
import {fileURLToPath} from 'node:url';
const root=fileURLToPath(new URL('../',import.meta.url));
const out=`${root}.validation/responsive`;
await mkdir(out,{recursive:true});
await cp(`${root}apps/client/dist`,`${out}/client`,{recursive:true});
await cp(`${root}scripts/fixtures/screen-fixture.jpg`,`${out}/client/screen-fixture.jpg`);
let html=await readFile(`${out}/client/index.html`,'utf8');
html=html.replace('<script src="icons.js"','<script src="fixture.js"></script><script src="icons.js"');
await writeFile(`${out}/client/index.html`,html);
await cp(`${root}scripts/layout-fixture.js`,`${out}/client/fixture.js`);
const sizes=[[320,480],[360,640],[390,844],[600,400],[700,600],[720,480],[768,1024],[900,500],[1024,600],[1180,860],[1440,900],[1920,1080]];
await writeFile(`${out}/index.html`, `<!doctype html><meta charset="utf-8"><title>Асси — проверка адаптивности</title><style>body{margin:20px;background:#101419;color:white;font:15px system-ui}.case{margin-bottom:36px;width:max-content}iframe{border:1px solid #666;display:block}h1{font-size:22px}</style><h1>Проверка реального UI · тестовые данные</h1>${sizes.map(([w,h])=>`<section class="case"><h2>${w} × ${h}</h2><iframe title="${w}x${h}" width="${w}" height="${h}" src="client/index.html"></iframe></section>`).join('')}`);
console.log(out);
