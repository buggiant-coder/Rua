package com.rua.mobile

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneScreenshotLifecycleTest {
    private fun invocation(turn: String) = PhoneInvocation(
        "owner", PhoneChatAddress("agent", "default", "chat"), turn, "authority", "configuration",
    )

    @Test fun aLateScreenshotFailureDoesNotInvalidateTheNextTurnsObservation() {
        val service = MobileAccessibilityService()
        PhoneDeviceControl.clear()
        try {
            val old = PhoneDeviceControl.state.begin(invocation("old"), "screenshot")
            PhoneDeviceControl.state.finish(old)
            val next = PhoneDeviceControl.state.begin(invocation("next"), "observe")
            val observation = service.observations.remember(next.owner, JSONObject().put("nodes", JSONArray()))
            PhoneScreenshot.invalidateIfCurrent(service, old)
            assertEquals(observation.getString("observation_id"), service.observations.consume(next.owner, observation.getString("observation_id")).id)
            assertSame(next, PhoneDeviceControl.state.current())
        } finally {
            PhoneDeviceControl.clear()
            PhoneDeviceControl.sessions.reset()
        }
    }

    @Test fun theCurrentScreenshotFailureStillInvalidatesItsObservation() {
        val service = MobileAccessibilityService()
        PhoneDeviceControl.clear()
        try {
            val current = PhoneDeviceControl.state.begin(invocation("current"), "screenshot")
            val observation = service.observations.remember(current.owner, JSONObject().put("nodes", JSONArray()))
            PhoneScreenshot.invalidateIfCurrent(service, current)
            assertTrue(runCatching { service.observations.consume(current.owner, observation.getString("observation_id")) }.isFailure)
        } finally {
            PhoneDeviceControl.clear()
            PhoneDeviceControl.sessions.reset()
        }
    }
}
