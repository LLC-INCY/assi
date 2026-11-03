import test from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./ci-artifacts.mjs', import.meta.url));
async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'assi-artifacts-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'input'), { recursive: true });
  await writeFile(join(root, 'package.json'), JSON.stringify({ version: '0.6.0' }));
  return root;
}
function collect(root, platform) {
  return spawnSync(process.execPath, [script, 'input', platform], { cwd: root, encoding: 'utf8' });
}
test('APK artifact has usable name, exact checksum and explicit debug signing metadata', async t => {
  const root = await fixture(t);
  const data = Buffer.from('synthetic APK content');
  await writeFile(join(root, 'input/app-universal-debug.apk'), data);
  assert.equal(collect(root, 'android-arm64-debug').status, 0);
  assert.deepEqual(await readFile(join(root, 'ci-artifacts/assi-android-arm64-debug.apk')), data);
  assert.equal(await readFile(join(root, 'ci-artifacts/SHA256SUMS'), 'utf8'), `${createHash('sha256').update(data).digest('hex')}  assi-android-arm64-debug.apk\n`);
  const info = JSON.parse(await readFile(join(root, 'ci-artifacts/build-info.json'), 'utf8'));
  assert.equal(info.profile, 'debug');
  assert.equal(info.signing, 'ephemeral debug key');
});
test('Linux collection fails if one requested installer format is missing', async t => {
  const root = await fixture(t);
  await writeFile(join(root, 'input/assi.deb'), 'synthetic package');
  const result = collect(root, 'linux-x64');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Missing linux-x64 .AppImage package/);
});
test('duplicate installer names cannot silently overwrite each other', async t => {
  const root = await fixture(t);
  await mkdir(join(root, 'input/second'));
  await writeFile(join(root, 'input/setup.exe'), 'one');
  await writeFile(join(root, 'input/second/setup.exe'), 'two');
  const result = collect(root, 'windows-x64');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Ambiguous artifact name: setup.exe/);
});
