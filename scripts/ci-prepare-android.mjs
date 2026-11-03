// gen/android is disposable. Keep the tested native toolchain reproducible after init.
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';

function replaceOne(text, pattern, replacement, label) {
  const matches = [...text.matchAll(new RegExp(pattern.source, pattern.flags.replace('g', '') + 'g'))];
  if (matches.length !== 1) throw new Error(`Unexpected Android template: ${label}`);
  return text.replace(pattern, replacement);
}

export async function prepareAndroid(directory) {
  const files = ['build.gradle.kts', 'app/build.gradle.kts', 'gradle/wrapper/gradle-wrapper.properties', 'gradle.properties'];
  const values = await Promise.all(files.map(file => readFile(resolve(directory, file), 'utf8')));
  let [build, app, wrapper, properties] = values;
  build = replaceOne(build, /com\.android\.tools\.build:gradle:[^"\s]+/, 'com.android.tools.build:gradle:9.3.1', 'AGP version');
  build = replaceOne(build, /org\.jetbrains\.kotlin:kotlin-gradle-plugin:[^"\s]+/, 'org.jetbrains.kotlin:kotlin-gradle-plugin:2.2.10', 'Kotlin version');
  app = replaceOne(app, /compileSdk\s*=\s*\d+/, 'compileSdk = 37', 'compile SDK');
  app = replaceOne(app, /targetSdk\s*=\s*\d+/, 'targetSdk = 37', 'target SDK');
  wrapper = replaceOne(wrapper, /^distributionUrl=.*$/m, 'distributionUrl=https\\://services.gradle.org/distributions/gradle-9.6.1-bin.zip', 'Gradle distribution');
  wrapper = wrapper.replace(/^distributionSha256Sum=.*\r?\n?/gm, '').trimEnd()
    + '\ndistributionSha256Sum=9c0f7faeeb306cb14e4279a3e084ca6b596894089a0638e68a07c945a32c9e14\n';
  // Tauri's generated Rust plugin and capture plugin use the classic Android DSL.
  properties = properties.replace(/^android\.(?:builtInKotlin|newDsl)=.*\r?\n?/gm, '').trimEnd()
    + '\nandroid.builtInKotlin=false\nandroid.newDsl=false\n';
  // Validate all inputs first; do not leave a half-patched project on template changes.
  for (const [index, value] of [build, app, wrapper, properties].entries()) {
    await writeFile(resolve(directory, files[index]), value);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  await prepareAndroid(resolve('apps/client/src-tauri/gen/android'));
  console.log('Pinned Android SDK 37, AGP 9.3.1, Gradle 9.6.1 and classic Kotlin DSL.');
}
