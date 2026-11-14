package com.rua.mobile

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.shadows.ShadowSettings

@RunWith(RobolectricTestRunner::class)
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
}
