package com.rua.mobile

import org.junit.After
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File
import java.net.ServerSocket
import java.security.MessageDigest
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [28], shadows = [RuntimeMirrorDownloaderTest.DirectorySyncShadow::class])
class RuntimeMirrorDownloaderTest {
    // Robolectric cannot open directories for fsync; device tests cover real atomic publication.
    @org.robolectric.annotation.Implements(android.system.Os::class)
    class DirectorySyncShadow {
        companion object {
            @JvmStatic @org.robolectric.annotation.Implementation
            fun open(path: String, flags: Int, mode: Int): java.io.FileDescriptor = java.io.FileDescriptor()
            @JvmStatic @org.robolectric.annotation.Implementation
            fun fsync(fd: java.io.FileDescriptor) { }
            @JvmStatic @org.robolectric.annotation.Implementation
            fun close(fd: java.io.FileDescriptor) { }
        }
    }
    private val requests = java.util.Collections.synchronizedList(mutableListOf<String>())
    private val executor = Executors.newCachedThreadPool()
    private data class Response(val delay: Long, val code: Int, val content: ByteArray, val size: Long)
    private val responses = java.util.concurrent.ConcurrentHashMap<String, Response>()
    private val server = ServerSocket(0, 10, java.net.InetAddress.getByName("127.0.0.1"))
    init {
        executor.execute {
            while (!server.isClosed) {
                val socket = try { server.accept() } catch (_: java.net.SocketException) { break }
                executor.execute {
                    socket.use {
                        val reader = it.getInputStream().bufferedReader()
                        val request = reader.readLine().split(' ')
                        while (!reader.readLine().isNullOrEmpty()) { }
                        val method = request[0]
                        val path = request[1]
                        requests.add(method + path)
                        val response = responses[path]!!
                        Thread.sleep(response.delay)
                        val output = it.getOutputStream()
                        output.write("HTTP/1.1 ${response.code} Fixture\r\nContent-Length: ${response.size}\r\nConnection: close\r\n\r\n".toByteArray())
                        if (method != "HEAD") output.write(response.content)
                        output.flush()
                    }
                }
            }
        }
    }
    private val root = java.nio.file.Files.createTempDirectory("runtime-mirrors").toFile()
    private val bytes = "trusted artifact".toByteArray()
    private val artifact = LinuxRuntimeInstaller.Artifact("fixed-arm64.tar.gz", bytes.size.toLong(), sha(bytes))
    private fun sha(bytes: ByteArray) = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
    private fun source(path: String, delay: Long = 0, code: Int = 200, content: ByteArray = bytes, size: Long = content.size.toLong()): RuntimeMirrorDownloader.Source {
        responses[path] = Response(delay, code, content, size)
        return RuntimeMirrorDownloader.Source(path, "http://127.0.0.1:${server.localPort}$path")
    }
    private fun downloader(cancel: () -> Boolean = { false }) = RuntimeMirrorDownloader(cancel, {}, allowLoopbackHttp = true)
    @After fun close() { server.close(); executor.shutdownNow(); root.deleteRecursively() }

    @Test fun fastestEligibleFileWinsAndUnavailableMetadataIsExcluded() {
        val missing = source("/missing", code = 404)
        val wrongSize = source("/wrong-size", size = 1)
        val slow = source("/slow", delay = 150)
        val fast = source("/fast", delay = 15)
        downloader().use { client ->
            val ranked = client.rank(artifact, listOf(missing, wrongSize, slow, fast))
            assertEquals(listOf("/fast", "/slow"), ranked.map { it.source.name })
            assertTrue(ranked.all { it.nanos > 0 })
        }
    }

    @Test fun corruptedFastDownloadFallsBackWithoutPublishingWrongBytes() {
        val corrupt = source("/corrupt", content = ByteArray(bytes.size))
        val good = source("/good", delay = 60)
        val target = File(root, artifact.sha256)
        downloader().use { it.download(artifact, target, listOf(corrupt, good)) }
        assertArrayEquals(bytes, target.readBytes())
        assertTrue(requests.contains("GET/corrupt"))
        assertTrue(requests.contains("GET/good"))
        assertFalse(root.listFiles()!!.any { it.name.endsWith(".part") })
    }

    @Test fun debianAndNodeChooseTheirOwnFastestMirror() {
        val debianA = source("/a/debian", delay = 10)
        val debianB = source("/b/debian", delay = 120)
        val nodeA = source("/a/node", delay = 120)
        val nodeB = source("/b/node", delay = 10)
        downloader().use { client ->
            assertEquals("/a/debian", client.rank(artifact, listOf(debianB, debianA)).first().source.name)
            assertEquals("/b/node", client.rank(artifact.copy(file = "node-fixed-arm64.tar.gz"), listOf(nodeA, nodeB)).first().source.name)
        }
    }

    @Test fun verifiedCacheSkipsNetworkAndCancellationNeverPublishes() {
        val target = File(root, artifact.sha256)
        target.writeBytes(bytes)
        downloader().use { it.download(artifact, target, emptyList()) }
        target.delete()
        val stop = AtomicBoolean(true)
        try { downloader(stop::get).use { it.download(artifact, target, listOf(source("/cancel"))) }; fail("accepted cancellation") }
        catch (_: java.util.concurrent.CancellationException) { }
        assertFalse(target.exists())
        assertTrue(requests.isEmpty())
    }

    @Test fun damagedCacheIsQuarantinedBeforeDownloadingTrustedContent() {
        val target = File(root, artifact.sha256).apply { writeBytes(ByteArray(bytes.size)) }
        downloader().use { it.download(artifact, target, listOf(source("/replace"))) }
        assertArrayEquals(bytes, target.readBytes())
        val invalid = root.listFiles()!!.filter { it.name.startsWith("invalid-${artifact.sha256}-") }
        assertEquals(1, invalid.size)
        assertArrayEquals(ByteArray(bytes.size), invalid.single().readBytes())
    }

    @Test fun metadataPreparationHasOneBoundedWindow() {
        downloader().use { client ->
            val started = System.nanoTime()
            val available = client.prepareSources(listOf(
                { Thread.sleep(12_000); RuntimeMirrorDownloader.Source("late", "https://example.invalid/late") },
                { RuntimeMirrorDownloader.Source("ready", "https://example.invalid/ready") },
                { error("metadata invalid") },
            ))
            val elapsed = TimeUnit.NANOSECONDS.toMillis(System.nanoTime() - started)
            assertEquals(listOf("ready"), available.map { it.name })
            assertTrue("Probe took $elapsed ms", elapsed < 10_000)
        }
    }

    @Test fun productionRejectsCleartextAndAllFailuresStayInactive() {
        val source = source("/cleartext")
        RuntimeMirrorDownloader({ false }, {}).use { client ->
            assertTrue(client.rank(artifact, listOf(source)).isEmpty())
            try { client.download(artifact, File(root, artifact.sha256), listOf(source)); fail("downloaded over HTTP") }
            catch (_: IllegalStateException) { }
        }
        assertFalse(File(root, artifact.sha256).exists())
    }
}
