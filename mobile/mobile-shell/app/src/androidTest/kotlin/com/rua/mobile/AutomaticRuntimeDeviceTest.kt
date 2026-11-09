package com.rua.mobile

import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

/** External-network smoke: use an isolated AVD with no active guest/download cache. */
@RunWith(AndroidJUnit4::class)
class AutomaticRuntimeDeviceTest {
    private fun web(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) web(view.getChildAt(index))?.let { return it }
        return null
    }
    @Test fun firstLaunchDownloadsTrustedDebianWithoutAnInstallClick() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertFalse("This acceptance requires a fresh guest", RuntimeToolchains.ready(context))
        assertFalse("This acceptance must download actual bytes", LinuxRuntimeInstaller.cache(context).exists())
        assertFalse(File(context.filesDir, "usr/bin/bash").exists())
        val started = SystemClock.elapsedRealtime()
        ActivityScenario.launch(MainActivity::class.java).use { activity ->
            val token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Native resident handshake missing")
            val startup = SystemClock.elapsedRealtime() + 30_000
            while (!RuntimePreparation.running && SystemClock.elapsedRealtime() < startup) SystemClock.sleep(100)
            assertTrue("First launch should prepare automatically: ${LinuxRuntimeInstaller.status}", RuntimePreparation.running)
            activity.recreate()
            assertTrue("Activity recreation observes the existing operation", RuntimePreparation.running)
            var apiReady = false
            while (!apiReady && SystemClock.elapsedRealtime() < startup) {
                val api = URL("http://127.0.0.1:8090/api/models/active").openConnection() as HttpURLConnection
                try {
                    api.connectTimeout = 1000
                    api.readTimeout = 1000
                    api.setRequestProperty("Authorization", "Bearer $token")
                    apiReady = api.responseCode == 200
                } catch (_: java.io.IOException) { SystemClock.sleep(100) }
                finally { api.disconnect() }
            }
            assertTrue("Resident API remains available during installation", apiReady)
            var setupShown = false
            val setupDeadline = SystemClock.elapsedRealtime() + 30_000
            while (!setupShown && SystemClock.elapsedRealtime() < setupDeadline) {
                activity.onActivity {
                    assertNull("Installation must hide the console", web(it.window.decorView))
                    setupShown = it.findViewById<View>(R.id.rua_runtime_setup)?.isShown == true
                }
                SystemClock.sleep(100)
            }
            assertTrue("First run must show the full-screen setup", setupShown)
            val deadline = SystemClock.elapsedRealtime() + 8 * 60_000
            while (!RuntimeToolchains.ready(context) && SystemClock.elapsedRealtime() < deadline) {
                SystemClock.sleep(500)
            }
            assertTrue(LinuxRuntimeInstaller.status, RuntimeToolchains.ready(context))
            var opened = false
            val consoleDeadline = SystemClock.elapsedRealtime() + 30_000
            while (!opened && SystemClock.elapsedRealtime() < consoleDeadline) {
                activity.onActivity {
                    opened = web(it.window.decorView) != null && it.findViewById<View>(R.id.rua_runtime_setup) == null
                }
                SystemClock.sleep(100)
            }
            assertTrue("Ready tools must automatically reveal the console", opened)
            val log = File(context.filesDir, "runtime/install.log").readText()
            assertTrue(log.contains("RUA_DEBIAN_PYTHON_READY"))
            assertTrue(log.contains("RUA_VENV_READY"))
            assertTrue("Materials downloaded by Android UID", LinuxRuntimeInstaller.manifest(context).artifacts.all {
                LinuxRuntimeInstaller.validCached(File(LinuxRuntimeInstaller.cache(context), it.sha256), it)
            })
            LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/first-install.json"),
                org.json.JSONObject().put("elapsed_ms", SystemClock.elapsedRealtime() - started)
                    .put("download_bytes", RuntimeMirrorDownloader.receivedBytes.get())
                    .put("requests", RuntimeMirrorDownloader.requests.get()).toString())
        }
    }
}
