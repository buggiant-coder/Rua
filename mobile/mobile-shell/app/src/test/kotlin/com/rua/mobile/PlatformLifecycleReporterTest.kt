package com.rua.mobile

import org.junit.Assert.assertEquals
import org.junit.Test

class PlatformLifecycleReporterTest {
    @Test
    fun residentRestartInvalidatesEvenAnInFlightAcknowledgement() {
        val bodies = mutableListOf<String>()
        lateinit var reporter: PlatformLifecycleReporter
        reporter = PlatformLifecycleReporter(
            endpoint = "http://127.0.0.1/lifecycle",
            postJson = { _, body ->
                bodies += body
                if (bodies.size == 1) reporter.residentChanged()
            },
            launchAsync = { it.run() },
            sleepMs = {},
            logFailure = { _, _ -> },
        )
        reporter.reportForeground()
        assertEquals(2, bodies.size)
        reporter.reportForeground()
        assertEquals(2, bodies.size)
        reporter.residentChanged()
        assertEquals(3, bodies.size)
    }

    @Test
    fun foregroundAcknowledgementRunsOnlyAfterSuccessfulDelivery() {
        var attempts = 0
        var acknowledgements = 0
        val reporter = PlatformLifecycleReporter(
            endpoint = "http://127.0.0.1/lifecycle",
            postJson = { _, _ ->
                attempts += 1
                if (attempts == 1) error("server starting")
            },
            launchAsync = { task -> task.run() },
            sleepMs = {},
            logFailure = { _, _ -> },
        )

        reporter.reportForeground { acknowledgements += 1 }

        assertEquals(2, attempts)
        assertEquals(1, acknowledgements)
    }

    @Test
    fun failedDeliveryRemainsPendingUntilAcknowledged() {
        val bodies = mutableListOf<String>()
        var attempts = 0
        val reporter = PlatformLifecycleReporter(
            endpoint = "http://127.0.0.1/lifecycle",
            postJson = { _, body ->
                bodies += body
                attempts += 1
                if (attempts == 1) error("server starting")
            },
            launchAsync = { task -> task.run() },
            sleepMs = {},
            logFailure = { _, _ -> },
        )

        reporter.reportForeground()
        reporter.reportForeground()

        assertEquals(
            listOf(
                """{"event":"foreground"}""",
                """{"event":"foreground"}""",
            ),
            bodies,
        )
    }

    @Test
    fun retrySendsTheLatestDesiredStateInsteadOfAStaleState() {
        val bodies = mutableListOf<String>()
        var staleForegroundAcknowledgements = 0
        lateinit var reporter: PlatformLifecycleReporter
        reporter = PlatformLifecycleReporter(
            endpoint = "http://127.0.0.1/lifecycle",
            postJson = { _, body ->
                bodies += body
                if (bodies.size == 1) error("server starting")
            },
            launchAsync = { task -> task.run() },
            sleepMs = { reporter.reportBackground() },
            logFailure = { _, _ -> },
        )

        reporter.reportForeground { staleForegroundAcknowledgements += 1 }

        assertEquals(
            listOf(
                """{"event":"foreground"}""",
                """{"event":"background"}""",
            ),
            bodies,
        )
        assertEquals(0, staleForegroundAcknowledgements)
    }
}
