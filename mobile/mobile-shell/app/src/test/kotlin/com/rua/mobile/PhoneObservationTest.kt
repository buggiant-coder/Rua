package com.rua.mobile

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneObservationTest {
    private fun tree() = JSONObject().put("nodes", JSONArray().put(JSONObject().put("text", "测试")))

    @Test fun observationIsChatOwnedAndSingleUse() {
        val store = PhoneObservations { 100 }
        val result = store.remember("chat-a", tree())
        val id = result.getString("observation_id")
        assertEquals("$id:0", result.getJSONArray("nodes").getJSONObject(0).getString("token"))
        assertTrue(runCatching { store.consume("chat-b", id) }.isFailure)
        assertEquals("chat-a", store.consume("chat-a", id).owner)
        assertTrue(runCatching { store.consume("chat-a", id) }.isFailure)
    }

    @Test fun newerObservationAndOwnerFailureInvalidateOldReference() {
        val store = PhoneObservations { 100 }
        val old = store.remember("chat", tree()).getString("observation_id")
        val newer = store.remember("chat", tree()).getString("observation_id")
        assertTrue(runCatching { store.consume("chat", old) }.isFailure)
        store.invalidateOwner("chat")
        assertTrue(runCatching { store.consume("chat", newer) }.isFailure)
    }

    @Test fun expiredAndResetReferencesCannotBeUsed() {
        var now = 100L
        val store = PhoneObservations { now }
        val old = store.remember("chat", tree()).getString("observation_id")
        now += PhoneObservations.MAX_AGE_MS + 1
        assertTrue(runCatching { store.consume("chat", old) }.isFailure)
        val id = store.remember("chat", tree()).getString("observation_id")
        store.clear()
        assertTrue(runCatching { store.consume("chat", id) }.isFailure)
        assertNotEquals(store.generation, PhoneObservations { now }.generation)
    }

    @Test fun limitsAreBoundedBeforeTraversingAndroidNodes() {
        val service = MobileAccessibilityService()
        assertTrue(runCatching { PhoneTreeCapture.capture(service, 41, 10) }.isFailure)
        assertTrue(runCatching { PhoneTreeCapture.capture(service, 30, 13) }.isFailure)
        assertTrue(runCatching { PhoneTreeCapture.capture(service, 0, 10) }.isFailure)
    }

    @Test fun returnedMediaDoesNotMutateStoredObservation() {
        val store = PhoneObservations { 100 }
        val response = store.remember("chat", tree())
        response.put("screenshot", JSONObject().put("data", "large-image"))
        response.getJSONArray("nodes").getJSONObject(0).put("text", "changed")
        val stored = store.consume("chat", response.getString("observation_id")).result
        assertFalse(stored.has("screenshot"))
        assertEquals("测试", stored.getJSONArray("nodes").getJSONObject(0).getString("text"))
    }

}
