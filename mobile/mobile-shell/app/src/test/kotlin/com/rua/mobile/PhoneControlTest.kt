package com.rua.mobile

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneControlTest {
    @Test fun terminateCancelsOnlyCurrentOperationAndNextCallNeedsNoResume() {
        val state = PhoneControlState()
        val operation = state.begin("a", "1")
        assertTrue(runCatching { state.begin("b", "2") }.isFailure)
        state.cancelCurrent()
        assertTrue(operation.cancelled.get())
        assertEquals("cancelled", operation.result.get().getString("state"))
        assertTrue(runCatching { state.begin("b", "2") }.isFailure)
        state.finish(operation)
        assertTrue(runCatching { state.begin("a", "1") }.isFailure)
        assertNotNull(state.begin("b", "new"))
    }

    @Test fun cancelIsOwnerScopedAndSubmittedEffectsAreUnknown() {
        val state = PhoneControlState()
        val operation = state.begin("a", "1")
        state.cancel("b", "1")
        assertFalse(operation.cancelled.get())
        operation.submitted.set(true)
        state.cancel("a", "1")
        assertEquals("outcome_unknown", operation.result.get().getString("state"))
        assertTrue(operation.result.get().getBoolean("submitted"))
    }

    @Test fun cancellationCanArriveBeforeTheOperation() {
        val state = PhoneControlState()
        state.cancel("a", "1")
        assertTrue(runCatching { state.begin("a", "1") }.isFailure)
        assertNotNull(state.begin("b", "1"))
    }

    @Test fun staleCompletionCannotHideOrCancelTheNextOperation() {
        val state = PhoneControlState()
        val first = state.begin("a", "1")
        state.cancelCurrent()
        state.finish(first)
        val next = state.begin("b", "2")
        state.finish(first)
        state.cancel("a", "1")
        assertSame(next, state.current())
        assertFalse(next.cancelled.get())
    }

    @Test fun controlLifecycleNotifiesStartCancelAndFinishWithoutIdleControl() {
        val state = PhoneControlState()
        val visible = mutableListOf<Boolean>()
        state.onOperationChanged = { visible += state.current()?.let { !it.cancelled.get() && !it.result.isDone } == true }
        assertNull(state.current())
        val first = state.begin("a", "1")
        state.cancelCurrent()
        state.finish(first)
        val second = state.begin("a", "2")
        state.finish(second)
        assertEquals(listOf(true, false, false, true, false), visible)
    }

    @Test fun cancelledOperationCannotDispatchEvenWhenAlreadyPrepared() {
        val state=PhoneControlState()
        val operation=state.begin("a","prepared")
        state.cancel("a","prepared")
        var sent=false
        assertTrue(runCatching { state.dispatch(operation) { sent=true } }.isFailure)
        assertFalse(sent)
        assertFalse(operation.submitted.get())
    }
}
