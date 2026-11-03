import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { prepareAndroid } from './ci-prepare-android.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'assi-gradle-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, 'app'), { recursive: true });
  await mkdir(join(root, 'gradle/wrapper'), { recursive: true });
  await writeFile(join(root, 'build.gradle.kts'), 'classpath("com.android.tools.build:gradle:8.9.1")\nclasspath("org.jetbrains.kotlin:kotlin-gradle-plugin:1.9.25")\n');
  await writeFile(join(root, 'app/build.gradle.kts'), 'compileSdk = 35\ntargetSdk = 35\nminSdk = 24\n');
  await writeFile(join(root, 'gradle/wrapper/gradle-wrapper.properties'), 'distributionBase=GRADLE_USER_HOME\ndistributionUrl=https\\://services.gradle.org/distributions/gradle-8.14-bin.zip\n');
  await writeFile(join(root, 'gradle.properties'), 'org.gradle.jvmargs=-Xmx2048m\nandroid.newDsl=true\n');
  return root;
}

test('clean generated project receives reproducible versions and classic DSL flags idempotently', async t => {
  const root = await fixture(t);
  await prepareAndroid(root);
  const before = await readFile(join(root, 'gradle.properties'), 'utf8');
  assert.match(before, /^org.gradle.jvmargs=-Xmx2048m$/m);
  assert.match(before, /^android.builtInKotlin=false$/m);
  assert.match(before, /^android.newDsl=false$/m);
  assert.match(await readFile(join(root, 'app/build.gradle.kts'), 'utf8'), /compileSdk = 37\ntargetSdk = 37\nminSdk = 24/);
  assert.match(await readFile(join(root, 'gradle/wrapper/gradle-wrapper.properties'), 'utf8'), /distributionSha256Sum=[a-f0-9]{64}/);
  await prepareAndroid(root);
  assert.equal(await readFile(join(root, 'gradle.properties'), 'utf8'), before);
});

test('unexpected generated template fails before writing any file', async t => {
  const root = await fixture(t);
  const original = await readFile(join(root, 'build.gradle.kts'), 'utf8');
  await writeFile(join(root, 'app/build.gradle.kts'), 'changed template');
  await assert.rejects(prepareAndroid(root), /Unexpected Android template: compile SDK/);
  assert.equal(await readFile(join(root, 'build.gradle.kts'), 'utf8'), original);
});
