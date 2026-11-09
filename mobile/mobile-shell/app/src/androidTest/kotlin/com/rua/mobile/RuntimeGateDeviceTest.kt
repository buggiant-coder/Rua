package com.rua.mobile

import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class RuntimeGateDeviceTest {
    companion object {
        fun assertUpdateBusy(context: android.content.Context) {
            val error = runCatching { RuntimeUpdate.install(context, download = { error("Occupied environment reached download") }) }.exceptionOrNull()
            assertTrue(error?.message.orEmpty(), error?.message.orEmpty().contains("正在使用中"))
            assertFalse(File(context.filesDir, "runtime/update.pending").exists())
        }
    }
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val native get() = File(context.applicationInfo.nativeLibraryDir)
    private fun gate(root: File, exclusive: Boolean, command: List<String> = emptyList()): Process {
        val builder = ProcessBuilder(listOf(File(native, "librua-runtime-gate.so").path,
            if (exclusive) "--exclusive" else "--shared", File(root, "use.lock").path, File(root, "update.pending").path) + command)
            .redirectErrorStream(true)
        builder.environment().apply {
            clear()
            put("LD_LIBRARY_PATH", native.path)
            put("PROOT_LOADER", File(native, "libproot-loader.so").path)
            put("PROOT_NO_SECCOMP", "1")
            put("PROOT_TMP_DIR", File(context.filesDir, "tmp").path)
        }
        return builder.start()
    }

    @Test fun prootRetainsUseLockEvenWhenGuestClosesInheritedDescriptors() {
        val fixture = File(context.cacheDir, "gate-${UUID.randomUUID()}").apply { mkdirs() }
        val signal = File(context.filesDir, "tmp/gate-${UUID.randomUUID()}")
        var child: Process? = null
        var exclusive: Process? = null
        try {
            child = gate(fixture, false, listOf(File(native, "libproot.so").path, "-0", "--kill-on-exit", "--link2symlink",
                "-r", File(context.filesDir, "runtime/debian").path, "-b", "/dev", "-b", "/proc",
                "-b", "${File(context.filesDir, "tmp").path}:/tmp", "-w", "/root", "/usr/bin/env", "-i", "PATH=/usr/bin:/bin",
                "/usr/bin/python3", "-c", "import os,time; os.closerange(3,256); open('/tmp/${signal.name}','w').write('ready'); time.sleep(3)"))
            val deadline = SystemClock.elapsedRealtime() + 10_000
            while (!signal.exists() && child.isAlive && SystemClock.elapsedRealtime() < deadline) SystemClock.sleep(20)
            assertTrue("Actual guest must start", signal.exists())
            val blocked = gate(fixture, true)
            assertTrue(blocked.waitFor(3, TimeUnit.SECONDS))
            assertEquals(blocked.inputStream.bufferedReader().readText(), 75, blocked.exitValue())
            assertTrue(child.waitFor(8, TimeUnit.SECONDS))
            assertEquals(child.inputStream.bufferedReader().readText(), 0, child.exitValue())
            exclusive = gate(fixture, true)
            assertEquals("LOCKED", exclusive.inputStream.bufferedReader().readLine())
            exclusive.outputStream.close()
            assertTrue(exclusive.waitFor(3, TimeUnit.SECONDS))
            assertEquals(0, exclusive.exitValue())
        } finally {
            child?.destroy(); child?.waitFor(5, TimeUnit.SECONDS)
            exclusive?.destroy(); exclusive?.waitFor(5, TimeUnit.SECONDS)
            signal.delete()
            LinuxRuntimeInstaller.removeStaging(fixture)
        }
    }

    @Test fun exclusiveUpdateAndInterruptedJournalRejectNewExecutionWithoutSideEffects() {
        val fixture = File(context.cacheDir, "gate-${UUID.randomUUID()}").apply { mkdirs() }
        var exclusive: Process? = null
        try {
            exclusive = gate(fixture, true)
            assertEquals("LOCKED", exclusive.inputStream.bufferedReader().readLine())
            val marker = File(fixture, "executed")
            fun attempt(): Process = gate(fixture, false, listOf("/system/bin/touch", marker.path))
            val busy = attempt()
            assertTrue(busy.waitFor(3, TimeUnit.SECONDS)); assertEquals(75, busy.exitValue())
            assertFalse(marker.exists())
            LinuxRuntimeInstaller.atomicText(File(fixture, "update.pending"), "interrupted")
            exclusive.outputStream.close()
            assertTrue(exclusive.waitFor(3, TimeUnit.SECONDS))
            val recovery = attempt()
            assertTrue(recovery.waitFor(3, TimeUnit.SECONDS)); assertEquals(75, recovery.exitValue())
            assertFalse(marker.exists())
            assertTrue(File(fixture, "update.pending").delete())
            val resumed = attempt()
            assertTrue(resumed.waitFor(3, TimeUnit.SECONDS)); assertEquals(0, resumed.exitValue())
            assertTrue(marker.isFile)
        } finally {
            exclusive?.destroy(); exclusive?.waitFor(5, TimeUnit.SECONDS)
            LinuxRuntimeInstaller.removeStaging(fixture)
        }
    }
}
