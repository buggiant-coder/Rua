package com.rua.mobile

import android.app.Notification
import android.app.NotificationManager
import android.os.PowerManager
import com.rua.mobile.ForegroundOperationLease
import com.rua.mobile.ForegroundOperationSnapshot
import com.rua.mobile.MainActivity
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.android.controller.ServiceController

@RunWith(RobolectricTestRunner::class)
class NativeRuntimeServiceResidencyTest {
    private lateinit var controller: ServiceController<NativeRuntimeService>
    private lateinit var service: NativeRuntimeService

    @Before
    fun setUp() {
        controller = Robolectric.buildService(NativeRuntimeService::class.java).create()
        service = controller.get()
    }

    @After
    fun tearDown() {
        controller.destroy()
    }

    @Test
    fun unifiedNotificationIsOngoingActionlessAndOpensRua() {
        val notification = service.buildNotification()

        assertEquals("手Rua", notification.extras.getString(Notification.EXTRA_TITLE))
        assertEquals(
            "手Rua 正在运行",
            notification.extras.getString(Notification.EXTRA_TEXT),
        )
        assertTrue(notification.flags and Notification.FLAG_ONGOING_EVENT != 0)
        assertTrue(notification.actions.isNullOrEmpty())
        assertEquals(
            MainActivity::class.java.name,
            shadowOf(notification.contentIntent).savedIntent.component?.className,
        )

        val manager = service.getSystemService(NotificationManager::class.java)
        assertEquals(
            1,
            manager.activeNotifications.count {
                it.id == 1201
            },
        )
    }

    @Test
    fun residencySnapshotsReuseOneLockPairThroughGraceAndReleaseOnSuspend() {
        val active = snapshot(
            revision = 1,
            locksRequired = true,
            residencyState = "active",
        )
        service.applyForegroundOperationSnapshot(active)

        val firstWakeLock = requireNotNull(field<PowerManager.WakeLock>("wakeLock"))
        val firstWifiLock = requireNotNull(field<Any>("wifiLock"))
        assertTrue((field<PowerManager.WakeLock>("wakeLock")?.isHeld == true))
        assertTrue(firstWakeLock.isHeld)

        service.applyForegroundOperationSnapshot(active.copy(revision = 2))
        assertSame(firstWakeLock, field<PowerManager.WakeLock>("wakeLock"))
        assertSame(firstWifiLock, field<Any>("wifiLock"))

        service.applyForegroundOperationSnapshot(
            active.copy(
                revision = 3,
                activeCount = 0,
                residencyState = "background_grace",
                operations = emptyList(),
            ),
        )
        assertSame(firstWakeLock, field<PowerManager.WakeLock>("wakeLock"))
        assertSame(firstWifiLock, field<Any>("wifiLock"))

        service.applyForegroundOperationSnapshot(
            active.copy(
                revision = 4,
                activeCount = 0,
                locksRequired = false,
                residencyState = "suspended",
                operations = emptyList(),
            ),
        )
        assertFalse((field<PowerManager.WakeLock>("wakeLock")?.isHeld == true))
        assertFalse(firstWakeLock.isHeld)
    }

    @Test
    fun destroyReleasesEveryLockAndIgnoresQueuedSnapshots() {
        val active = snapshot(1, true, "active")
        service.applyForegroundOperationSnapshot(active)
        val wake = requireNotNull(field<PowerManager.WakeLock>("wakeLock"))
        val wifi = requireNotNull(field<android.net.wifi.WifiManager.WifiLock>("wifiLock"))
        val multicast = requireNotNull(field<android.net.wifi.WifiManager.MulticastLock>("multicastLock"))
        service.onDestroy()
        service.applyForegroundOperationSnapshot(active.copy(revision = 2))
        assertFalse(wake.isHeld)
        assertFalse(wifi.isHeld)
        assertFalse(multicast.isHeld)
        assertEquals(null, field<Any>("wakeLock"))
    }

    private fun snapshot(
        revision: Long,
        locksRequired: Boolean,
        residencyState: String,
    ): ForegroundOperationSnapshot = ForegroundOperationSnapshot(
        revision = revision,
        background = true,
        activeCount = 1,
        locksRequired = locksRequired,
        residencyState = residencyState,
        graceGeneration = 1,
        operations = listOf(
            ForegroundOperationLease(
                leaseId = "transfer:a",
                operationId = "a",
                kind = "airdrop_transfer",
                bytesWritten = 1,
                totalBytes = 2,
            ),
        ),
    )

    @Suppress("UNCHECKED_CAST")
    private fun <T> field(name: String): T? {
        val field = NativeRuntimeService::class.java.getDeclaredField(name)
        field.isAccessible = true
        return field.get(service) as T?
    }
}
