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

/** Run on an isolated clean emulator; never clear a user's installed app for this test. */
@RunWith(AndroidJUnit4::class)
class NativeResidentDeviceTest {
    @Test
    fun assignedListenerAuthenticatesAndSurvivesRecreationWithOtherAppsListening() {
        java.net.ServerSocket(0, 1, java.net.InetAddress.getByName("127.0.0.1")).use { foreign ->
            val context = InstrumentationRegistry.getInstrumentation().targetContext
            val intent = android.content.Intent(context, MainActivity::class.java)
                .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK or android.content.Intent.FLAG_ACTIVITY_CLEAR_TASK)
            ActivityScenario.launch<MainActivity>(intent).use { scenario ->
                val port = PhoneUseBridge.awaitResidentPort(30_000) ?: error("Private endpoint missing")
                assertTrue(port in 1..65535)
                assertNotEquals("must not reuse another listener", foreign.localPort, port)
                val token = PhoneUseBridge.awaitAuthToken(1000) ?: error("Private handshake missing")
                await { status("/api/agents", token) == 200 }
                assertEquals(401, status("/api/agents", null))
                val pids = MobileRuntimeProcess.findRuaServerPids()
                assertEquals("one APK-owned resident", 1, pids.size)
                scenario.recreate()
                await { status("/api/agents", token) == 200 }
                assertEquals(port, MainActivity.RUA_PORT)
                assertEquals(pids, MobileRuntimeProcess.findRuaServerPids())
                assertFalse("unrelated listener is retained", foreign.isClosed)
            }
        }
    }

    @Test
    fun coldStartWithoutGuestAuthenticatesAndSurvivesActivityRecreation() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertFalse("fixture must not contain a host userland", File(context.filesDir, "usr/bin/bash").exists())
        assertFalse("fixture must not contain a guest", File(context.filesDir, "runtime/debian/usr/bin/bash").exists())
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            // Exercise Activity recreation while native initialization may still be running.
            scenario.recreate()
            val token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Private resident handshake missing")
            await { status("/api/agents", token) == 200 }
            assertEquals(401, status("/api/agents", null))
            val pids = MobileRuntimeProcess.findRuaServerPids()
            assertEquals("exactly one resident", 1, pids.size)
            assertEquals(PhoneUsePeerDecision.ACCEPT,
                PhoneUsePeerAuthenticator.authenticate(android.os.Process.myUid(), pids.single(),
                    expectedExe = NativePayload.executable(context)))
            scenario.recreate()
            await { status("/api/agents", token) == 200 }
            assertEquals("recreation must retain the same resident", pids, MobileRuntimeProcess.findRuaServerPids())
            await {
                var preparing = false
                scenario.onActivity { activity ->
                    assertNull("First-run console must not exist before tools are ready", findWebView(activity.window.decorView))
                    preparing = activity.findViewById<View>(R.id.rua_runtime_setup)?.isShown == true
                }
                preparing
            }

        }
        assertTrue(context.stopService(android.content.Intent(context, NativeRuntimeService::class.java)))
        await { MobileRuntimeProcess.findRuaServerPids().isEmpty() }
    }

    private fun status(path: String, token: String?): Int = runCatching {
        val connection = URL("http://127.0.0.1:${MainActivity.RUA_PORT}$path").openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 1000; connection.readTimeout = 1000
            connection.instanceFollowRedirects = false
            if (token != null) connection.setRequestProperty("Authorization", "Bearer $token")
            connection.responseCode
        } finally { connection.disconnect() }
    }.getOrDefault(-1)

    private fun await(check: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + 30_000
        while (SystemClock.elapsedRealtime() < deadline) {
            if (check()) return
            SystemClock.sleep(100)
        }
        error("Native resident did not become ready")
    }

    private fun findWebView(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (i in 0 until view.childCount) {
            findWebView(view.getChildAt(i))?.let { return it }
        }
        return null
    }
}
