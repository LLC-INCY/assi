# Разработка

Из корня проекта:

```sh
npm ci
npm run ui:build
npm run test:ui
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check
npm run dev
```

Rust 1.94+, Node.js/npm и системные зависимости Tauri 2 обязательны. Rust — для runtime, Node — для подготовки ресурсов и запуска CLI. Во время `npm run dev` ресурсы берутся из `dist`; после изменений UI перезапустите команду, чтобы собрать их заново.

## Desktop

```sh
npm run build
```

Для отладочной сборки на macOS без DMG:

```sh
npm run ui:build
cd apps/client
../../node_modules/.bin/tauri build --debug --bundles app
```

Результат: `target/debug/bundle/macos/Асси.app`. Это локальная отладочная сборка, не подписанный для распространения релиз. Windows собирается на Windows с MSVC/WebView2, Linux — с WebKitGTK 4.1 и остальными зависимостями Tauri. [Требования по ОС](https://v2.tauri.app/start/prerequisites/).

## Android

Нужны Android Studio, JDK, Android SDK/NDK; выставите JAVA_HOME, ANDROID_HOME и NDK_HOME для своей установки. CLI инициализирует проект и устанавливает Rust targets:

```sh
npm run android:init
npm run android:build
```

`apps/client/src-tauri/gen/` — генерируемый каталог, не источник доменной логики. Не хранить в нём единственную копию будущих нативных адаптеров. Повторная генерация не требует копирования личных ключей; подписанный release потребует отдельной настройки.

`android:init` также запускает `scripts/ci-prepare-android.mjs`: фиксирует SDK 37,
AGP 9.3.1, Gradle 9.6.1, Kotlin 2.2.10 и совместимые параметры DSL.
В CI используется JDK 17 и NDK 27.2.12479018. Установите `platforms;android-37.0`
и `build-tools;36.0.0` через SDK Manager.

## GitHub Actions

[Build](https://github.com/LLC-INCY/assi/actions/workflows/build.yml) запускается
на push в `main`, pull request и вручную. Отдельная job проверяет форматирование,
Clippy и тесты; сборки идут независимо для Windows x64 (NSIS), Linux x64
(DEB/AppImage), macOS arm64/x64 (ZIP с `.app`) и Android arm64 (debug APK).

Артефакты доступны 14 дней, содержат `SHA256SUMS` и `build-info.json` с commit SHA.
Desktop пакеты не имеют сертификата распространения, macOS не notarized.
Android подписывается временным debug-ключом runner: обновление поверх APK из
другого запуска может не установиться из-за смены ключа. Production signing
потребует отдельного постоянного ключа в защищённых настройках CI.

Workflow не публикует релизы автоматически и не использует секреты.

## Проверка вручную

1. Открыть клиент: показано текущее устройство; вымышленных компьютеров нет, приём доступа выключен.
2. Добавить тестовую запись с именем и кодом из 9 цифр. Повтор того же кода должен отклоняться. Пробелы в коде допускаются, буквы — нет.
3. Перезапустить клиент и убедиться, что запись осталась. Поиск и группы должны работать.
4. Поменять имя текущего устройства и тему; перезапустить, проверить сохранение.
5. Проверить отмену и подтверждение удаления записи; перепроверить после перезапуска.
6. Сузить окно до мобильного размера: навигация снизу, устройства доступны списком.
7. Без явного запуска приёма и подтверждения входящего запроса экран не захватывается.

Прямой удалённый сценарий: [инструкция и ограничения](docs/REMOTE-SESSIONS.md). Проверяйте на тестовых устройствах с разрешениями ОС. Автотесты используют только синтетические кадры и не выполняют системный ввод.

Инициализацию Android запускать через `npm run android:init`, а сборку через `npm run android:build`: Gradle генерирует обратный вызов CLI, для него необходим npm-скрипт `tauri` в apps/client/package.json. Прямой запуск бинарника .bin при init в этой структуре даёт неверный путь обратного вызова.

## Relay и Android host

`cargo test -p assi-relay -p assi-session` проверяет реальные локальные TLS-туннели
и синтетические файлы. `cargo build --release -p assi-relay` создаёт CLI сервера
на текущей ОС; публичное развёртывание не выполняется автоматически.

Android plugin хранится в `apps/client/android-host`, автоматически подключается
Tauri по Cargo build metadata и объединяет manifest foreground service.
Не переносить его единственную копию в `gen/`. Команда локальных JVM-тестов
описана в `apps/client/android-host/README.md`; они не запускают MediaProjection.

## Дополнения 0.4.0

macOS требует 13+ для ScreenCaptureKit. Сборке нужен Xcode 26+ с актуальным macOS SDK из-за Swift/Metal-зависимостей; CI выбирает Xcode 26.3. Версия SDK сборки отличается от минимальной версии ОС приложения. Клиент добавляет /usr/lib/swift в LC_RPATH и описание разрешения записи экрана/системного звука в Info.plist. Микрофон не запрашивается. Windows backend аудио проверяется отдельно: `cargo check -p assi-audio --target x86_64-pc-windows-gnu --locked`; это не полная Windows сборка приложения.

`npm run test:ui` проверяет очереди ввода, режимы обзора/рисования, дробную прокрутку и отзыв аудио. `scripts/responsive-review.mjs` создаёт только тестовый стенд; `scripts/fixtures/screen-fixture.jpg` — синтетический экран без пользовательских данных. Он никогда не включается в native bundle.
