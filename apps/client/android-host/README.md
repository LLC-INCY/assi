# Android screen host

Local Tauri plugin `tauri-plugin-assi-capture` (`assi_capture` Rust crate). Persistent
source is outside `src-tauri/gen`; Tauri's plugin build metadata includes the
Android library and merges its foreground-service manifest. There are **no plugin
commands exposed in the WebView capability**. The application Rust consent
boundary calls the mobile commands via `PluginHandle`.

## Integration

Add the workspace member `apps/client/android-host`, and this client dependency:

```toml
[target.'cfg(target_os = "android")'.dependencies]
assi-capture = { package = "tauri-plugin-assi-capture", path = "../android-host" }
```

Register `.plugin(assi_capture::init())` on Android. Native API:

- `request_capture(&AppHandle, request_id: String) -> Result<(), String>` opens the
  mandatory system MediaProjection consent. Call only after the local owner presses
  the app's per-request Allow button. Never call it on startup, a network request,
  or before that local decision. This returns when the foreground capture service
  starts; the app must recheck the **same still-pending request ID** before accepting
  the network session. Never hold a lifecycle lock across the system dialog.
- `capture_jpeg(&AppHandle, &request_id) -> Result<Option<Vec<u8>>, String>` returns
  the latest JPEG, `None` while awaiting the first image, and an error if the system
  revokes capture. Call only for the active, accepted host session. No frame queue.
- `stop_request(&AppHandle, &request_id)` clears only that request, including a
  pending system dialog, late startup callback, and frame. Use on failed permission,
  expired network request, late result, or failed `engine.accept`.
- `stop(&AppHandle)` revokes all capture for normal session/application shutdown.
  Call even if the session never reached Active. Both calls are async; a pending
  capture callback is invalidated before native resource cleanup is awaited.

Android capabilities: `capture=true`, `control=false`. This plugin never uses an
AccessibilityService and never injects input. The user may choose one app or the
whole display in Android's system dialog. App UI must not claim the selection is
always the complete primary display.

## Lifecycle and bounds

Each consent token is consumed once by `getMediaProjection` and one
`createVirtualDisplay`. Android 14+ content-size changes resize the existing
virtual display and replace its ImageReader surface; older Android full-screen
capture uses configuration size updates. `onStop` clears frames and propagates an
error. Lock-screen, system chip stop, a different projection and service destruction
can therefore terminate the session. The foreground notification has an explicit
Stop action and `mediaProjection` type. Service is not exported or sticky, and
`stopWithTask=true`; there is no boot receiver or unattended restart.

JPEG encoding runs on a HandlerThread, at most about 6 fps, longest edge <=1920,
encoded size <=4 MiB. ImageReader retains at most two native buffers; each acquired
image is closed in `finally`; only the newest encoded frame is held. One base64
string crosses the Kotlin-to-Rust plugin bridge; Rust validates encoded/decoded
sizes, then the ordinary binary media IPC carries the frame to the viewer. This
is an initial software JPEG implementation, not a latency/battery benchmark.

## Verification and limits

Isolated Kotlin compile and pure unit tests (no emulator/capture/permission):

```sh
# From apps/client/src-tauri/gen/android, with JAVA_HOME / ANDROID_HOME set:
./gradlew -I ../../../android-host/validation.init.gradle \
  :assi-capture-validation:compileDebugKotlin \
  :assi-capture-validation:testDebugUnitTest
```

The tests cover cancellation/replacement, late permission leases, stale frame/error
publication, revoke behavior and latest-frame retention. Rust compiles for macOS
(stub registration) and `aarch64-linux-android`; full APK validation is performed by
the app integration owner. Actual MediaProjection, vendor OS background limits,
rotation, selecting individual apps, notification stop, device lock and app quit
require physical-device testing. No such runtime check is claimed here.

Primary references checked 2026-11-03:
- [Android MediaProjection](https://developer.android.com/media/grow/media-projection)
- [Foreground-service types](https://developer.android.com/develop/background-work/services/fg-service-types#media-projection)
- [Tauri mobile plugins](https://v2.tauri.app/develop/plugins/develop-mobile/)
- Installed Tauri 2.12.5 `Plugin.kt`, `PluginHandle.kt` and Rust mobile-plugin API.
