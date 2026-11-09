package com.rua.mobile

import java.io.File
import java.io.FilterInputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URL
import java.util.concurrent.Callable
import java.util.concurrent.CancellationException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/** Ranks equivalent pinned artifacts using actual HTTP metadata latency, never ICMP. */
internal class RuntimeMirrorDownloader(
    private val cancelled: () -> Boolean,
    private val progress: (String) -> Unit,
    private val allowLoopbackHttp: Boolean = false,
) : java.io.Closeable {
    companion object {
        // Process counters support diagnostics and zero-download APK replacement acceptance.
        val receivedBytes = java.util.concurrent.atomic.AtomicLong()
        val requests = java.util.concurrent.atomic.AtomicLong()
    }
    private val pool = Executors.newFixedThreadPool(3)
    override fun close() { pool.shutdownNow() }

    data class Source(val name: String, val url: String, val headers: Map<String, String> = emptyMap())
    data class Candidate(val source: Source, val nanos: Long)
    private var failures = emptyList<String>()

    fun prepareSources(jobs: List<() -> Source>): List<Source> {
        checkCancelled()
        return completed(pool.invokeAll(jobs.map { job -> Callable { job() } }, 8, TimeUnit.SECONDS))
            .also { checkCancelled() }
    }

    private fun <T> completed(results: List<java.util.concurrent.Future<T>>): List<T> {
        val errors = mutableListOf<String>()
        val values = results.mapNotNull { result ->
            try {
                if (result.isCancelled) { errors.add("镜像检测超时"); null } else result.get()
            } catch (error: Exception) { errors.add(error.cause?.message ?: error.message ?: "镜像检测失败"); null }
        }
        failures = errors.distinct().take(3)
        return values
    }

    fun checkCancelled() {
        if (cancelled() || Thread.currentThread().isInterrupted) throw CancellationException("已取消环境准备")
    }

    private fun connection(source: Source, method: String): HttpURLConnection {
        var url = URL(source.url)
        repeat(6) {
            checkCancelled()
            check(url.protocol == "https" || (allowLoopbackHttp && url.protocol == "http" && url.host in listOf("127.0.0.1", "localhost"))) { "环境来源必须使用 HTTPS" }
            val connection = (url.openConnection() as HttpURLConnection).apply {
                requestMethod = method
                connectTimeout = 4000
                readTimeout = 4000
                instanceFollowRedirects = false
                setRequestProperty("Accept-Encoding", "identity")
                // Registry pull tokens belong only to the registry, never its blob redirect/CDN.
                if (url.host == URL(source.url).host) source.headers.forEach { (key, value) -> setRequestProperty(key, value) }
            }
            requests.incrementAndGet()
            val code = try { connection.responseCode } catch (error: Exception) { connection.disconnect(); throw error }
            if (code in listOf(301, 302, 303, 307, 308)) {
                val next = connection.getHeaderField("Location")
                connection.disconnect()
                check(next != null) { "镜像跳转缺少地址" }
                url = URL(url, next)
            } else {
                if (code !in 200..299) { connection.disconnect(); error("${source.name}: HTTP $code") }
                return connection
            }
        }
        error("镜像跳转次数过多")
    }

    /** All candidates address the same fixed version/architecture; missing files are ineligible. */
    fun rank(artifact: LinuxRuntimeInstaller.Artifact, sources: List<Source>): List<Candidate> {
        checkCancelled()
        val jobs = sources.map { source -> Callable {
            val started = System.nanoTime()
            val connection = connection(source, "HEAD")
            try {
                check(connection.getHeaderField("Content-Length")?.toLongOrNull() == artifact.size) { "${source.name}: 材料大小与清单不符" }
                Candidate(source, System.nanoTime() - started)
            } finally { connection.disconnect() }
        } }
        // A slow or unreachable mirror cannot extend one selection indefinitely.
        return completed(pool.invokeAll(jobs, 8, TimeUnit.SECONDS))
            .sortedBy { it.nanos }.also { checkCancelled() }
    }

    fun metadata(source: Source): ByteArray {
        val connection = connection(source, "GET")
        try {
            return connection.inputStream.use { input ->
                val bytes = input.readBytesBounded(1024 * 1024)
                checkCancelled()
                bytes
            }
        } finally { connection.disconnect() }
    }

    fun download(artifact: LinuxRuntimeInstaller.Artifact, target: File, sources: List<Source>) {
        checkCancelled()
        if (LinuxRuntimeInstaller.validCached(target, artifact)) return
        if (target.exists()) {
            val invalid = File(target.parentFile, "invalid-${artifact.sha256}-${java.util.UUID.randomUUID()}")
            check(target.renameTo(invalid)) { "无法隔离损坏的缓存" }
            NativePayload.syncDirectory(target.parentFile!!)
        }
        progress("正在检测镜像 · ${artifact.file}")
        val candidates = if (sources.isEmpty()) emptyList() else rank(artifact, sources)
        check(candidates.isNotEmpty()) { "没有镜像提供所需版本：${artifact.file}（${failures.joinToString("；")}）" }
        var last: Exception? = null
        for (candidate in candidates) {
            checkCancelled()
            try {
                val connection = connection(candidate.source, "GET")
                try {
                    var received = 0L
                    connection.inputStream.use { input ->
                        val checked = object : FilterInputStream(input) {
                            override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                                checkCancelled()
                                val count = super.read(buffer, offset, length)
                                if (count > 0) { received += count; receivedBytes.addAndGet(count.toLong()) }
                                progress("正在下载 ${artifact.file} · ${received * 100 / artifact.size}% · ${candidate.source.name}")
                                return count
                            }
                        }
                        LinuxRuntimeInstaller.verifiedCopy(checked, target, artifact)
                    }
                } finally { connection.disconnect() }
                return
            } catch (error: Exception) {
                checkCancelled()
                last = error
            }
        }
        throw IllegalStateException("所有镜像下载失败：${artifact.file}（${last?.message}）", last)
    }

    private fun InputStream.readBytesBounded(limit: Int): ByteArray {
        val output = java.io.ByteArrayOutputStream()
        val buffer = ByteArray(8192)
        while (true) {
            checkCancelled()
            val count = read(buffer)
            if (count < 0) break
            receivedBytes.addAndGet(count.toLong())
            check(output.size() + count <= limit) { "镜像元数据过大" }
            output.write(buffer, 0, count)
        }
        return output.toByteArray()
    }
}
