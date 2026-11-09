package com.rua.mobile

import android.util.Log
import java.io.BufferedReader
import java.io.InputStreamReader
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import org.json.JSONObject

data class ForegroundOperationLease(
    val leaseId: String,
    val operationId: String,
    val kind: String,
    val bytesWritten: Long,
    val totalBytes: Long,
)

data class ForegroundOperationSnapshot(
    val revision: Long,
    val background: Boolean,
    val activeCount: Int,
    val locksRequired: Boolean,
    val residencyState: String,
    val graceGeneration: Long,
    val operations: List<ForegroundOperationLease>,
) {
    companion object {
        @JvmStatic
        fun parse(json: String): ForegroundOperationSnapshot {
            val root = JSONObject(json)
            val operationsJson = root.optJSONArray("operations")
            val operations = buildList {
                if (operationsJson != null) {
                    for (index in 0 until operationsJson.length()) {
                        val operation = operationsJson.getJSONObject(index)
                        add(
                            ForegroundOperationLease(
                                leaseId = operation.optString("lease_id"),
                                operationId = operation.optString("operation_id"),
                                kind = operation.optString("kind"),
                                bytesWritten = operation.optLong("bytes_written"),
                                totalBytes = operation.optLong("total_bytes"),
                            )
                        )
                    }
                }
            }
            return ForegroundOperationSnapshot(
                revision = root.optLong("revision"),
                background = root.optBoolean("background"),
                activeCount = root.optInt("active_count"),
                locksRequired = root.optBoolean("locks_required"),
                residencyState = root.optString("residency_state"),
                graceGeneration = root.optLong("grace_generation"),
                operations = operations,
            )
        }

        @JvmStatic
        fun notificationText(snapshot: ForegroundOperationSnapshot?): String {
            if (snapshot == null) return "手Rua 正在运行"
            val operations = uniqueOperations(snapshot)
            if (operations.isEmpty()) {
                return if (snapshot.residencyState == "background_grace") {
                    "正在完成后台同步"
                } else {
                    "手Rua 正在运行"
                }
            }
            val kinds = operations.map { operation -> operation.kind }.toSet()
            if (kinds.size > 1) return "${operations.size} 项活动正在进行"
            return when (kinds.single()) {
                "conversation_execution" -> "对话正在运行"
                "chat_viewer" -> "远程对话正在查看"
                "terminal_viewer" -> "远程终端正在查看"
                "airdrop_transfer" -> {
                    val written = operations.sumOf { operation -> operation.bytesWritten }
                    val total = operations.sumOf { operation -> operation.totalBytes }
                    val progress = if (total > 0) {
                        " ${formatBytes(written)}/${formatBytes(total)}"
                    } else {
                        ""
                    }
                    "${operations.size} 个隔空传送$progress"
                }
                else -> "${operations.size} 项活动正在进行"
            }
        }

        @JvmStatic
        fun notificationProgress(snapshot: ForegroundOperationSnapshot?): Int? {
            if (snapshot == null) return null
            val transfers = uniqueOperations(snapshot)
                .filter { operation -> operation.kind == "airdrop_transfer" }
            val total = transfers.sumOf { operation -> operation.totalBytes }
            if (total <= 0) return null
            val written = transfers.sumOf { operation -> operation.bytesWritten }
            return ((written * 100L) / total).toInt().coerceIn(0, 100)
        }

        private fun uniqueOperations(
            snapshot: ForegroundOperationSnapshot,
        ): List<ForegroundOperationLease> {
            return snapshot.operations
                .groupBy { operation -> "${operation.kind}:${operation.operationId}" }
                .values
                .map { leases ->
                    leases.maxByOrNull { operation -> operation.bytesWritten } ?: leases.first()
                }
        }

        private fun formatBytes(bytes: Long): String {
            if (bytes < 1024L) return "${bytes}B"
            if (bytes < 1024L * 1024L) return "${bytes / 1024L}KB"
            return "${bytes / (1024L * 1024L)}MB"
        }
    }
}

interface ForegroundOperationMonitorListener {
    fun onSnapshot(snapshot: ForegroundOperationSnapshot)
    fun onUnavailable()
}

class ForegroundOperationMonitor(
    private val endpoint: String,
    private val onSnapshot: (ForegroundOperationSnapshot) -> Unit,
    private val maxUnavailableMs: Long? = null,
    private val onUnavailable: () -> Unit = {},
    private val logFailure: (String, Throwable) -> Unit = { message, error ->
        Log.w(MainActivity.TAG, message, error)
    },
) {
    private val running = AtomicBoolean(false)
    @Volatile private var connection: HttpURLConnection? = null
    @Volatile private var lastReachableAtMs: Long = System.currentTimeMillis()
    @Volatile private var hasReachedHost = false
    @Volatile private var outageLogged = false
    @Volatile private var unavailableNotified = false

    fun start() {
        if (!running.compareAndSet(false, true)) return
        thread(name = "ForegroundOperationMonitor") {
            while (running.get()) {
                try {
                    consumeEvents()
                } catch (error: Throwable) {
                    if (running.get() && shouldLogFailure()) {
                        logFailure("foreground operation event stream failed", error)
                    }
                }
                val unavailableLimit = maxUnavailableMs
                if (
                    running.get() &&
                    unavailableLimit != null &&
                    System.currentTimeMillis() - lastReachableAtMs >= unavailableLimit &&
                    !unavailableNotified
                ) {
                    onUnavailable()
                    unavailableNotified = true
                }
                if (running.get()) {
                    try {
                        Thread.sleep(RECONNECT_DELAY_MS)
                    } catch (_: InterruptedException) {
                        Thread.currentThread().interrupt()
                        return@thread
                    }
                }
            }
        }
    }

    fun stop() {
        running.set(false)
        connection?.disconnect()
        connection = null
    }

    private fun consumeEvents() {
        val current = (URL(endpoint).openConnection() as HttpURLConnection).apply {
            requestMethod = "GET"
            connectTimeout = CONNECT_TIMEOUT_MS
            readTimeout = 0
            setRequestProperty("Accept", "application/x-ndjson")
            PhoneUseBridge.authorize(this, endpoint)
        }
        connection = current
        try {
            val code = current.responseCode
            if (code !in 200..299) {
                throw java.io.IOException("foreground operation stream returned HTTP $code")
            }
            recordReachable()
            BufferedReader(InputStreamReader(current.inputStream, Charsets.UTF_8)).use { reader ->
                while (running.get()) {
                    val line = reader.readLine() ?: return
                    if (line.isNotBlank()) {
                        recordReachable()
                        onSnapshot(ForegroundOperationSnapshot.parse(line))
                    }
                }
            }
        } finally {
            if (connection === current) {
                connection = null
            }
            current.disconnect()
        }
    }

    internal fun recordReachable(nowMs: Long = System.currentTimeMillis()) {
        lastReachableAtMs = nowMs
        hasReachedHost = true
        outageLogged = false
        unavailableNotified = false
    }

    internal fun shouldLogFailure(): Boolean {
        if (!hasReachedHost || outageLogged) return false
        outageLogged = true
        return true
    }

    companion object {
        private const val CONNECT_TIMEOUT_MS = 1_000
        private const val RECONNECT_DELAY_MS = 1_000L
        private const val RESIDENT_HOST_UNAVAILABLE_MS = 30_000L

        @JvmStatic
        fun eventsEndpoint(serverBaseUrl: String): String =
            serverBaseUrl.trimEnd('/') + "/api/platform/foreground-operations/events"

        @JvmStatic
        fun forResidentService(
            serverBaseUrl: String,
            listener: ForegroundOperationMonitorListener,
        ): ForegroundOperationMonitor {
            return ForegroundOperationMonitor(
                endpoint = eventsEndpoint(serverBaseUrl),
                onSnapshot = listener::onSnapshot,
                maxUnavailableMs = RESIDENT_HOST_UNAVAILABLE_MS,
                onUnavailable = listener::onUnavailable,
            )
        }
    }
}
