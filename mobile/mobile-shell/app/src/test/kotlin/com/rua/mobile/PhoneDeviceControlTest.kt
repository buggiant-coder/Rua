package com.rua.mobile

import android.os.Looper
import java.time.Duration
import android.widget.LinearLayout
import android.widget.TextView
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.LooperMode
import org.robolectric.shadow.api.Shadow
import org.robolectric.shadows.ShadowWindowManagerImpl
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.shadows.ShadowSettings

@RunWith(RobolectricTestRunner::class)
@LooperMode(LooperMode.Mode.PAUSED)
class PhoneDeviceControlTest {
    @Test fun aLateOperationCancellationCannotReleaseTheSessionOrCancelItsNextOperation() {
        val invocation = PhoneInvocation("owner", PhoneChatAddress("chat"), "turn", "authority", "configuration")
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(RuntimeEnvironment.getApplication(), "generation") {}
        try {
            val lease = PhoneDeviceControl.sessions.activate(invocation)
            val first = PhoneDeviceControl.state.begin(invocation, "first")
            PhoneDeviceControl.state.finish(first)
            val next = PhoneDeviceControl.state.begin(invocation, "next")
            PhoneDeviceControl.cancel(invocation, "first", "generation")
            assertSame(next, PhoneDeviceControl.state.current())
            assertFalse(next.cancelled.get())
            assertEquals(lease, PhoneDeviceControl.sessions.current())
            assertTrue(PhoneDeviceControl.sessions.isCurrent(invocation))
        } finally {
            PhoneDeviceControl.close("generation")
            PhoneDeviceControl.sessions.reset()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

    @Test fun surfaceShowsTheCurrentActionThenWaitsAndIgnoresLateCompletion() {
        val invocation = PhoneInvocation("owner", PhoneChatAddress("chat"), "turn", "authority", "configuration")
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(RuntimeEnvironment.getApplication(), "generation") {}
        try {
            val actions = linkedMapOf(
                "observe" to "Agent 正在观看屏幕",
                "screenshot" to "Agent 正在观看屏幕",
                "click" to "Agent 正在点击屏幕",
                "long_press" to "Agent 正在长按屏幕",
                "type_text" to "Agent 正在输入文字",
                "swipe" to "Agent 正在滑动屏幕",
                "back" to "Agent 正在返回上一页",
                "home" to "Agent 正在返回桌面",
                "launch_app" to "Agent 正在打开应用",
                "take_photo" to "Agent 正在拍照",
                "unknown" to "Agent 正在操作手机",
            )
            val windows = Shadow.extract<ShadowWindowManagerImpl>(RuntimeEnvironment.getApplication().getSystemService(android.content.Context.WINDOW_SERVICE))
            var previous: PhoneOperation? = null
            var surface: LinearLayout? = null
            for ((action, label) in actions) {
                val operation = PhoneDeviceControl.begin(invocation, action, "generation", action)
                previous?.let { PhoneDeviceControl.state.finish(it) }
                shadowOf(Looper.getMainLooper()).idle()
                val panel = windows.views.single() as LinearLayout
                surface?.let { assertSame("Status updates must keep the Stop surface", it, panel) }
                surface = panel
                assertEquals(label, (panel.getChildAt(1) as TextView).text.toString())
                PhoneDeviceControl.state.finish(operation)
                shadowOf(Looper.getMainLooper()).idle()
                assertEquals("Short actions should stay readable", label, (panel.getChildAt(1) as TextView).text.toString())
                shadowOf(Looper.getMainLooper()).idleFor(Duration.ofMillis(700))
                assertEquals("Agent 正在等待下一步", (panel.getChildAt(1) as TextView).text.toString())
                assertTrue(PhoneDeviceControl.hasSurface(invocation))
                previous = operation
            }
            ((surface!!.getChildAt(0) as LinearLayout).getChildAt(1)).performClick()
            shadowOf(Looper.getMainLooper()).idle()
            assertTrue(windows.views.isEmpty())
            assertNull(PhoneDeviceControl.sessions.current())
            previous?.let { PhoneDeviceControl.state.finish(it) }
            shadowOf(Looper.getMainLooper()).idle()
            assertTrue(windows.views.isEmpty())
        } finally {
            PhoneDeviceControl.close("generation")
            PhoneDeviceControl.sessions.reset()
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

    @Test fun explicitReleaseReturnsToItsChatExactlyOnceAndPreservesTheTask() {
        val app = RuntimeEnvironment.getApplication()
        val launches = shadowOf(app)
        ShadowSettings.setCanDrawOverlays(true)
        PhoneDeviceControl.initialize(app, "generation") {}
        try {
            for (address in listOf(
                PhoneChatAddress("chat-one"),
                PhoneChatAddress("chat +/?"),
            )) {
                val invocation = PhoneInvocation("owner", address, "turn", "authority", "configuration")
                PhoneDeviceControl.sessions.activate(invocation)
                val operation = PhoneDeviceControl.state.begin(invocation, "observe", "observe")
                PhoneDeviceControl.state.finish(operation)
                shadowOf(Looper.getMainLooper()).idle()
                assertNull("Finishing an operation must not leave the target app", launches.nextStartedActivity)
                assertFalse(PhoneDeviceControl.release(invocation.copy(turnId = "stale"), "generation"))
                assertTrue(PhoneDeviceControl.release(invocation, "generation"))
                shadowOf(Looper.getMainLooper()).idle()
                val intent = launches.nextStartedActivity
                assertNotNull(intent)
                assertEquals(MainActivity::class.java.name, intent.component!!.className)
                assertEquals(PHONE_USE_RETURN_ACTION, intent.action)
                assertEquals("rua:/${address.apiPath()}", intent.getStringExtra(MobileNotificationBridge.EXTRA_RUA_URI))
                assertTrue(intent.flags and android.content.Intent.FLAG_ACTIVITY_NEW_TASK != 0)
                assertTrue(intent.flags and android.content.Intent.FLAG_ACTIVITY_CLEAR_TOP != 0)
                assertFalse(PhoneDeviceControl.release(invocation, "generation"))
                shadowOf(Looper.getMainLooper()).idle()
                assertNull(launches.nextStartedActivity)
            }
        } finally {
            PhoneDeviceControl.close("generation")
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

    @Test fun stopReturnsToChatWhileFailureAndSupersededReturnsDoNotNavigate() {
        val app = RuntimeEnvironment.getApplication()
        val launches = shadowOf(app)
        ShadowSettings.setCanDrawOverlays(true)
        val invocation = PhoneInvocation("owner", PhoneChatAddress("chat"), "turn", "authority", "configuration")
        PhoneDeviceControl.initialize(app, "generation") {}
        try {
            val lease = PhoneDeviceControl.sessions.activate(invocation)
            val active = PhoneDeviceControl.state.begin(invocation, "active")
            PhoneDeviceControl.stopSession(lease.sessionId)
            assertTrue("Stop cancels before returning to Rua", active.cancelled.get())
            assertNull(PhoneDeviceControl.sessions.current())
            assertNull("Navigation is queued after cancellation", launches.nextStartedActivity)
            PhoneDeviceControl.state.finish(active)
            shadowOf(Looper.getMainLooper()).idle()
            val returned = launches.nextStartedActivity
            assertNotNull(returned)
            assertEquals(MainActivity::class.java.name, returned.component!!.className)
            assertEquals("rua://chats/chat", returned.getStringExtra(MobileNotificationBridge.EXTRA_RUA_URI))
            PhoneDeviceControl.stopSession(lease.sessionId)
            assertFalse(PhoneDeviceControl.release(invocation, "generation"))
            assertTrue(runCatching { PhoneDeviceControl.sessions.activate(invocation) }.isFailure)
            shadowOf(Looper.getMainLooper()).idle()
            assertNull("Repeated Stop or late release must not return again", launches.nextStartedActivity)
            val next = invocation.copy(turnId = "next")
            PhoneDeviceControl.sessions.activate(next)
            val operation = PhoneDeviceControl.state.begin(next, "failed")
            PhoneDeviceControl.fail(operation)
            PhoneDeviceControl.state.finish(operation)
            shadowOf(Looper.getMainLooper()).idle()
            assertNull(launches.nextStartedActivity)
            PhoneDeviceControl.sessions.activate(next)
            assertTrue(PhoneDeviceControl.release(next, "generation"))
            PhoneDeviceControl.sessions.activate(next.copy(turnId = "new-session"))
            shadowOf(Looper.getMainLooper()).idle()
            assertNull(launches.nextStartedActivity)
            assertTrue(PhoneDeviceControl.release(next.copy(turnId = "new-session"), "generation"))
            PhoneDeviceControl.close("generation")
            PhoneDeviceControl.initialize(app, "new-generation") {}
            shadowOf(Looper.getMainLooper()).idle()
            assertNull(launches.nextStartedActivity)
        } finally {
            PhoneDeviceControl.close("generation")
            PhoneDeviceControl.close("new-generation")
            shadowOf(Looper.getMainLooper()).idle()
            ShadowSettings.setCanDrawOverlays(false)
        }
    }

}
