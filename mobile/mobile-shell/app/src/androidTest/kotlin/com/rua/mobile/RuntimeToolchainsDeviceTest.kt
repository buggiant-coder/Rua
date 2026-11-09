package com.rua.mobile

import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class RuntimeToolchainsDeviceTest {
    @Test fun firstLaunchAutomaticallyAddsNodeAndUvWithoutReplacingDebian() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val base = File(context.filesDir, "runtime/debian/.rua-runtime-ready")
        assertTrue(base.isFile)
        val installedAt = base.lastModified()
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            val deadline = SystemClock.elapsedRealtime() + 300_000
            var recreated = false
            while (!RuntimeToolchains.ready(context) && SystemClock.elapsedRealtime() < deadline) {
                if (RuntimePreparation.running && !recreated) { scenario.recreate(); recreated = true }
                SystemClock.sleep(500)
            }
            assertTrue(LinuxRuntimeInstaller.status + "\n" + File(context.filesDir, "runtime/install.log").readText().takeLast(2500), RuntimeToolchains.ready(context))
            assertEquals("Adding tools must preserve Debian and installed packages", installedAt, base.lastModified())
            val artifacts = LinuxRuntimeInstaller.manifest(context).toolchains
            assertTrue(artifacts.all { LinuxRuntimeInstaller.validCached(File(LinuxRuntimeInstaller.cache(context), it.sha256), it) })
            val root = File(context.filesDir, "runtime/debian")
            val node = org.json.JSONObject(File(root, "usr/local/lib/rua/toolchains/${artifacts[0].sha256}/.rua-versions.json").readText())
            assertEquals("v24.18.0", node.getString("node"))
            assertEquals(node.getString("npm"), node.getString("npx"))
            RuntimeToolchains.installCached(context)
            assertEquals(installedAt, base.lastModified())
        }
    }
    @Test fun cancelledToolchainInstallResumesOfflineAndPreservesExistingData() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertTrue(RuntimeToolchains.ready(context))
        val root = File(context.filesDir, "runtime/debian")
        val tools = File(root, "usr/local/lib/rua/toolchains")
        val backup = File(tools.parentFile, "fixture-backup-${java.util.UUID.randomUUID()}")
        val base = File(root, ".rua-runtime-ready")
        val installedAt = base.lastModified()
        val sentinel = File(context.filesDir, "home/.rua/workspaces/default/skill-dependency-sentinel")
        LinuxRuntimeInstaller.atomicText(sentinel, "preserve")
        check(tools.renameTo(backup))
        try {
            try {
                RuntimeToolchains.installCached(context) {
                    tools.listFiles().orEmpty().filter { it.name.startsWith(".staging-") }
                        .any { File(it, "node-v24.18.0-linux-arm64/bin/node").isFile }
                }
                fail("Cancellation ignored")
            } catch (_: java.util.concurrent.CancellationException) { }
            assertFalse(RuntimeToolchains.ready(context))
            assertTrue(LinuxRuntimeInstaller.ready(context))
            File(context.filesDir, "runtime-test/offline.zip").inputStream().use { LinuxRuntimeInstaller.importOffline(context, it) }
            assertTrue(RuntimeToolchains.ready(context))
            assertEquals(installedAt, base.lastModified())
            assertEquals("preserve", sentinel.readText())
            assertFalse(tools.listFiles()!!.any { it.name.startsWith(".staging-") })
        } finally {
            if (tools.exists()) LinuxRuntimeInstaller.removeStaging(tools)
            check(backup.renameTo(tools))
            NativePayload.syncDirectory(tools.parentFile!!)
        }
    }

}
