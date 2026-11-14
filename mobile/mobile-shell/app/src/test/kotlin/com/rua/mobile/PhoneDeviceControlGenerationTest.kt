package com.rua.mobile

import android.os.Handler
import android.os.Looper
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.LooperMode
import org.robolectric.shadows.ShadowSettings

@RunWith(RobolectricTestRunner::class)
@LooperMode(LooperMode.Mode.PAUSED)
class PhoneDeviceControlGenerationTest {
    private fun invocation(turn: String) = PhoneInvocation(
        "owner", PhoneChatAddress("agent", "default", "chat"), turn, "authority", "configuration",
    )

    @Test fun aQueuedOldGenerationCannotAcquireOrReplaceTheNewGenerationsSession() {
        val context = RuntimeEnvironment.getApplication()
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(context, "old-generation") {}
        var rejected = false
        Handler(Looper.getMainLooper()).post {
            rejected = runCatching { PhoneDeviceControl.begin(invocation("old"), "queued", "old-generation") }.isFailure
        }
        PhoneDeviceControl.close("old-generation")
        PhoneDeviceControl.initialize(context, "new-generation") {}
        val current = PhoneDeviceControl.sessions.activate(invocation("new"))
        try {
            shadowOf(Looper.getMainLooper()).idle()
            assertTrue(rejected)
            assertEquals(current, PhoneDeviceControl.sessions.current())
            assertNull(PhoneDeviceControl.state.current())
        } finally {
            PhoneDeviceControl.close("new-generation")
            PhoneDeviceControl.sessions.reset()
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

    @Test fun aLateCloseFromTheOldGenerationCannotClearTheNewSession() {
        val context = RuntimeEnvironment.getApplication()
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(context, "old-generation") {}
        PhoneDeviceControl.close("old-generation")
        PhoneDeviceControl.initialize(context, "new-generation") {}
        val current = PhoneDeviceControl.sessions.activate(invocation("new"))
        try {
            PhoneDeviceControl.close("old-generation")
            shadowOf(Looper.getMainLooper()).idle()
            assertEquals(current, PhoneDeviceControl.sessions.current())
            assertTrue(PhoneDeviceControl.sessions.isCurrent(current.invocation))
        } finally {
            PhoneDeviceControl.close("new-generation")
            PhoneDeviceControl.sessions.reset()
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

    @Test fun oldGenerationControlMessagesCannotMutateTheCurrentInvocation() {
        val context = RuntimeEnvironment.getApplication()
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(context, "current-generation") {}
        val current = PhoneDeviceControl.sessions.activate(invocation("current"))
        val operation = PhoneDeviceControl.state.begin(current.invocation, "call")
        try {
            runCatching { PhoneDeviceControl.release(current.invocation, "old-generation") }
            assertEquals(current, PhoneDeviceControl.sessions.current())
            runCatching { PhoneDeviceControl.bindTurn(invocation("stale"), "old-generation") }
            assertEquals(current, PhoneDeviceControl.sessions.current())
            runCatching { PhoneDeviceControl.cancel(current.invocation, "call", "old-generation") }
            assertFalse(operation.cancelled.get())
            assertSame(operation, PhoneDeviceControl.state.current())
            PhoneDeviceControl.fail(current.invocation, "call", "old-generation")
            assertEquals(current, PhoneDeviceControl.sessions.current())
            assertFalse(operation.cancelled.get())
            assertSame(operation, PhoneDeviceControl.state.current())
        } finally {
            PhoneDeviceControl.close("current-generation")
            PhoneDeviceControl.sessions.reset()
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }
}
