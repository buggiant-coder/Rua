package com.rua.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.os.Handler
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.util.ReflectionHelpers
import java.time.Duration

@RunWith(RobolectricTestRunner::class)
class DnsSyncManagerTest {
    @Test
    fun networkEventsNotifyNativeRuntimeWithoutGuestAndCoalesceBeforeDelivery() {
        val context = RuntimeEnvironment.getApplication()
        val connectivity = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        var notifications = 0
        val manager = DnsSyncManager(context) { notifications++ }
        manager.start()
        val worker = ReflectionHelpers.getField<Handler>(manager, "workHandler")
        val callback = shadowOf(connectivity).networkCallbacks.single()
        val network = org.robolectric.shadows.ShadowNetwork.newInstance(42)
        try {
            callback.onAvailable(network)
            callback.onLost(network)
            callback.onAvailable(network)
            shadowOf(worker.looper).idleFor(Duration.ofMillis(200))
            assertEquals(1, notifications)
            callback.onLost(network)
            shadowOf(worker.looper).idleFor(Duration.ofMillis(200))
            assertEquals(2, notifications)
            callback.onAvailable(network)
        } finally {
            manager.stop()
        }
        assertEquals(2, notifications)
        assertEquals(0, shadowOf(connectivity).networkCallbacks.size)
    }
}
