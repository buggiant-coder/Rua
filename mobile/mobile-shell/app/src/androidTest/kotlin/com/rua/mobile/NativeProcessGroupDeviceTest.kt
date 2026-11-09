package com.rua.mobile

import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class NativeProcessGroupDeviceTest {
    private fun verify(cancel: Boolean) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val output = File.createTempFile("installer-group-", ".log", context.cacheDir)
        val script = if (cancel) "sleep 60 & echo \"$$ $!\"; wait" else "sleep 60 & echo \"$!\"; exit 17"
        val builder = ProcessBuilder(File(context.applicationInfo.nativeLibraryDir, "librua-process-group.so").path,
            "/system/bin/sh", "-c", script).redirectOutput(output).redirectErrorStream(true)
        builder.environment().apply { clear(); put("PATH", "/system/bin") }
        val child = builder.start()
        try {
            val deadline = SystemClock.elapsedRealtime() + 5000
            var pids = emptyList<Int>()
            while (pids.size != (if (cancel) 2 else 1) && SystemClock.elapsedRealtime() < deadline) {
                pids = output.readText().trim().split(' ').mapNotNull { it.toIntOrNull() }
                SystemClock.sleep(20)
            }
            assertEquals(output.readText(), if (cancel) 2 else 1, pids.size)
            if (cancel) {
                assertEquals("Installer owns its group", pids.first(), File("/proc/${pids.first()}/stat").readText().substringAfterLast(')').trim().split(' ')[2].toInt())
                child.destroy()
            }
            assertTrue("Native owner did not reap its child", child.waitFor(5, TimeUnit.SECONDS))
            assertEquals(if (cancel) 143 else 17, child.exitValue())
            val cleanup = SystemClock.elapsedRealtime() + 5000
            while (pids.any { File("/proc/$it").exists() } && SystemClock.elapsedRealtime() < cleanup) SystemClock.sleep(50)
            assertTrue("Installer group left descendants", pids.none { File("/proc/$it").exists() })
        } finally { if (child.isAlive) child.destroy(); output.delete() }
    }
    @Test fun cancellationKillsAndReapsTheWholeGroup() = verify(true)
    @Test fun normalExitPreservesStatusAndCleansBackgroundDescendants() = verify(false)
}
