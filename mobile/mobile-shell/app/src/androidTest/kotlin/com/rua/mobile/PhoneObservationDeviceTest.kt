package com.rua.mobile

import android.content.Intent
import android.provider.Settings
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PhoneObservationDeviceTest {
    @Test
    fun enabledServiceObservesRealWindowAndRejectsOtherChat() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        instrumentation.getUiAutomation(android.app.UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
        instrumentation.targetContext.startActivity(Intent(Settings.ACTION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        instrumentation.waitForIdleSync()
        val deadline = android.os.SystemClock.elapsedRealtime() + 10_000
        var observation: org.json.JSONObject? = null
        var lastFailure = "accessibility service is not connected"
        while (android.os.SystemClock.elapsedRealtime() < deadline && observation == null) {
            instrumentation.runOnMainSync {
                val service = MobileAccessibilityService.instance
                if (service != null) {
                    val captured = runCatching { service.observe("device-test-a", 30, 10) }
                    observation = captured.getOrNull()
                    captured.exceptionOrNull()?.let { lastFailure = it.message ?: it.javaClass.simpleName }
                }
            }
            if (observation == null) android.os.SystemClock.sleep(100)
        }
        val result = observation ?: throw AssertionError("Phone observation unavailable: $lastFailure")
        assertTrue(result.getString("package_name").isNotBlank())
        assertTrue(result.getInt("width") > 0)
        assertTrue(result.getJSONArray("nodes").length() in 1..30)
        val id = result.getString("observation_id")
        instrumentation.runOnMainSync {
            val service = MobileAccessibilityService.instance!!
            assertTrue(runCatching { service.observations.consume("device-test-b", id) }.isFailure)
            service.observations.consume("device-test-a", id)
            assertTrue(runCatching { service.observations.consume("device-test-a", id) }.isFailure)
        }
    }
}
