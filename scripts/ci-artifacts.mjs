import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { basename, extname, resolve } from 'node:path';

const [source, platform] = process.argv.slice(2);
const extensions = {
  'windows-x64': ['.exe'],
  'linux-x64': ['.deb', '.AppImage'],
  'macos-arm64': ['.zip'],
  'macos-x64': ['.zip'],
  'android-arm64-debug': ['.apk'],
}[platform];
if (!source || !extensions) throw new Error('Expected artifact directory and supported platform');
const root = resolve(source);
const files = (await readdir(root, { recursive: true, withFileTypes: true }))
  .filter(item => item.isFile() && extensions.includes(extname(item.name)))
  .map(item => resolve(item.parentPath, item.name)).sort();
for (const extension of extensions) {
  if (!files.some(file => extname(file) === extension)) throw new Error(`Missing ${platform} ${extension} package`);
}
const output = resolve('ci-artifacts');
await mkdir(output, { recursive: true });
const names = new Set();
const hashes = [];
for (const file of files) {
  const name = platform === 'android-arm64-debug' ? 'assi-android-arm64-debug.apk' : basename(file);
  if (names.has(name)) throw new Error(`Ambiguous artifact name: ${name}`);
  names.add(name);
  await copyFile(file, resolve(output, name));
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  hashes.push(`${hash.digest('hex')}  ${name}`);
}
await writeFile(resolve(output, 'SHA256SUMS'), hashes.join('\n') + '\n');
const { version } = JSON.parse(await readFile('package.json', 'utf8'));
await writeFile(resolve(output, 'build-info.json'), JSON.stringify({
  version, platform, commit: process.env.GITHUB_SHA ?? null,
  profile: platform === 'android-arm64-debug' ? 'debug' : 'release',
  signing: platform === 'android-arm64-debug' ? 'ephemeral debug key' : 'no distribution certificate',
}, null, 2) + '\n');
console.log(`Collected ${files.length} ${platform} package(s) with SHA-256 checksums.`);
