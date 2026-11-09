package com.rua.mobile

import android.app.NotificationManager
import android.app.UiAutomation
import android.content.ComponentName
import android.content.Intent
import android.os.SystemClock
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.net.URL

@RunWith(AndroidJUnit4::class)
class NativeHostDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val fixture = NativeAgentDeviceTest()
    private val automation get() = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    private fun shell(command: String): String = android.os.ParcelFileDescriptor.AutoCloseInputStream(
        automation.executeShellCommand(command)).bufferedReader().use { it.readText().trim() }
    private fun await(message: String, seconds: Int = 30, check: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + seconds * 1000L
        while (SystemClock.elapsedRealtime() < end) {
            if (check()) return
            SystemClock.sleep(50)
        }
        error(message)
    }
    private fun readyToken(): String {
        var token = ""
        await("Native resident did not become ready") {
            token = PhoneUseBridge.awaitAuthToken(100) ?: token
            runCatching { fixture.api(token, "GET", "/platform/foreground-operations"); true }.getOrDefault(false)
        }
        return token
    }
    private fun operations(token: String) = fixture.api(token, "GET", "/platform/foreground-operations")

    @Test fun nativeHostAppliesRealLifecyclePowerPolicyAndNetworkFacts() {
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            val token = readyToken()
            val originalPolicy = fixture.api(token, "GET", "/sync/power-policy").optJSONObject("stored_policy")
            val resident = MobileRuntimeProcess.findRuaServerPids()
            val syncWasEnabled = fixture.api(token, "GET", "/sync/status").getBoolean("enabled")
            var createdNetwork = false
            assertEquals(1, resident.size)
            val terminal = fixture.api(token, "POST", "/terminal/sessions", JSONObject()
                .put("execution_device", JSONObject().put("kind", "local"))
                .put("browser_viewer_id", "native-power-viewer").put("scrollback_lines", 2000)
                .put("cwd", "${MainActivity.HOME_DIR}/.rua/workspaces/default").toString())
                .getJSONObject("session").getString("session_id")
            fixture.api(token, "POST", "/terminal/sessions/$terminal/detach", """{"browser_viewer_id":"native-power-viewer"}""")
            try {
                fixture.api(token, "POST", "/sync/enable")
                createdNetwork = fixture.api(token, "POST", "/sync/network/create").getBoolean("created")
                fixture.api(token, "PUT", "/sync/power-policy", """{"mode":"power_saver"}""")
                await("Foreground state missing") { !operations(token).getBoolean("background") }
                scenario.moveToState(Lifecycle.State.CREATED)
                await("Background lifecycle not delivered") { operations(token).getBoolean("background") }
                assertEquals("background_grace", operations(token).getString("residency_state"))
                assertEquals("Detached terminal is not an activity lease", 0, operations(token).getInt("active_count"))
                await("Service did not acquire Android wake lock") { shell("dumpsys power").lineSequence().any { it.contains("PARTIAL_WAKE_LOCK") && it.contains("rua:resident") } }
                await("Production grace did not suspend network", 80) { operations(token).getString("residency_state") == "suspended" }
                assertFalse(operations(token).getBoolean("locks_required"))
                assertEquals("suspended", fixture.api(token, "GET", "/platform/device-network-availability").getString("state"))
                await("Wake lock remained after suspension") { !shell("dumpsys power").lineSequence().any { it.contains("PARTIAL_WAKE_LOCK") && it.contains("rua:resident") } }
                val notifications = context.getSystemService(NotificationManager::class.java).activeNotifications
                assertEquals(1, notifications.count { it.id == 1201 })
                assertEquals("手Rua", notifications.first { it.id == 1201 }.notification.extras.getString("android.title"))
                scenario.moveToState(Lifecycle.State.RESUMED)
                await("Foreground did not resume network") { operations(token).getString("residency_state") == "foreground" }
                await("Device Network did not become ready after foreground") {
                    fixture.api(token, "GET", "/platform/device-network-availability").getString("state") == "ready"
                }
                assertEquals(resident, MobileRuntimeProcess.findRuaServerPids())
                fixture.api(token, "PUT", "/sync/power-policy", """{"mode":"realtime"}""")
                scenario.moveToState(Lifecycle.State.CREATED)
                await("Realtime did not retain resident locks") {
                    val state = operations(token)
                    state.getBoolean("background") && state.getBoolean("locks_required") && state.getString("residency_state") == "resident_idle"
                }
                await("Realtime wake lock missing") { shell("dumpsys power").lineSequence().any { it.contains("PARTIAL_WAKE_LOCK") && it.contains("rua:resident") } }
                scenario.moveToState(Lifecycle.State.RESUMED)
                val dns = File(context.filesDir, "runtime/debian/etc/resolv.conf").readText()
                assertTrue(dns.contains("nameserver "))
                val https = URL("https://deb.debian.org/debian/dists/stable/Release").openConnection() as javax.net.ssl.HttpsURLConnection
                try {
                    https.connectTimeout = 15000; https.readTimeout = 15000
                    assertEquals(200, https.responseCode)
                    assertTrue(https.inputStream.bufferedReader().use { it.readLine() }.isNotBlank())
                } finally { https.disconnect() }
                assertEquals(android.os.Process.myUid(), context.applicationInfo.uid)
                val initRoot = File(context.cacheDir, "phone-catalog-${System.nanoTime()}")
                try {
                    val init = NativePayload.process(context, listOf("init", "--rua-root", initRoot.path)).start()
                    assertTrue(init.waitFor(30, java.util.concurrent.TimeUnit.SECONDS))
                    assertEquals(0, init.exitValue())
                    val tools = JSONObject(File(initRoot, "workspaces/default/agent.json").readText()).getJSONObject("tools").getJSONObject("builtin_tools")
                    assertTrue("First-install init must seed Phone Use before the handshake", tools.has("phone_observe"))
                } finally { initRoot.deleteRecursively() }
            } finally {
                fixture.api(token, "DELETE", "/terminal/sessions/$terminal?browser_viewer_id=native-power-viewer")
                fixture.api(token, "PUT", "/sync/power-policy", originalPolicy?.toString() ?: """{"mode":"power_saver"}""")
                if (createdNetwork && !syncWasEnabled) fixture.api(token, "POST", "/sync/disable")
            }
        }
    }

    @Test fun realAgentUsesPhoneBridgeAndRevocationDoesNotStopResident() {
        val oldServices = shell("settings get secure enabled_accessibility_services")
        val oldEnabled = shell("settings get secure accessibility_enabled")
        try {
            automation.serviceInfo = automation.serviceInfo.apply {
                flags = flags or android.accessibilityservice.AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
            }
            shell("settings put secure enabled_accessibility_services com.rua.mobile/com.rua.mobile.MobileAccessibilityService")
            shell("settings put secure accessibility_enabled 1")
            await("Accessibility service not connected") { MobileAccessibilityService.instance != null }
            ActivityScenario.launch(MainActivity::class.java).use {
                val token = readyToken()
                val resident = MobileRuntimeProcess.findRuaServerPids()
                val provider = "native-phone-fixture-${System.currentTimeMillis()}"
                val previous = fixture.api(token, "GET", "/models/active")
                val agentId = "phone-fixture-${System.currentTimeMillis()}"
                fixture.api(token, "POST", "/agents", JSONObject().put("id", agentId).put("name", "Native Phone acceptance").put("workspace_dir", "${MainActivity.HOME_DIR}/.rua/workspaces/$agentId").toString())
                fun api(method: String, path: String, body: String? = null) = fixture.api(token, method, path.replace("/agents/default", "/agents/$agentId"), body)
                assertTrue(api("GET", "/agents/default/tools").toString().contains("phone_observe"))
                fixture.api(token, "POST", "/models/custom-providers", """{"id":"$provider","name":"Phone Fixture","base_url":"http://127.0.0.1:18099/v1","api_key":"fixture-key","chat_model":"OpenAIChatModel","models":[{"id":"native-runtime-model","name":"Phone Fixture"}]}""")
                try {
                    fixture.api(token, "PUT", "/models/active", """{"provider_id":"$provider","model":"native-runtime-model"}""")
                    for (mode in listOf("phone", "phone-terminate", "phone-stop", "phone-denied")) {
                        context.startActivity(Intent().setComponent(ComponentName("com.rua.mobile.test", "com.rua.mobile.PhoneFixtureJavaActivity")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
                        instrumentation.waitForIdleSync()
                        SystemClock.sleep(1000)
                        if (mode == "phone-denied") {
                            shell("settings put secure enabled_accessibility_services null")
                            shell("settings put secure accessibility_enabled 0")
                            await("Accessibility revocation not observed") { MobileAccessibilityService.instance == null }
                        }
                        val chat = api("POST", "/agents/default/chats", """{"name":"$mode","user_id":"native-host-e2e","channel":"console","meta":{},"model":{"provider_id":"$provider","model":"native-runtime-model","thinking_strength":"off"}}""")
                        val id = chat.getString("id")
                        api("POST", "/agents/default/chats/$id/console/submit", """{"message":"native-runtime-$mode"}""")
                        var terminated = false
                        var snapshot = ""
                        await("$mode did not finish: $snapshot", 60) {
                            val pending = api("GET", "/agents/default/approvals/snapshot?chat_id=$id").optJSONArray("pending")
                            if (pending != null) for (index in 0 until pending.length()) {
                                val approvalId = pending.getJSONObject(index).getString("id")
                                api("POST", "/agents/default/approvals/$approvalId/decide", """{"decision":"run_once","decided_by":"native-phone-fixture"}""")
                            }
                            val op = PhoneDeviceControl.state.current()
                            if (!terminated && op != null && op.submitted.get() && mode in listOf("phone-terminate", "phone-stop")) {
                                if (mode == "phone-terminate") {
                                    val windows = automation.windows
                                    val stop = windows.asSequence().mapNotNull { it.root }.flatMap { it.findAccessibilityNodeInfosByText("终止").asSequence() }.firstOrNull()
                                    if (stop != null) {
                                        assertTrue(stop.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK))
                                        terminated = true
                                    }
                                } else {
                                    val history = api("GET", "/agents/default/chats/$id")
                                    val messages = history.getJSONArray("messages")
                                    val turn = (0 until messages.length()).map { messages.getJSONObject(it) }.last { it.optString("turn_status") == "in_progress" }
                                    val epoch = history.getJSONObject("meta").getJSONObject("authority_placement").getString("authority_epoch")
                                    val body = JSONObject().put("turn_id", turn.getString("turn_id")).put("authority_epoch", epoch)
                                    assertTrue(api("POST", "/agents/default/chats/$id/console/stop", body.toString()).getBoolean("stopped"))
                                    terminated = true
                                }
                            }
                            snapshot = api("GET", "/agents/default/chats/$id").toString()
                            if (snapshot.contains("NATIVE_AGENT_FAILED")) error("$mode: " + snapshot.substringAfterLast("NATIVE_AGENT_FAILED").take(2000))
                            if (mode == "phone-stop") terminated && PhoneDeviceControl.state.current() == null else snapshot.contains("NATIVE_AGENT_ALL_PASS")
                        }
                        if (mode in listOf("phone-terminate", "phone-stop")) assertTrue("Actual operation never terminated", terminated)
                        assertEquals(resident, MobileRuntimeProcess.findRuaServerPids())
                        assertTrue(api("GET", "/platform/foreground-operations").has("revision"))
                    }
                } finally {
                    fixture.api(token, "DELETE", "/agents/$agentId")
                    if (previous.optString("provider_id").isEmpty()) fixture.api(token, "DELETE", "/models/active")
                    else fixture.api(token, "PUT", "/models/active", previous.getJSONObject("default_llm").toString())
                }
            }
        } finally {
            shell("settings put secure enabled_accessibility_services '$oldServices'")
            shell("settings put secure accessibility_enabled '$oldEnabled'")
        }
    }
}
