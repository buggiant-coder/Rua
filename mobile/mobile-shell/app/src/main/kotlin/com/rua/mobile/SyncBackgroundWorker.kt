package com.rua.mobile

import android.app.job.JobInfo
import android.app.job.JobParameters
import android.app.job.JobScheduler
import android.app.job.JobService
import android.content.ComponentName
import android.content.Context
import android.util.Log
import org.json.JSONObject
import java.net.HttpURLConnection
import java.net.URL
import kotlin.concurrent.thread

class SyncBackgroundWorker : JobService() {
    override fun onStartJob(params: JobParameters?): Boolean {
        thread(name = "MobileScheduledSync") {
            val endpoint = scheduledSyncEndpoint(defaultServerBaseUrl())
            try {
                val policy = fetchScheduledSyncPolicy(statusEndpoint(defaultServerBaseUrl()))
                if (!policy.allowBackgroundScheduledSync) {
                    cancel(this)
                    return@thread
                }
                val code = postScheduledSync(endpoint)
                Log.i(MainActivity.TAG, "scheduled background sync finished: HTTP $code")
            } catch (ex: Throwable) {
                Log.w(MainActivity.TAG, "scheduled background sync skipped", ex)
            } finally {
                if (params != null) jobFinished(params, false)
            }
        }
        return true
    }

    override fun onStopJob(params: JobParameters?): Boolean = false

    companion object {
        private const val JOB_ID = 0x50C0
        private const val DEFAULT_PERIODIC_MS = 6L * 60L * 60L * 1000L
        private const val MIN_PERIODIC_MS = 15L * 60L * 1000L
        private const val FLEX_MS = 60L * 60L * 1000L
        private const val TIMEOUT_MS = 1500

        internal data class ScheduledSyncPolicy(
            val allowBackgroundScheduledSync: Boolean,
            val periodicIntervalMs: Long,
        )

        fun refreshScheduleAsync(
            context: Context,
            serverBaseUrl: String = defaultServerBaseUrl(),
        ) {
            val appContext = context.applicationContext
            thread(name = "MobileSyncScheduleProbe") {
                val policy = try {
                    fetchScheduledSyncPolicy(statusEndpoint(serverBaseUrl))
                } catch (ex: Throwable) {
                    Log.w(MainActivity.TAG, "scheduled sync policy probe failed; canceling job", ex)
                    ScheduledSyncPolicy(false, DEFAULT_PERIODIC_MS)
                }
                if (policy.allowBackgroundScheduledSync) {
                    enqueue(appContext, policy.periodicIntervalMs)
                } else {
                    cancel(appContext)
                }
            }
        }

        internal fun enqueue(context: Context, periodicMs: Long = DEFAULT_PERIODIC_MS) {
            val scheduler = context.getSystemService(JobScheduler::class.java) ?: return
            val effectivePeriodicMs = periodicMs.coerceAtLeast(MIN_PERIODIC_MS)
            val effectiveFlexMs = FLEX_MS.coerceAtMost(effectivePeriodicMs)
            val info = JobInfo.Builder(
                JOB_ID,
                ComponentName(context, SyncBackgroundWorker::class.java),
            )
                .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
                .setPersisted(true)
                .setPeriodic(effectivePeriodicMs, effectiveFlexMs)
                .build()
            scheduler.schedule(info)
            Log.i(
                MainActivity.TAG,
                "scheduled background sync enqueued interval_ms=$effectivePeriodicMs flex_ms=$effectiveFlexMs"
            )
        }

        internal fun cancel(context: Context) {
            context.getSystemService(JobScheduler::class.java)?.cancel(JOB_ID)
            Log.i(MainActivity.TAG, "scheduled background sync canceled")
        }

        internal fun defaultServerBaseUrl(): String = "http://127.0.0.1:${MainActivity.RUA_PORT}"

        internal fun statusEndpoint(serverBaseUrl: String): String =
            serverBaseUrl.trimEnd('/') + "/api/sync/status"

        internal fun scheduledSyncEndpoint(serverBaseUrl: String): String =
            serverBaseUrl.trimEnd('/') + "/api/sync/scheduled-background"

        internal fun parseAllowBackgroundScheduledSync(body: String): Boolean =
            parseScheduledSyncPolicy(body).allowBackgroundScheduledSync

        internal fun parseScheduledSyncPolicy(body: String): ScheduledSyncPolicy =
            JSONObject(body)
                .optJSONObject("effective_power_policy")
                ?.let { policy ->
                    ScheduledSyncPolicy(
                        allowBackgroundScheduledSync = policy.optBoolean(
                            "allow_background_scheduled_sync",
                            false
                        ),
                        periodicIntervalMs = policy.optLong(
                            "periodic_interval_secs",
                            DEFAULT_PERIODIC_MS / 1000L
                        )
                            .coerceAtLeast(MIN_PERIODIC_MS / 1000L) * 1000L,
                    )
                }
                ?: ScheduledSyncPolicy(false, DEFAULT_PERIODIC_MS)

        @Suppress("UNUSED_PARAMETER")
        internal fun shouldRetryScheduledSyncStatus(statusCode: Int): Boolean = false

        private fun fetchScheduledSyncPolicy(endpoint: String): ScheduledSyncPolicy {
            val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
                requestMethod = "GET"
                connectTimeout = TIMEOUT_MS
                readTimeout = TIMEOUT_MS
                PhoneUseBridge.authorize(this, endpoint)
            }
            return try {
                if (connection.responseCode !in 200..299) return ScheduledSyncPolicy(
                    false,
                    DEFAULT_PERIODIC_MS
                )
                val body = connection.inputStream.bufferedReader(Charsets.UTF_8).use { it.readText() }
                parseScheduledSyncPolicy(body)
            } finally {
                connection.disconnect()
            }
        }

        internal fun postScheduledSync(endpoint: String): Int {
            val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
                requestMethod = "POST"
                connectTimeout = TIMEOUT_MS
                readTimeout = TIMEOUT_MS
                doOutput = false
                PhoneUseBridge.authorize(this, endpoint)
            }
            return try {
                val code = connection.responseCode
                if (code >= 400) {
                    val stream = connection.errorStream ?: connection.inputStream
                    stream?.close()
                } else {
                    connection.inputStream.close()
                }
                code
            } finally {
                connection.disconnect()
            }
        }
    }
}
