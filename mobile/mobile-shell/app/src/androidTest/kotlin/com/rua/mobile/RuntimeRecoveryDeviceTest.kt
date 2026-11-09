package com.rua.mobile

import android.content.Context
import android.content.ContextWrapper
import android.os.SystemClock
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Fault harness pauses inside real filesystem installation; the driver kills the app process. */
@RunWith(AndroidJUnit4::class)
class RuntimeRecoveryDeviceTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private val runtime get() = File(context.filesDir, "runtime")
    private val marker get() = File(context.filesDir, "runtime-test/fault-stage.json")

    @Test fun pauseInstallationForExternalProcessKill() {
        val phase = InstrumentationRegistry.getArguments().getString("fault_phase") ?: error("fault_phase required")
        assertFalse(LinuxRuntimeInstaller.ready(context))
        marker.delete()
        LinuxRuntimeInstaller.installCached(context, onPhase = { current ->
            if (current == phase) {
                val readyMarker = File(runtime, "debian/.rua-runtime-ready")
                LinuxRuntimeInstaller.atomicText(marker, JSONObject().put("phase", current).put("pid", android.os.Process.myPid())
                    .put("activatedAt", if (readyMarker.exists()) readyMarker.lastModified() else 0).toString())
                while (true) SystemClock.sleep(1000)
            }
        })
        fail("Requested phase was never observed")
    }

    @Test fun resumeAfterProcessKillWithoutTouchingWorkspaceOrForeignStaging() {
        val cut = JSONObject(marker.readText())
        val workspace = File(context.filesDir, "home/recovery-test-data")
        val foreign = File(runtime, "foreign-staging/keep")
        assertEquals("workspace survives", workspace.readText())
        assertEquals("not owned", foreign.readText())
        LinuxRuntimeInstaller.recover(context)
        LinuxRuntimeInstaller.installCached(context)
        assertTrue(LinuxRuntimeInstaller.ready(context))
        assertEquals("ready", JSONObject(File(runtime, "installation.json").readText()).getString("phase"))
        if (cut.getString("phase") == "after_activation") {
            assertEquals("Committed guest must not be installed twice", cut.getLong("activatedAt"), File(runtime, "debian/.rua-runtime-ready").lastModified())
        }
        assertEquals("workspace survives", workspace.readText())
        assertEquals("not owned", foreign.readText())
        assertFalse(runtime.listFiles()!!.any { it.name.startsWith("debian.staging-") })
    }

    @Test fun spaceFailuresPreserveDataAndOwnedCleanupDoesNotRemoveForeignDirectories() {
        val isolated = File(context.cacheDir, "recovery-space-fixture").apply { mkdirs() }
        val fixture = object : ContextWrapper(context) { override fun getFilesDir(): File = isolated }
        val data = File(isolated, "home/keep").apply { parentFile!!.mkdirs(); LinuxRuntimeInstaller.atomicText(this, "keep") }
        val foreign = File(isolated, "runtime/debian.staging/keep").apply { parentFile!!.mkdirs(); LinuxRuntimeInstaller.atomicText(this, "foreign") }
        val manifest = LinuxRuntimeInstaller.manifest(context)
        try { LinuxRuntimeInstaller.installCached(fixture, manifest.copy(requiredFreeBytes = Long.MAX_VALUE)); fail("accepted insufficient space") }
        catch (error: IllegalStateException) { assertTrue(error.message.orEmpty().contains("存储空间不足")) }
        val downloads = File(isolated, "runtime/downloads")
        if (!downloads.exists()) android.system.Os.symlink(LinuxRuntimeInstaller.cache(context).path, downloads.path)
        try {
            LinuxRuntimeInstaller.installCached(fixture, manifest, onPhase = {
                if (it == "configuring") throw android.system.ErrnoException("write", android.system.OsConstants.ENOSPC)
            })
            fail("accepted write failure")
        } catch (error: IllegalStateException) { assertTrue(error.message.orEmpty().contains("存储空间不足")) }
        assertFalse(LinuxRuntimeInstaller.ready(fixture))
        assertEquals("keep", data.readText())
        assertEquals("foreign", foreign.readText())
        LinuxRuntimeInstaller.recover(fixture)
        assertFalse(File(isolated, "runtime").listFiles()!!.any { it.name.startsWith("debian.staging-") })
        assertEquals("foreign", foreign.readText())
        assertEquals("keep", data.readText())
        assertTrue("Existing real guest stays available", LinuxRuntimeInstaller.ready(context))
        // Do not recurse through the test's cache link when deleting the isolated fixture.
        downloads.delete()
        isolated.deleteRecursively()
    }
}
