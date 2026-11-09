package com.rua.mobile

import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.ServerSocket
import java.security.MessageDigest
import java.util.concurrent.Executors

@RunWith(AndroidJUnit4::class)
class RuntimeDownloadRecoveryDeviceTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val bytes = ByteArray(128 * 1024) { (it % 251).toByte() }
    private val artifact get() = LinuxRuntimeInstaller.Artifact("recovery-fixture.bin", bytes.size.toLong(),
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) })

    private fun transfer(pause: Boolean) {
        val server = ServerSocket(0, 2, java.net.InetAddress.getByName("127.0.0.1"))
        val executor = Executors.newSingleThreadExecutor()
        val methods = java.util.Collections.synchronizedList(mutableListOf<String>())
        executor.execute {
            repeat(2) {
                server.accept().use { socket ->
                    val reader = socket.getInputStream().bufferedReader()
                    val method = reader.readLine().substringBefore(' ')
                    methods.add(method)
                    var header = reader.readLine()
                    while (!header.isNullOrEmpty()) {
                        check(!header.startsWith("Range:", true)) { "Unverified partial transfer must restart in full" }
                        header = reader.readLine()
                    }
                    socket.getOutputStream().use { output ->
                        output.write("HTTP/1.1 200 OK\r\nContent-Length: ${bytes.size}\r\nConnection: close\r\n\r\n".toByteArray())
                        if (method == "GET") output.write(bytes)
                    }
                }
            }
        }
        try {
            val target = File(LinuxRuntimeInstaller.cache(context), artifact.sha256)
            RuntimeMirrorDownloader({ false }, { status ->
                if (pause && status.contains("100%")) {
                    LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/fault-stage.json"),
                        JSONObject().put("phase", "download").put("pid", android.os.Process.myPid()).put("activatedAt", 0).toString())
                    while (true) SystemClock.sleep(1000)
                }
            }, allowLoopbackHttp = true).use {
                it.download(artifact, target, listOf(RuntimeMirrorDownloader.Source("loopback fixture", "http://127.0.0.1:${server.localPort}/artifact")))
            }
            assertArrayEquals(bytes, target.readBytes())
            assertEquals(listOf("HEAD", "GET"), methods)
        } finally { server.close(); executor.shutdownNow() }
    }

    @Test fun pauseTransferForExternalKill() {
        File(LinuxRuntimeInstaller.cache(context), artifact.sha256).delete()
        transfer(true)
        fail("Did not pause inside partial transfer")
    }

    @Test fun recoverTransferAfterProcessKill() {
        val parts = LinuxRuntimeInstaller.cache(context).listFiles()!!.filter { it.name.startsWith("download-") && it.name.endsWith(".part") }
        assertTrue("Process death should leave a real incomplete transfer", parts.any { it.length() in 1 until artifact.size })
        LinuxRuntimeInstaller.recover(context)
        assertTrue(parts.none { it.exists() })
        transfer(false)
        assertEquals("workspace survives", File(context.filesDir, "home/recovery-test-data").readText())
        assertEquals("not owned", File(context.filesDir, "runtime/foreign-staging/keep").readText())
    }
}
