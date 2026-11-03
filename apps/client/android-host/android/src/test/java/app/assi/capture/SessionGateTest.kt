package app.assi.capture

import org.junit.Assert.*
import org.junit.Test

class SessionGateTest {
    @Test fun latePermissionCannotStartAfterStop() {
        val gate = SessionGate()
        val first = gate.begin("old")!!
        gate.stop()
        assertFalse(gate.isCurrent(first))
        val next = gate.begin("new")!!
        assertFalse(gate.isCurrent(first))
        assertTrue(gate.isCurrent(next))
    }
    @Test fun scopedOldStopCannotRevokeNewProjection() {
        val gate = SessionGate()
        gate.begin("old")
        gate.stop("old")
        val next = gate.begin("new")!!
        assertNull(gate.stop("old"))
        assertTrue(gate.isCurrent(next))
    }
    @Test fun concurrentPermissionIsRejected() {
        val gate = SessionGate()
        val current = gate.begin("request")!!
        assertNull(gate.begin("other"))
        assertTrue(gate.isCurrent(current))
        assertEquals(current, gate.stop("request"))
        assertNull(gate.stop())
    }
    @Test fun requestIdsAreRequiredAndBounded() {
        val gate = SessionGate()
        assertNull(gate.begin(" "))
        assertNull(gate.begin("a".repeat(129)))
    }
}
