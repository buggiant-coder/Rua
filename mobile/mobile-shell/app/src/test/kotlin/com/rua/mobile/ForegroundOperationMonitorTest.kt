package com.rua.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class ForegroundOperationMonitorTest {
    @Test
    fun parsesForegroundOperationSnapshotAndBuildsEndpoint() {
        val snapshot = ForegroundOperationSnapshot.parse(
            """
            {
              "revision": 7,
              "background": true,
              "active_count": 1,
              "locks_required": true,
              "residency_state": "active",
              "grace_generation": 3,
              "operations": [{
                "lease_id": "job:one",
                "operation_id": "one",
                "kind": "airdrop_transfer",
                "bytes_written": 1048576,
                "total_bytes": 2097152
              }]
            }
            """.trimIndent()
        )

        assertEquals(7, snapshot.revision)
        assertTrue(snapshot.background)
        assertEquals(1, snapshot.activeCount)
        assertTrue(snapshot.locksRequired)
        assertEquals("active", snapshot.residencyState)
        assertEquals(3, snapshot.graceGeneration)
        assertEquals("airdrop_transfer", snapshot.operations.single().kind)
        assertEquals(1_048_576, snapshot.operations.single().bytesWritten)
        assertEquals(
            "http://127.0.0.1:8090/api/platform/foreground-operations/events",
            ForegroundOperationMonitor.eventsEndpoint("http://127.0.0.1:8090/"),
        )
    }

    @Test
    fun startupConnectionFailuresStaySilentAndEachLaterOutageLogsOnce() {
        val monitor = ForegroundOperationMonitor(
            endpoint = "http://127.0.0.1:1",
            onSnapshot = {},
        )

        assertFalse(monitor.shouldLogFailure())
        monitor.recordReachable(nowMs = 10)
        assertTrue(monitor.shouldLogFailure())
        assertFalse(monitor.shouldLogFailure())
        monitor.recordReachable(nowMs = 20)
        assertTrue(monitor.shouldLogFailure())
    }

    @Test
    fun notificationContentIsGenericAndDeduplicatesTransferLeases() {
        val transfer = ForegroundOperationSnapshot(
            revision = 2,
            background = true,
            activeCount = 2,
            locksRequired = true,
            residencyState = "active",
            graceGeneration = 1,
            operations = listOf(
                ForegroundOperationLease(
                    "job:a",
                    "a",
                    "airdrop_transfer",
                    1_048_576,
                    2_097_152,
                ),
                ForegroundOperationLease(
                    "receive:a",
                    "a",
                    "airdrop_transfer",
                    1_048_576,
                    2_097_152,
                ),
            ),
        )
        val grace = transfer.copy(
            revision = 3,
            activeCount = 0,
            residencyState = "background_grace",
            operations = emptyList(),
        )
        val mixed = transfer.copy(
            revision = 4,
            activeCount = 2,
            operations = listOf(
                ForegroundOperationLease("chat:a", "a", "chat_viewer", 0, 0),
                ForegroundOperationLease("terminal:b", "b", "terminal_viewer", 0, 0),
            ),
        )

        assertEquals("手Rua 正在运行", ForegroundOperationSnapshot.notificationText(null))
        assertEquals("正在完成后台同步", ForegroundOperationSnapshot.notificationText(grace))
        assertEquals(
            "1 个隔空传送 1MB/2MB",
            ForegroundOperationSnapshot.notificationText(transfer),
        )
        assertEquals("2 项活动正在进行", ForegroundOperationSnapshot.notificationText(mixed))
        assertEquals(50, ForegroundOperationSnapshot.notificationProgress(transfer))
    }
}
