package com.rua.mobile

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneScreenshotTest {
    private fun target() = JSONObject("""{"package_name":"app","window_id":1,"rotation":0,"width":1080,"height":2400,"nodes":[{"text":"hello","bounds":[0,0,20,20]}]}""")
    @Test fun screenshotRequiresSameWindowGeometryAndTree() {
        val before=target(); before.getJSONArray("nodes").getJSONObject(0).put("token","opaque:0")
        assertTrue(PhoneScreenshot.sameTarget(before,target()))
        for(field in listOf("window_id","rotation","width","height")) {
            assertFalse(PhoneScreenshot.sameTarget(before,target().put(field,99)))
        }
        val changed=target();changed.getJSONArray("nodes").getJSONObject(0).put("text","changed")
        assertFalse(PhoneScreenshot.sameTarget(before,changed))
    }
}
