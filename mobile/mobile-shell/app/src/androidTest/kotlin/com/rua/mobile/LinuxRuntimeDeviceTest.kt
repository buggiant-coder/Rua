package com.rua.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.apache.commons.compress.archivers.tar.TarArchiveEntry
import org.apache.commons.compress.archivers.tar.TarArchiveOutputStream
import org.apache.commons.compress.archivers.tar.TarConstants
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.zip.GZIPOutputStream

@RunWith(AndroidJUnit4::class)
class LinuxRuntimeDeviceTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext

    private fun archive(vararg entries: Pair<TarArchiveEntry, ByteArray>): ByteArray {
        val bytes = ByteArrayOutputStream()
        TarArchiveOutputStream(GZIPOutputStream(bytes)).use { tar ->
            entries.forEach { (entry, content) ->
                entry.size = content.size.toLong()
                tar.putArchiveEntry(entry)
                tar.write(content)
                tar.closeArchiveEntry()
            }
        }
        return bytes.toByteArray()
    }

    @Test
    fun extractionPreservesGuestLinksAndRejectsEscapes() {
        val root = File(context.cacheDir, "archive-test").apply { deleteRecursively(); mkdirs() }
        val payload = "hello\n".toByteArray()
        val regular = TarArchiveEntry("usr/bin/real").apply { mode = 0x1ed }
        val symlink = TarArchiveEntry("usr/bin/link", TarConstants.LF_SYMLINK).apply { linkName = "/usr/bin/real" }
        val hardlink = TarArchiveEntry("usr/bin/copy", TarConstants.LF_LINK).apply { linkName = "usr/bin/real" }
        LinuxRuntimeInstaller.extractRootfs(ByteArrayInputStream(archive(regular to payload, symlink to byteArrayOf(), hardlink to byteArrayOf())), root)
        assertEquals("hello\n", File(root, "usr/bin/link").readText())
        assertEquals("hello\n", File(root, "usr/bin/copy").readText())
        assertTrue(File(root, "usr/bin/copy").canExecute())
        for (name in listOf("../escaped", "/absolute", "usr/../../../escaped")) {
            try { LinuxRuntimeInstaller.extractRootfs(ByteArrayInputStream(archive(TarArchiveEntry(name, true) to payload)), root); fail("accepted $name") }
            catch (_: IllegalArgumentException) { }
        }
        val escaping = TarArchiveEntry("usr/bin/escape", TarConstants.LF_SYMLINK).apply { linkName = "../../../../outside" }
        try { LinuxRuntimeInstaller.extractRootfs(ByteArrayInputStream(archive(escaping to byteArrayOf())), root); fail("accepted escaping link") }
        catch (_: IllegalArgumentException) { }
        assertFalse(File(context.cacheDir, "escaped").exists())
        root.deleteRecursively()
    }

    @Test
    fun verifiedOfflineMaterialsInstallDebianPythonAndRemainReadyAfterInvalidImport() {
        val materials = File(context.filesDir, "runtime-test/offline.zip")
        check(materials.isFile) { "Push the pinned offline ZIP into this isolated test app first" }
        assertFalse(File(context.filesDir, "usr/bin/bash").exists())
        materials.inputStream().use { LinuxRuntimeInstaller.importOffline(context, it) }
        assertTrue(LinuxRuntimeInstaller.ready(context))
        val log = File(context.filesDir, "runtime/install.log").readText()
        assertTrue(log, log.contains("RUA_DEBIAN_PYTHON_READY"))
        assertTrue(log, log.contains("RUA_VENV_READY"))
        val marker = File(context.filesDir, "runtime/debian/.rua-runtime-ready")
        val identity = marker.readText()
        val activatedAt = marker.lastModified()
        try { LinuxRuntimeInstaller.importOffline(context, ByteArrayInputStream(byteArrayOf(1, 2, 3))); fail("accepted incomplete materials") }
        catch (_: IllegalStateException) { }
        assertTrue(LinuxRuntimeInstaller.ready(context))
        assertEquals(identity, marker.readText())
        LinuxRuntimeInstaller.installCached(context)
        assertEquals("Ready environment must not be reinstalled", activatedAt, marker.lastModified())
    }
    @Test
    fun cancellationDuringOfflineCopyKeepsTheReadyGuestAndVerifiedCache() {
        assertTrue(LinuxRuntimeInstaller.ready(context))
        val marker = File(context.filesDir, "runtime/debian/.rua-runtime-ready")
        val activatedAt = marker.lastModified()
        val started = java.util.concurrent.CountDownLatch(1)
        val release = java.util.concurrent.CountDownLatch(1)
        val executor = java.util.concurrent.Executors.newSingleThreadExecutor()
        val task = executor.submit {
            File(context.filesDir, "runtime-test/offline.zip").inputStream().use { original ->
                val source = object : java.io.FilterInputStream(original) {
                    var bytes = 0
                    override fun read(buffer: ByteArray, offset: Int, length: Int): Int {
                        val count = super.read(buffer, offset, length)
                        if (count > 0) bytes += count
                        if (bytes >= 65536 && started.count > 0) {
                            started.countDown()
                            check(release.await(10, java.util.concurrent.TimeUnit.SECONDS))
                        }
                        return count
                    }
                }
                RuntimePreparation.importOffline(context, source)
            }
        }
        try {
            assertTrue(started.await(5, java.util.concurrent.TimeUnit.SECONDS))
            RuntimePreparation.cancel(context)
            release.countDown()
            try { task.get(5, java.util.concurrent.TimeUnit.SECONDS); fail("Import ignored cancellation") }
            catch (error: java.util.concurrent.ExecutionException) { assertTrue(error.cause?.message.orEmpty().contains("取消")) }
            assertFalse(RuntimePreparation.running)
            assertTrue(LinuxRuntimeInstaller.ready(context))
            assertEquals(activatedAt, marker.lastModified())
            assertTrue(LinuxRuntimeInstaller.manifest(context).artifacts.all {
                LinuxRuntimeInstaller.validCached(File(LinuxRuntimeInstaller.cache(context), it.sha256), it)
            })
            assertFalse(LinuxRuntimeInstaller.cache(context).listFiles()!!.any { it.name.endsWith(".part") })
        } finally { release.countDown(); executor.shutdownNow() }
    }

}
