package com.rua.mobile

import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment

@RunWith(RobolectricTestRunner::class)
class PhoneCameraTest {
    @Test fun externalIntentsCannotClaimAnUnissuedPhotoRequest() {
        assertNull(PhoneCamera.claim(null))
        assertNull(PhoneCamera.claim("forged"))
    }
    @Test fun photoFailsWithoutSeparateCameraPermission() {
        val invocation = PhoneInvocation("test-owner", PhoneChatAddress("chat"), "turn", "authority")
        val result=runCatching {PhoneCamera.take(RuntimeEnvironment.getApplication(),invocation,"test-call","0","generation")}
        assertTrue(result.isFailure)
        assertTrue(result.exceptionOrNull()!!.message!!.contains("camera permission"))
    }
}
