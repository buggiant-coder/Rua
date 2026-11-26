package com.rua.mobile

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneControlTest {
    private fun invocation(owner: String) = PhoneInvocation(
        owner, PhoneChatAddress("chat"), "turn", "authority",
    )

    @Test fun operationCancellationKeepsTheReservationUntilFinish() {
        val state = PhoneControlState { true }
        val operation = state.begin(invocation("a"), "1")
        assertTrue(runCatching { state.begin(invocation("b"), "2") }.isFailure)
        state.cancelCurrent()
        assertTrue(operation.cancelled.get())
        assertEquals("cancelled", operation.result.get().getString("state"))
        assertTrue(runCatching { state.begin(invocation("b"), "2") }.isFailure)
        state.finish(operation)
        assertTrue(runCatching { state.begin(invocation("a"), "1") }.isFailure)
        assertNotNull(state.begin(invocation("b"), "new"))
    }

    @Test fun cancelIsOwnerScopedAndSubmittedEffectsAreUnknown() {
        val state = PhoneControlState { true }
        val operation = state.begin(invocation("a"), "1")
        state.cancel(invocation("b"), "1")
        assertFalse(operation.cancelled.get())
        operation.submitted.set(true)
        state.cancel(invocation("a"), "1")
        assertEquals("outcome_unknown", operation.result.get().getString("state"))
        assertTrue(operation.result.get().getBoolean("submitted"))
    }

    @Test fun cancellationCanArriveBeforeTheOperation() {
        val state = PhoneControlState { true }
        state.cancel(invocation("a"), "1")
        assertTrue(runCatching { state.begin(invocation("a"), "1") }.isFailure)
        assertNotNull(state.begin(invocation("b"), "1"))
    }

    @Test fun staleCompletionCannotHideOrCancelTheNextOperation() {
        val state = PhoneControlState { true }
        val first = state.begin(invocation("a"), "1")
        state.cancelCurrent()
        state.finish(first)
        val next = state.begin(invocation("b"), "2")
        state.finish(first)
        state.cancel(invocation("a"), "1")
        assertSame(next, state.current())
        assertFalse(next.cancelled.get())
    }

    @Test fun operationLifecycleNotifiesStartCancelAndFinish() {
        val state = PhoneControlState { true }
        val executing = mutableListOf<Boolean>()
        state.onOperationChanged = { executing += state.current()?.let { !it.cancelled.get() && !it.result.isDone } == true }
        assertNull(state.current())
        val first = state.begin(invocation("a"), "1")
        state.cancelCurrent()
        state.finish(first)
        val second = state.begin(invocation("a"), "2")
        state.finish(second)
        assertEquals(listOf(true, false, false, true, false), executing)
    }

    @Test fun cancelledOperationCannotDispatchEvenWhenAlreadyPrepared() {
        val state=PhoneControlState { true }
        val operation=state.begin(invocation("a"),"prepared")
        state.cancel(invocation("a"),"prepared")
        var sent=false
        assertTrue(runCatching { state.dispatch(operation) { sent=true } }.isFailure)
        assertFalse(sent)
        assertFalse(operation.submitted.get())
    }

    @Test fun aPreparedOperationCannotDispatchAfterItsSessionWasStopped() {
        var current = true
        val state = PhoneControlState { current }
        val operation = state.begin(invocation("a"), "prepared")
        current = false
        var sent = false
        assertTrue(runCatching { state.dispatch(operation) { sent = true } }.isFailure)
        assertFalse(sent)
        assertFalse(operation.submitted.get())
    }

    @Test fun requestIdsCanBeReusedByANewTurnWithoutReplayingOldCancellation() {
        val state = PhoneControlState { true }
        val first = invocation("a")
        val nextTurn = first.copy(turnId = "next-turn")
        val firstOperation = state.begin(first, "call0")
        state.cancel(first, "call0")
        state.finish(firstOperation)
        val nextOperation = state.begin(nextTurn, "call0")
        assertFalse(nextOperation.cancelled.get())
        state.cancel(first, "call0")
        assertSame(nextOperation, state.current())
        assertFalse(nextOperation.cancelled.get())
        assertFalse(nextOperation.result.isDone)
    }
}
