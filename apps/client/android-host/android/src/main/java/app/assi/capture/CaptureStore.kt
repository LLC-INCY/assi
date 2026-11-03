package app.assi.capture

/** One latest encoded frame, shared by the main-thread bridge and capture worker. */
internal object CaptureStore {
    private val gate = SessionGate()
    private var jpeg: String? = null
    private var error: String? = null
    private var ready: ((String?) -> Unit)? = null

    @Synchronized fun begin(requestId: String): SessionGate.Lease? {
        val lease = gate.begin(requestId) ?: return null
        jpeg = null
        error = null
        return lease
    }
    @Synchronized fun current(lease: SessionGate.Lease): Boolean = gate.isCurrent(lease)
    @Synchronized fun onReady(lease: SessionGate.Lease, callback: (String?) -> Unit) {
        if (!gate.isCurrent(lease)) callback("Запрос на показ экрана отменён") else ready = callback
    }
    @Synchronized fun started(lease: SessionGate.Lease) {
        if (!gate.isCurrent(lease)) return
        val callback = ready
        ready = null
        callback?.invoke(null)
    }
    @Synchronized fun publish(lease: SessionGate.Lease, frame: String) {
        if (gate.isCurrent(lease) && error == null) jpeg = frame
    }
    @Synchronized fun fail(lease: SessionGate.Lease, message: String) {
        if (!gate.isCurrent(lease)) return
        error = message
        jpeg = null
        val callback = ready
        ready = null
        callback?.invoke(message)
    }
    @Synchronized fun frame(requestId: String): String? {
        if (gate.current?.requestId != requestId) throw IllegalStateException("Сеанс показа экрана уже завершён")
        error?.let { throw IllegalStateException(it) }
        return jpeg
    }
    @Synchronized fun cancel(requestId: String?): SessionGate.Lease? {
        val old = gate.stop(requestId) ?: return null
        jpeg = null
        error = null
        val callback = ready
        ready = null
        callback?.invoke("Показ экрана остановлен")
        return old
    }
}
