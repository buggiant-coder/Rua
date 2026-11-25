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
        assertTrue(runCatching { PhoneTreeCapture.capture(service, PhoneObservations.MAX_NODES + 1, 10) }.isFailure)
        assertTrue(runCatching { PhoneTreeCapture.capture(service, 30, PhoneObservations.MAX_DEPTH + 1) }.isFailure)
        assertTrue(runCatching { PhoneTreeCapture.capture(service, 0, 10) }.isFailure)
    }

    @Test fun structuralContainersDoNotEarnAnObservationSlot() {
        // A layout wrapper with only an id and no readable or actionable state is noise.
        assertFalse(PhoneNodeFilter.isMeaningful("", "", false, false, false, false, false))
        assertFalse(PhoneNodeFilter.isMeaningful("  ", "", false, false, false, false, false))
        // Any readable text, description, or actionable state earns a slot.
        assertTrue(PhoneNodeFilter.isMeaningful("发送", "", false, false, false, false, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "搜索", false, false, false, false, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "", true, false, false, false, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "", false, true, false, false, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "", false, false, true, false, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "", false, false, false, true, false))
        assertTrue(PhoneNodeFilter.isMeaningful("", "", false, false, false, false, true))
    }

    @Test fun payloadTrimDropsTrailingNodesAndFlagsTruncation() {
        val nodes = JSONArray()
        repeat(20) { nodes.put(JSONObject().put("text", "消息$it")) }
        val result = JSONObject().put("nodes", nodes).put("truncated", false)
        val limit = result.toString().toByteArray(Charsets.UTF_8).size / 2
        PhoneTreeCapture.trimToPayloadLimit(result, nodes, limit)
        assertTrue(result.getBoolean("truncated"))
        assertTrue(nodes.length() in 1 until 20)
        assertTrue(result.toString().toByteArray(Charsets.UTF_8).size <= limit)
    }

    @Test fun observationCeilingIsRaisedAboveTheOldFortyNodeLimit() {
        assertEquals(120, PhoneObservations.MAX_NODES)
        assertTrue(PhoneObservations.DEFAULT_NODES < PhoneObservations.MAX_NODES)
        // The serialized tree must stay inside the resident's observe response frame.
        assertTrue(PhoneObservations.MAX_PAYLOAD_BYTES < 256 * 1024)
        assertTrue(PhoneObservations.MAX_SCREENSHOT_PAYLOAD_BYTES < PhoneObservations.MAX_PAYLOAD_BYTES)
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
