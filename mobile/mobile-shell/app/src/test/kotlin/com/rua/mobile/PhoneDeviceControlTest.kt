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
        val invocation = PhoneInvocation("owner", PhoneChatAddress("agent", "default", "chat"), "turn", "authority", "configuration")
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
        val invocation = PhoneInvocation("owner", PhoneChatAddress("agent", "default", "chat"), "turn", "authority", "configuration")
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

}
