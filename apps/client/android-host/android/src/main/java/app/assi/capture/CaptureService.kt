package app.assi.capture

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.content.res.Configuration
import android.graphics.Bitmap
import android.graphics.PixelFormat
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.ImageReader
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.SystemClock
import android.util.Base64
import android.util.DisplayMetrics
import android.view.WindowManager
import java.io.ByteArrayOutputStream
import kotlin.math.max
import kotlin.math.roundToInt

class CaptureService : Service() {
    companion object {
        const val REQUEST_ID = "requestId"
        const val EPOCH = "epoch"
        const val RESULT_CODE = "resultCode"
        const val RESULT_DATA = "resultData"
        private const val ACTION_STOP = "app.assi.capture.STOP"
        private const val CHANNEL = "assi-screen-sharing"
        private const val NOTIFICATION = 18482
        @Volatile private var instance: CaptureService? = null
        internal fun stop(expected: SessionGate.Lease, done: () -> Unit) {
            val service = instance
            if (service == null) done() else service.stopScoped(expected, done)
        }
    }

    private lateinit var thread: HandlerThread
    private lateinit var worker: Handler
    @Volatile private var lease: SessionGate.Lease? = null
    @Volatile private var requestedLease: SessionGate.Lease? = null
    private var projection: MediaProjection? = null
    private var projectionCallback: MediaProjection.Callback? = null
    private var display: VirtualDisplay? = null
    private var reader: ImageReader? = null
    private var frameWidth = 0
    private var frameHeight = 0
    private var lastEncodedAt = 0L
    private var hasFrame = false
    @Volatile private var destroyed = false
    @Volatile private var lastStartId = 0

    override fun onCreate() {
        super.onCreate()
        thread = HandlerThread("AssiCapture").also { it.start() }
        worker = Handler(thread.looper)
        instance = this
    }
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent == null) { stopIfUnowned(startId); return START_NOT_STICKY }
        val id = intent.getStringExtra(REQUEST_ID)
        val epoch = intent.getLongExtra(EPOCH, -1)
        if (id == null || epoch < 0) { stopIfUnowned(startId); return START_NOT_STICKY }
        val incoming = SessionGate.Lease(id, epoch)
        if (intent.action == ACTION_STOP) {
            if (lease == incoming) {
                CaptureStore.cancel(id)
                stopScoped(incoming) {}
            }
            return START_NOT_STICKY
        }
        if (!CaptureStore.current(incoming)) { stopIfUnowned(startId); return START_NOT_STICKY }
        requestedLease = incoming
        // Foreground notification is mandatory BEFORE getMediaProjection on Android 14+.
        try { showNotification(incoming) } catch (_: Exception) {
            CaptureStore.fail(incoming, "Не удалось показать уведомление о записи экрана")
            stopSelf(startId)
            return START_NOT_STICKY
        }
        lastStartId = startId
        @Suppress("DEPRECATION")
        val consent = if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(RESULT_DATA, Intent::class.java) else intent.getParcelableExtra<Intent>(RESULT_DATA)
        val code = intent.getIntExtra(RESULT_CODE, 0)
        if (consent == null || code != android.app.Activity.RESULT_OK) {
            CaptureStore.fail(incoming, "Нет системного разрешения на показ экрана")
            stopSelf(startId)
            return START_NOT_STICKY
        }
        worker.post {
            if (destroyed || !CaptureStore.current(incoming)) { stopIfUnowned(startId); return@post }
            releaseResources()
            lease = incoming
            try {
                val manager = getSystemService(Context.MEDIA_PROJECTION_SERVICE) as MediaProjectionManager
                val acquired = manager.getMediaProjection(code, consent)
                    ?: throw IllegalStateException("No projection")
                projection = acquired
                val callback = object : MediaProjection.Callback() {
                    override fun onStop() {
                        if (lease != incoming) return
                        CaptureStore.fail(incoming, "Android остановил показ экрана. Подтвердите новый запрос для продолжения")
                        releaseResources()
                        stopSelf(startId)
                    }
                    override fun onCapturedContentResize(width: Int, height: Int) {
                        if (lease == incoming && CaptureStore.current(incoming)) {
                            try { resize(width, height, incoming) } catch (_: Exception) { captureFailed(incoming) }
                        }
                    }
                }
                projectionCallback = callback
                acquired.registerCallback(callback, worker)
                val size = screenSize()
                val target = scaledSize(size.first, size.second)
                val surface = newReader(target.first, target.second, incoming)
                display = acquired.createVirtualDisplay(
                    "Асси", target.first, target.second, resources.displayMetrics.densityDpi,
                    DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, surface.surface, null, worker
                )
                // One getMediaProjection and one createVirtualDisplay per consent token.
                CaptureStore.started(incoming)
                worker.postDelayed({
                    if (lease == incoming && !hasFrame) captureFailed(incoming)
                }, 15_000)
            } catch (_: Exception) { captureFailed(incoming) }
        }
        return START_NOT_STICKY
    }

    private fun newReader(width: Int, height: Int, expected: SessionGate.Lease): ImageReader {
        val next = ImageReader.newInstance(width, height, PixelFormat.RGBA_8888, 2)
        next.setOnImageAvailableListener({ source ->
            val image = try { source.acquireLatestImage() } catch (_: Exception) { null } ?: return@setOnImageAvailableListener
            try {
                if (destroyed || lease != expected || !CaptureStore.current(expected)) return@setOnImageAvailableListener
                val now = SystemClock.elapsedRealtime()
                if (now - lastEncodedAt < 160) return@setOnImageAvailableListener
                lastEncodedAt = now
                val plane = image.planes.firstOrNull() ?: return@setOnImageAvailableListener
                val stride = plane.pixelStride
                if (stride != 4 || image.width <= 0 || image.height <= 0 || image.width > 1920 || image.height > 1920) throw IllegalStateException("Invalid frame")
                val paddedWidth = plane.rowStride / stride
                if (paddedWidth < image.width || paddedWidth > 4096) throw IllegalStateException("Invalid stride")
                val padded = Bitmap.createBitmap(paddedWidth, image.height, Bitmap.Config.ARGB_8888)
                var cropped: Bitmap? = null
                try {
                    padded.copyPixelsFromBuffer(plane.buffer)
                    val bitmap = if (paddedWidth == image.width) padded else Bitmap.createBitmap(padded, 0, 0, image.width, image.height).also { cropped = it }
                    val bytes = ByteArrayOutputStream()
                    if (!bitmap.compress(Bitmap.CompressFormat.JPEG, 70, bytes)) throw IllegalStateException("JPEG failed")
                    if (bytes.size() > 4 * 1024 * 1024) throw IllegalStateException("Frame too large")
                    CaptureStore.publish(expected, Base64.encodeToString(bytes.toByteArray(), Base64.NO_WRAP))
                    hasFrame = true
                } finally { cropped?.recycle(); padded.recycle() }
            } catch (_: Exception) { captureFailed(expected) } finally { image.close() }
        }, worker)
        reader = next
        frameWidth = width
        frameHeight = height
        return next
    }

    private fun resize(width: Int, height: Int, expected: SessionGate.Lease) {
        val target = scaledSize(width, height)
        if (target.first == frameWidth && target.second == frameHeight) return
        val currentDisplay = display ?: return
        val old = reader
        currentDisplay.surface = null
        val next = newReader(target.first, target.second, expected)
        currentDisplay.resize(target.first, target.second, resources.displayMetrics.densityDpi)
        currentDisplay.surface = next.surface
        old?.setOnImageAvailableListener(null, null)
        old?.close()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        // Android 14's callback also handles partial-app capture. Older versions
        // have only full-screen sharing, so configuration dimensions are used.
        if (Build.VERSION.SDK_INT < 34) worker.post {
            val expected = lease ?: return@post
            if (!CaptureStore.current(expected)) return@post
            try { val size = screenSize(); resize(size.first, size.second, expected) } catch (_: Exception) { captureFailed(expected) }
        }
    }

    @Suppress("DEPRECATION")
    private fun screenSize(): Pair<Int, Int> {
        val manager = getSystemService(Context.WINDOW_SERVICE) as WindowManager
        if (Build.VERSION.SDK_INT >= 30) {
            val bounds = manager.maximumWindowMetrics.bounds
            return Pair(bounds.width(), bounds.height())
        }
        val metrics = DisplayMetrics()
        manager.defaultDisplay.getRealMetrics(metrics)
        return Pair(metrics.widthPixels, metrics.heightPixels)
    }
    private fun scaledSize(width: Int, height: Int): Pair<Int, Int> {
        if (width <= 0 || height <= 0 || width > 32768 || height > 32768) throw IllegalArgumentException("Invalid display size")
        val scale = minOf(1.0, 1920.0 / max(width, height))
        return Pair(max(1, (width * scale).roundToInt()), max(1, (height * scale).roundToInt()))
    }

    private fun captureFailed(expected: SessionGate.Lease) {
        if (lease != expected) return
        CaptureStore.fail(expected, "Не удалось получить экран Android. Подтвердите новый запрос и повторите подключение")
        releaseResources()
        if (requestedLease == expected) { requestedLease = null; stopSelf(lastStartId) }
    }
    private fun stopIfUnowned(startId: Int) {
        if (requestedLease?.let { CaptureStore.current(it) } != true) stopSelf(startId)
    }
    private fun stopScoped(expected: SessionGate.Lease, done: () -> Unit) {
        if (destroyed) { done(); return }
        val posted = worker.post {
            if (lease == expected) {
                releaseResources()
                if (requestedLease == expected) { requestedLease = null; stopSelf(lastStartId) }
            }
            done()
        }
        if (!posted) done()
    }
    private fun releaseResources() {
        lease = null
        display?.release(); display = null
        reader?.setOnImageAvailableListener(null, null)
        reader?.close(); reader = null
        val old = projection
        val callback = projectionCallback
        projection = null
        projectionCallback = null
        if (old != null && callback != null) old.unregisterCallback(callback)
        old?.stop()
        frameWidth = 0; frameHeight = 0; lastEncodedAt = 0; hasFrame = false
    }
    override fun onDestroy() {
        destroyed = true
        if (instance === this) instance = null
        worker.post {
            lease?.let { CaptureStore.fail(it, "Системный модуль показа экрана остановлен") }
            releaseResources()
            thread.quitSafely()
        }
        stopForeground(STOP_FOREGROUND_REMOVE)
        super.onDestroy()
    }

    private fun showNotification(expected: SessionGate.Lease) {
        val notifications = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= 26) notifications.createNotificationChannel(NotificationChannel(CHANNEL, "Показ экрана", NotificationManager.IMPORTANCE_LOW))
        val stop = Intent(this, CaptureService::class.java).setAction(ACTION_STOP)
            .putExtra(REQUEST_ID, expected.requestId).putExtra(EPOCH, expected.epoch)
        val action = PendingIntent.getService(this, expected.epoch.toInt(), stop, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        @Suppress("DEPRECATION")
        val builder = if (Build.VERSION.SDK_INT >= 26) Notification.Builder(this, CHANNEL) else Notification.Builder(this)
        val notification = builder.setSmallIcon(android.R.drawable.ic_menu_view).setContentTitle("Асси — показ экрана")
            .setContentText("Показывается выбранный экран. Нажмите «Остановить», чтобы закрыть доступ.")
            .setOngoing(true).setCategory(Notification.CATEGORY_SERVICE)
            .addAction(Notification.Action.Builder(null, "Остановить", action).build()).build()
        if (Build.VERSION.SDK_INT >= 29) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        else startForeground(NOTIFICATION, notification)
    }
}
