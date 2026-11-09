package com.rua.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class SyncBackgroundWorkerTest {
    @Test
    fun parsesScheduledSyncPolicyFromStatusResponse() {
        val balancedStatus = """
            {
              "effective_power_policy": {
                "mode": "balanced",
                "periodic_interval_secs": 1800,
                "allow_background_scheduled_sync": true
              }
            }
        """.trimIndent()
        val powerSaverStatus = """
            {
              "effective_power_policy": {
                "mode": "power_saver",
                "allow_background_scheduled_sync": false
              }
            }
        """.trimIndent()

        assertTrue(SyncBackgroundWorker.parseAllowBackgroundScheduledSync(balancedStatus))
        assertEquals(
            30L * 60L * 1000L,
            SyncBackgroundWorker.parseScheduledSyncPolicy(balancedStatus).periodicIntervalMs
        )
        assertFalse(SyncBackgroundWorker.parseAllowBackgroundScheduledSync(powerSaverStatus))
        assertFalse(SyncBackgroundWorker.parseAllowBackgroundScheduledSync("{}"))
    }

    @Test
    fun clampsScheduledSyncIntervalToAndroidPeriodicMinimum() {
        val status = """
            {
              "effective_power_policy": {
                "mode": "balanced",
                "periodic_interval_secs": 120,
                "allow_background_scheduled_sync": true
              }
            }
        """.trimIndent()

        val policy = SyncBackgroundWorker.parseScheduledSyncPolicy(status)

        assertTrue(policy.allowBackgroundScheduledSync)
        assertEquals(15L * 60L * 1000L, policy.periodicIntervalMs)
    }

    @Test
    fun buildsLocalServerEndpointsWithoutDuplicatingSlash() {
        assertEquals(
            "http://127.0.0.1:8090/api/sync/status",
            SyncBackgroundWorker.statusEndpoint("http://127.0.0.1:8090/")
        )
        assertEquals(
            "http://127.0.0.1:8090/api/sync/scheduled-background",
            SyncBackgroundWorker.scheduledSyncEndpoint("http://127.0.0.1:8090")
        )
    }

    @Test
    fun scheduledWorkerNeverRequestsImmediateRetryForHttpFailures() {
        assertFalse(SyncBackgroundWorker.shouldRetryScheduledSyncStatus(404))
        assertFalse(SyncBackgroundWorker.shouldRetryScheduledSyncStatus(422))
        assertFalse(SyncBackgroundWorker.shouldRetryScheduledSyncStatus(503))
    }
}
