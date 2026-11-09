package com.rua.mobile

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class NativeToolProbeDeviceTest {
    @Test
    fun realExecutorCancellationAndExplicitDeadlineCleanUpDebianDescendants() = runProbe(null, "NATIVE_PROBE_ALL_PASS")

    @Test
    fun realPluginProtocolEnvironmentCancellationAndOwnerShutdownUseDebian() = runProbe("--plugin", "NATIVE_PLUGIN_ALL_PASS")

    private fun runProbe(argument: String?, marker: String) {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        assertTrue(LinuxRuntimeInstaller.ready(context))
        val probe = File(instrumentation.context.applicationInfo.nativeLibraryDir, "librua-runtime-probe.so")
        check(probe.canExecute()) { "Build the Android native_runtime_probe example before assembling the test APK" }
        val log = File(context.filesDir, "runtime-test/probe.log").apply { parentFile!!.mkdirs() }
        val builder = ProcessBuilder(listOfNotNull(probe.path, argument)).redirectErrorStream(true).redirectOutput(log)
        builder.environment().apply {
            clear()
            putAll(NativePayload.process(context, emptyList()).environment())
        }
        val child = builder.start()
        try {
            assertTrue("Probe timed out", child.waitFor(60, TimeUnit.SECONDS))
            val output = log.readText()
            assertEquals(output, 0, child.exitValue())
            assertTrue(output, output.contains(marker))
        } finally { if (child.isAlive) { child.destroyForcibly(); child.waitFor() } }
    }
}
