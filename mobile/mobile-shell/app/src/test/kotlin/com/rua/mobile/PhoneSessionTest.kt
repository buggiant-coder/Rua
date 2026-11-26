package com.rua.mobile

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import java.util.concurrent.Callable
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

@RunWith(RobolectricTestRunner::class)
class PhoneSessionTest {
    private fun invocation(turn: String = "turn-1") = PhoneInvocation(
        "owner", PhoneChatAddress("chat"), turn, "authority",
    )

    @Test fun invocationRequiresTheCurrentNestedProtocolAndEncodesOnlyCanonicalChatPaths() {
        val current = invocation().copy(chatAddress = PhoneChatAddress("chat +/?"))
        assertEquals(current, PhoneInvocation.fromParams(JSONObject().put("invocation", current.toJson())))
        assertEquals("/chats/chat%20%2B%2F%3F", current.chatAddress.apiPath())
        assertEquals("/chats/chat", invocation().chatAddress.apiPath())
        assertEquals(setOf("chat_id"), current.toJson().getJSONObject("chat_address").keys().asSequence().toSet())
        assertTrue(runCatching { PhoneInvocation.fromParams(current.toJson()) }.isFailure)
        for (key in listOf("owner", "turn_id", "authority_epoch")) {
            val json = current.toJson().put(key, " ")
            assertTrue(key, runCatching { PhoneInvocation.fromParams(JSONObject().put("invocation", json)) }.isFailure)
        }
        assertTrue(runCatching { PhoneChatAddress.fromJson(JSONObject().put("chat_id", "")) }.isFailure)
        assertTrue(runCatching { invocation().copy(turnId = "x".repeat(201)) }.isFailure)
    }

    @Test fun activationKeepsTheLeaseAcrossCallsAndRejectsAnotherChat() {
        val state = PhoneSessionState()
        val first = state.activate(invocation())
        assertEquals(first, state.activate(invocation()))
        assertTrue(state.isCurrent(invocation()))
        assertTrue(runCatching { state.activate(invocation().copy(owner = "other")) }.isFailure)
        assertTrue(runCatching { state.activate(invocation().copy(chatAddress = PhoneChatAddress("other"))) }.isFailure)
        assertEquals(first, state.current())
    }

    @Test fun bindingAdoptsOnlyAnExistingOwnedChatAndRetiresItsPreviousTurn() {
        val state = PhoneSessionState()
        assertFalse(state.bindTurn(invocation()))
        assertNull(state.current())
        val first = state.activate(invocation())
        assertFalse(state.bindTurn(invocation("turn-2").copy(owner = "other")))
        assertEquals(first, state.current())
        val next = invocation("turn-2").copy(authorityEpoch = "next-authority")
        assertTrue(state.bindTurn(next))
        assertEquals(first.sessionId, state.current()!!.sessionId)
        assertTrue(state.isCurrent(next))
        assertFalse(state.isCurrent(invocation()))
        assertTrue(runCatching { state.activate(invocation()) }.isFailure)
        assertTrue(runCatching { state.activate(invocation().copy(authorityEpoch = "changed")) }.isFailure)
        assertTrue(runCatching { state.bindTurn(invocation()) }.isFailure)
        assertFalse(state.release(invocation()))
        assertTrue(state.isCurrent(next))
        assertEquals(next, state.stop(first.sessionId))
    }

    @Test fun aNewActionCanAdoptTheNextTurnButCannotChangeMetadataWithinATurn() {
        val state = PhoneSessionState()
        val first = state.activate(invocation())
        for (changed in listOf(
            invocation().copy(authorityEpoch = "changed"),
        )) {
            assertTrue(runCatching { state.activate(changed) }.isFailure)
            assertTrue(runCatching { state.bindTurn(changed) }.isFailure)
        }
        assertEquals(first.sessionId, state.activate(invocation("turn-2")).sessionId)
        assertTrue(runCatching { state.activate(invocation()) }.isFailure)
    }

    @Test fun explicitReleaseIsExactAndIdempotentWithoutRevokingTheTurn() {
        val state = PhoneSessionState()
        val current = invocation()
        val first = state.activate(current)
        assertFalse(state.release(current.copy(authorityEpoch = "stale")))
        assertEquals(first, state.current())
        assertTrue(state.release(current))
        assertNull(state.current())
        assertFalse(state.release(current))
        assertNotEquals(first.sessionId, state.activate(current).sessionId)
    }

    @Test fun userStopRevokesTheWholeTurnUntilTheBridgeGenerationResets() {
        val state = PhoneSessionState()
        val current = invocation()
        val sessionId = state.activate(current).sessionId
        assertEquals(current, state.stop(sessionId))
        assertNull(state.current())
        assertNull(state.stop(sessionId))
        assertTrue(runCatching { state.activate(current) }.isFailure)
        assertTrue(runCatching { state.activate(current.copy(authorityEpoch = "changed")) }.isFailure)
        assertTrue(runCatching { state.bindTurn(current) }.isFailure)
        state.clear()
        assertTrue(runCatching { state.activate(current) }.isFailure)
        assertTrue(state.isCurrent(state.activate(invocation("turn-2")).invocation))
        state.reset()
        assertNull(state.current())
        assertEquals(current, state.activate(current).invocation)
    }

    @Test fun clearingALeaseDoesNotForgetRetiredTurns() {
        val state = PhoneSessionState()
        state.activate(invocation())
        state.bindTurn(invocation("turn-2"))
        state.clear()
        assertTrue(runCatching { state.activate(invocation()) }.isFailure)
        state.reset()
        assertEquals(invocation(), state.activate(invocation()).invocation)
    }

    @Test fun aLateStopFromAnOldSurfaceCannotStopAReplacementSession() {
        val state = PhoneSessionState()
        val first = state.activate(invocation())
        assertTrue(state.release(invocation()))
        val replacement = state.activate(invocation("turn-2"))
        assertNotEquals(first.sessionId, replacement.sessionId)
        assertNull(state.stop(first.sessionId))
        assertEquals(replacement, state.current())
        assertEquals(replacement.invocation, state.stop(replacement.sessionId))
        assertTrue(runCatching { state.activate(replacement.invocation) }.isFailure)
    }

    @Test fun observersRunOutsideTheStateLockAndOnlyWhenTheLeaseChanges() {
        val state = PhoneSessionState()
        val worker = Executors.newSingleThreadExecutor()
        val seen = mutableListOf<PhoneUseLease?>()
        state.onChanged = { seen.add(worker.submit(Callable { state.current() }).get(2, TimeUnit.SECONDS)) }
        try {
            state.activate(invocation())
            state.activate(invocation())
            state.bindTurn(invocation("turn-2"))
            state.bindTurn(invocation("turn-2"))
            state.release(invocation())
            state.stop(state.current()!!.sessionId)
            state.clear()
            assertEquals(listOf("turn-1", "turn-2", null), seen.map { it?.invocation?.turnId })
        } finally {
            worker.shutdownNow()
        }
    }

    @Test fun concurrentOwnersCannotBothAcquireTheDevice() {
        val state = PhoneSessionState()
        val workers = Executors.newFixedThreadPool(2)
        try {
            val attempts = workers.invokeAll(listOf("one", "two").map { owner ->
                Callable { runCatching { state.activate(invocation().copy(owner = owner)) }.isSuccess }
            })
            assertEquals(1, attempts.count { it.get() })
            assertNotNull(state.current())
        } finally {
            workers.shutdownNow()
        }
    }
}
