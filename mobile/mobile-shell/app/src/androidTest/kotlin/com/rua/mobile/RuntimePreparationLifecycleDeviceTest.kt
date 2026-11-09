package com.rua.mobile

import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Run only on the owned acceptance AVD: temporarily toggles its Wi-Fi/data. */
@RunWith(AndroidJUnit4::class)
class RuntimePreparationLifecycleDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val context get() = instrumentation.targetContext
    private fun shell(command: String) {
        instrumentation.uiAutomation.executeShellCommand(command).use { descriptor ->
            android.os.ParcelFileDescriptor.AutoCloseInputStream(descriptor).use { it.readBytes() }
        }
    }
    private fun await(message: String, seconds: Int = 30, condition: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + seconds * 1000
        while (!condition() && SystemClock.elapsedRealtime() < deadline) SystemClock.sleep(100)
        if (!condition()) {
            val stacks = Thread.getAllStackTraces().filter { (_, stack) -> stack.any { it.className.contains("RuntimePreparation") || it.className.contains("LinuxRuntimeInstaller") } }
                .entries.joinToString("\n") { (thread, stack) -> "${thread.name} ${thread.state}\n" + stack.joinToString("\n") }
            LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/preparation-threads.txt"), stacks)
            fail("$message: ${LinuxRuntimeInstaller.status}\n$stacks")
        }
    }
    private fun phase(): String = File(context.filesDir, "runtime/preparation.json").takeIf { it.isFile }
        ?.let { JSONObject(it.readText()).optString("phase") } ?: ""

    private fun clickPreparationAction(activity: ActivityScenario<MainActivity>, label: String) {
        await("Preparation action $label is visible") {
            var visible = false
            activity.onActivity {
                val button = it.findViewById<android.widget.Button>(R.id.rua_runtime_setup_action)
                visible = button?.isShown == true && button.isEnabled && button.text.toString() == label
            }
            visible
        }
        activity.onActivity { assertTrue(it.findViewById<android.widget.Button>(R.id.rua_runtime_setup_action).performClick()) }

    }

    private fun installation() = JSONObject(File(context.filesDir, "runtime/installation.json").readText())
    private fun guestPids(staging: String): List<String> = File("/proc").listFiles().orEmpty().filter { process ->
        process.name.toIntOrNull() != null && runCatching {
            File(process, "cmdline").readText().contains(staging) || File(process, "cwd").canonicalPath.startsWith(File(staging).canonicalPath + "/")
        }.getOrDefault(false)
    }.map { it.name }

    @Test fun offlineStartupRecoversAndExplicitCancelSurvivesRecreationUntilRetry() {
        assertFalse("Move the owned fixture guest aside before this test", LinuxRuntimeInstaller.ready(context))
        RuntimePreparation.retry(context)
        shell("svc wifi disable")
        shell("svc data disable")
        try {
            SystemClock.sleep(1500)
            ActivityScenario.launch(MainActivity::class.java).use { activity ->
                await("Offline first launch waits for network") { phase() == "waiting_network" }
                assertFalse(RuntimePreparation.running)
                shell("svc wifi enable")
                await("Network restoration starts preparation without a click") { RuntimePreparation.running }
                await("Real package configuration has started") { runCatching { installation().optString("phase") == "configuring" }.getOrDefault(false) }
                val operation = installation().getString("operation_id")
                val staging = File(context.filesDir, "runtime/" + installation().getString("staging")).path
                await("Installer guest processes actually exist") { guestPids(staging).isNotEmpty() }
                val children = guestPids(staging)
                RuntimePreparation.retry(context)
                RuntimePreparation.retry(context)
                try {
                    RuntimePreparation.importOffline(context, java.io.ByteArrayInputStream(byteArrayOf()))
                    fail("Concurrent import acquired a second writer")
                } catch (error: IllegalStateException) { assertTrue(error.message.orEmpty().contains("先取消")) }
                activity.recreate()
                assertEquals("Activity/retry created another installer", operation, installation().getString("operation_id"))
                clickPreparationAction(activity, "取消准备")
                await("Cancellation stops the owner", 15) { !RuntimePreparation.running }
                assertEquals("cancelled", phase())
                await("Cancellation removes installer descendants", 15) { children.none { File("/proc/$it").exists() } && guestPids(staging).isEmpty() }
                assertFalse(LinuxRuntimeInstaller.ready(context))
                activity.recreate()
                SystemClock.sleep(1000)
                assertFalse(RuntimePreparation.running)
                assertEquals("cancelled", phase())
                RuntimePreparation.stop(context)
                RuntimePreparation.start(context)
                SystemClock.sleep(1000)
                assertFalse("Persisted cancellation must survive owner restart", RuntimePreparation.running)
                clickPreparationAction(activity, "重试")
                await("Explicit retry activates the verified runtime", 300) { RuntimeToolchains.ready(context) }
                val marker = File(context.filesDir, "runtime/debian/.rua-runtime-ready").lastModified()
                activity.recreate()
                SystemClock.sleep(1000)
                assertFalse(RuntimePreparation.running)
                assertEquals("Ready environment was reinstalled", marker, File(context.filesDir, "runtime/debian/.rua-runtime-ready").lastModified())
            }
        } finally { shell("svc wifi enable"); shell("svc data enable") }
    }
}
