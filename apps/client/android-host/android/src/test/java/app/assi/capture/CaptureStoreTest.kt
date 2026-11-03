package app.assi.capture

import org.junit.Assert.*
import org.junit.Before
import org.junit.Test

class CaptureStoreTest {
    @Before fun reset() { CaptureStore.cancel(null) }
    @Test fun lateFrameAndFailureCannotChangeReplacement() {
        val old = CaptureStore.begin("old")!!
        CaptureStore.cancel("old")
        val current = CaptureStore.begin("new")!!
        CaptureStore.publish(current, "new-frame")
        CaptureStore.publish(old, "stale-frame")
        CaptureStore.fail(old, "old error")
        assertEquals("new-frame", CaptureStore.frame("new"))
        assertNull(CaptureStore.cancel("old"))
        assertEquals("new-frame", CaptureStore.frame("new"))
    }
    @Test fun cancellationResolvesStartupOnceAndDiscardsFrames() {
        val lease = CaptureStore.begin("request")!!
        var callbacks = 0
        var message: String? = null
        CaptureStore.onReady(lease) { callbacks++; message = it }
        CaptureStore.publish(lease, "frame")
        CaptureStore.cancel("request")
        CaptureStore.started(lease)
        CaptureStore.fail(lease, "late")
        assertEquals(1, callbacks)
        assertNotNull(message)
        assertThrows(IllegalStateException::class.java) { CaptureStore.frame("request") }
    }
    @Test fun systemRevocationMakesFrameReadsFailAndCannotBeOverwrittenByQueuedImage() {
        val lease = CaptureStore.begin("request")!!
        CaptureStore.publish(lease, "frame")
        CaptureStore.fail(lease, "permission revoked")
        CaptureStore.publish(lease, "late")
        val error = assertThrows(IllegalStateException::class.java) { CaptureStore.frame("request") }
        assertEquals("permission revoked", error.message)
        CaptureStore.cancel("request")
        assertNotNull(CaptureStore.begin("replacement"))
    }
    @Test fun onlyLatestFrameIsRetained() {
        val lease = CaptureStore.begin("request")!!
        repeat(500) { CaptureStore.publish(lease, "frame-$it") }
        assertEquals("frame-499", CaptureStore.frame("request"))
    }
}
