package app.assi.capture

/** Pure lifecycle gate. The permission Intent is never persisted or reusable. */
internal class SessionGate {
    data class Lease(val requestId: String, val epoch: Long)
    private var epoch = 0L
    var current: Lease? = null
        private set
    @Synchronized fun begin(requestId: String): Lease? {
        if (current != null || requestId.isBlank() || requestId.length > 128) return null
        return Lease(requestId, ++epoch).also { current = it }
    }
    @Synchronized fun isCurrent(lease: Lease): Boolean = current == lease
    @Synchronized fun stop(requestId: String? = null): Lease? {
        val old = current ?: return null
        if (requestId != null && requestId != old.requestId) return null
        current = null
        ++epoch
        return old
    }
}
