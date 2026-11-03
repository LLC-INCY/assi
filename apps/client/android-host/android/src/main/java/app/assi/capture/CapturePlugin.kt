package app.assi.capture

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.activity.result.ActivityResult
import androidx.appcompat.app.AppCompatActivity
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class CaptureArgs { var requestId: String? = null }

@TauriPlugin
class CapturePlugin(private val activity: Activity) : Plugin(activity) {
    private data class Pending(val lease: SessionGate.Lease, val invoke: Invoke, var resolved: Boolean = false)
    private var pending: Pending? = null
    private val main = Handler(Looper.getMainLooper())

    @Command
    fun requestCapture(invoke: Invoke) {
        // Keep the cancelled OS activity registered until its result arrives;
        // otherwise an old result could be delivered to a replacement request.
        if (pending != null) { invoke.reject("Сначала закройте предыдущий системный запрос показа экрана"); return }
        val id = invoke.parseArgs(CaptureArgs::class.java).requestId
        if (id == null) { invoke.reject("Отсутствует запрос на показ экрана"); return }
        val lease = CaptureStore.begin(id)
        if (lease == null) { invoke.reject("Показ экрана уже запущен или запрос недействителен"); return }
        val record = Pending(lease, invoke)
        pending = record
        try {
            val manager = activity.getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
            startActivityForResult(invoke, manager.createScreenCaptureIntent(), "capturePermissionResult")
            main.postDelayed({
                if (pending === record && !record.resolved) {
                    record.resolved = true
                    CaptureStore.cancel(id)
                    invoke.reject("Истекло время ожидания разрешения на показ экрана")
                }
            }, 55_000)
        } catch (_: Exception) {
            pending = null
            CaptureStore.cancel(id)
            invoke.reject("Не удалось открыть системный запрос показа экрана")
        }
    }

    @ActivityCallback
    private fun capturePermissionResult(invoke: Invoke, result: ActivityResult) {
        val record = pending
        if (record == null || record.invoke.id != invoke.id) return
        pending = null
        if (record.resolved || !CaptureStore.current(record.lease)) return
        if (result.resultCode != Activity.RESULT_OK || result.data == null) {
            record.resolved = true
            CaptureStore.cancel(record.lease.requestId)
            invoke.reject("Разрешение на показ экрана не предоставлено")
            return
        }
        CaptureStore.onReady(record.lease) { error ->
            main.post {
                if (!record.resolved) {
                    record.resolved = true
                    if (error == null) invoke.resolve() else invoke.reject(error)
                }
            }
        }
        val intent = Intent(activity, CaptureService::class.java)
            .putExtra(CaptureService.REQUEST_ID, record.lease.requestId)
            .putExtra(CaptureService.EPOCH, record.lease.epoch)
            .putExtra(CaptureService.RESULT_CODE, result.resultCode)
            .putExtra(CaptureService.RESULT_DATA, result.data)
        try {
            if (Build.VERSION.SDK_INT >= 26) activity.startForegroundService(intent) else activity.startService(intent)
        } catch (_: Exception) {
            CaptureStore.fail(record.lease, "Не удалось запустить показ экрана. Откройте Асси и повторите запрос")
        }
        main.postDelayed({
            if (!record.resolved) {
                CaptureStore.fail(record.lease, "Системный модуль показа экрана не ответил")
                CaptureService.stop(record.lease) {}
            }
        }, 10_000)
    }

    @Command
    fun captureFrame(invoke: Invoke) {
        val id = invoke.parseArgs(CaptureArgs::class.java).requestId
        if (id == null) { invoke.reject("Отсутствует запрос на показ экрана"); return }
        try {
            val frame = CaptureStore.frame(id)
            val result = JSObject()
            result.put("jpeg", frame ?: org.json.JSONObject.NULL)
            invoke.resolve(result)
        } catch (error: IllegalStateException) { invoke.reject(error.message) }
    }

    @Command
    fun stopCapture(invoke: Invoke) {
        val id = invoke.parseArgs(CaptureArgs::class.java).requestId
        val old = CaptureStore.cancel(id)
        pending?.let {
            if ((id == null || id == it.lease.requestId) && !it.resolved) {
                it.resolved = true
                it.invoke.reject("Запрос на показ экрана отменён")
            }
        }
        if (old == null) { invoke.resolve(); return }
        CaptureService.stop(old) { main.post { invoke.resolve() } }
    }

    override fun onDestroy(activity: AppCompatActivity) {
        // Activity recreation is supported by the service; normal destruction
        // revokes sharing, even if the Rust shutdown hook is not reached.
        if (activity.isChangingConfigurations) return
        val old = CaptureStore.cancel(null)
        pending?.let { if (!it.resolved) { it.resolved = true; it.invoke.reject("Приложение закрыто") } }
        pending = null
        old?.let { CaptureService.stop(it) {} }
    }
}
