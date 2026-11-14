package com.rua.mobile

import android.app.UiAutomation
import android.content.ComponentName
import android.content.Intent
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import android.view.accessibility.AccessibilityWindowInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.net.HttpURLConnection
import java.net.URL

/** Uses the gated local model fixture and a disposable emulator, never a shopping app. */
@RunWith(AndroidJUnit4::class)
@Suppress("DEPRECATION") // Accessibility nodes still require recycling on supported pre-33 devices.
class PhoneUseSessionDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val fixture = NativeAgentDeviceTest()
    private val automation get() = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    private lateinit var token: String
    private lateinit var model: JSONObject

    private fun shell(command: String) = android.os.ParcelFileDescriptor.AutoCloseInputStream(
        automation.executeShellCommand(command)).bufferedReader().use { it.readText().trim() }

    private fun await(message: String, seconds: Int = 45, check: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + seconds * 1000L
        while (SystemClock.elapsedRealtime() < end) {
            if (check()) return
            SystemClock.sleep(100)
        }
        error(message)
    }

    private fun api(method: String, path: String, body: JSONObject? = null) = fixture.api(token, method, path, body?.toString())

    private fun control(path: String, body: JSONObject? = null): JSONObject {
        val connection = URL("http://127.0.0.1:18099/control/$path").openConnection() as HttpURLConnection
        try {
            connection.connectTimeout = 5000
            connection.readTimeout = 5000
            if (body != null) {
                connection.requestMethod = "POST"
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray()) }
            }
            assertEquals("Model fixture control failed", 200, connection.responseCode)
            return JSONObject(connection.inputStream.bufferedReader().use { it.readText() })
        } finally { connection.disconnect() }
    }

    private fun visibleNode(description: String): AccessibilityNodeInfo? {
        for (window in automation.windows) {
            val root = window.root ?: continue
            try {
                val nodes = root.findAccessibilityNodeInfosByText(if (description == STOP) "停止" else description)
                var found: AccessibilityNodeInfo? = null
                for (node in nodes) {
                    if (found == null && node.isVisibleToUser && node.contentDescription?.toString() == description) found = node
                    else node.recycle()
                }
                if (found != null) return found
            } finally { root.recycle() }
        }
        return null
    }

    private fun surfaceVisible(): Boolean {
        val label = visibleNode(LABEL) ?: return false
        val stop = visibleNode(STOP)
        return try {
            val surfaces = automation.windows.filter { window ->
                val root = window.root ?: return@filter false
                try {
                    val nodes = root.findAccessibilityNodeInfosByText(LABEL)
                    try { nodes.any { it.isVisibleToUser && it.contentDescription?.toString() == LABEL } }
                    finally { nodes.forEach { it.recycle() } }
                } finally { root.recycle() }
            }
            stop != null && stop.windowId == label.windowId && surfaces.size == 1 &&
                surfaces.single().type == AccessibilityWindowInfo.TYPE_ACCESSIBILITY_OVERLAY
        } finally { label.recycle(); stop?.recycle() }
    }

    private fun assertSurface(expected: Boolean) {
        assertEquals("Native session state", expected, api("GET", "/platform/phone").getBoolean("session_active"))
        if (expected) assertTrue("Real accessibility overlay missing", surfaceVisible())
        else assertFalse("Native session surface remains", indicatorPresent())
    }

    private fun indicatorPresent(): Boolean {
        val label = visibleNode(LABEL)
        val stop = visibleNode(STOP)
        return try { label != null || stop != null }
        finally { label?.recycle(); stop?.recycle() }
    }

    private fun persistFor(milliseconds: Long) {
        val end = SystemClock.elapsedRealtime() + milliseconds
        while (SystemClock.elapsedRealtime() < end) {
            assertSurface(true)
            SystemClock.sleep(200)
        }
    }

    private fun approve(agent: String, chat: String) {
        val pending = api("GET", "/agents/$agent/approvals/snapshot?chat_id=$chat").optJSONArray("pending") ?: return
        for (index in 0 until pending.length()) {
            val item = pending.getJSONObject(index)
            val name = item.optString("tool_name", item.optString("tool"))
            assertTrue("Unexpected fixture tool approval: $name", name in listOf("phone_status", "phone_list_apps", "phone_observe", "phone_release"))
            api("POST", "/agents/$agent/approvals/${item.getString("id")}/decide", JSONObject()
                .put("decision", "run_once").put("decided_by", "phone-session-device-fixture"))
        }
    }

    private fun history(agent: String, chat: String) = api("GET", "/agents/$agent/chats/$chat")
    private fun messages(history: JSONObject): List<JSONObject> {
        val rows = history.getJSONArray("messages")
        return (0 until rows.length()).map { rows.getJSONObject(it) }
    }

    private fun newChat(agent: String) = api("POST", "/agents/$agent/chats", JSONObject()
        .put("name", "Phone session fixture").put("user_id", "phone-session-device-fixture")
        .put("channel", "console").put("meta", JSONObject()).put("model", model)).getString("id")

    private fun submit(agent: String, chat: String, case: String, mode: String): String {
        val result = api("POST", "/agents/$agent/chats/$chat/console/submit", JSONObject()
            .put("message", "native-phone-session:$case:$mode").put("model", model))
        assertTrue(result.getBoolean("accepted"))
        return case
    }

    private fun held(agent: String, chat: String, case: String, observationExpected: Boolean = true) {
        await("Observation did not reach the model hold: $case") {
            approve(agent, chat)
            val state = control("state").optJSONObject(case) ?: return@await false
            if (!state.getBoolean("held")) return@await false
            val events = state.getJSONArray("events")
            val observation = events.getJSONObject(events.length() - 1)
            if (observationExpected) {
                assertTrue("Actual structured observation required", observation.getBoolean("observation_valid"))
                assertTrue(observation.getBoolean("screenshot_absent"))
            } else assertEquals("New Turn must still be in its first model call", 0, observation.getInt("tool_results"))
            true
        }
        await("Session surface missing after observation") { surfaceVisible() }
        assertNull("A model hold must not keep an operation in flight", PhoneDeviceControl.state.current())
        assertSurface(true)
    }

    private fun continueCase(case: String) { control("continue", JSONObject().put("case", case)) }

    private fun completed(agent: String, chat: String, case: String, absent: Boolean = false) {
        await("Agent did not complete: $case") {
            approve(agent, chat)
            if (absent) assertSurface(false)
            val current = history(agent, chat)
            assertFalse("Fixture rejected the result", current.toString().contains("PHONE_SESSION_FAILED"))
            val rows = messages(current)
            val turn = rows.lastOrNull()?.optString("turn_id")
            val currentRows = rows.filter { it.optString("turn_id") == turn }
            assertFalse("Agent Turn failed", currentRows.any { it.optString("turn_status") == "failed" })
            currentRows.any { it.toString().contains("PHONE_SESSION_PASS") } &&
                currentRows.isNotEmpty() && currentRows.all { it.optString("turn_status") == "completed" }
        }
    }

    private fun release(agent: String, chat: String, case: String) {
        submit(agent, chat, case, "release-only")
        completed(agent, chat, case)
        await("Release did not remove the overlay", 3) { !indicatorPresent() }
        assertSurface(false)
    }

    private fun stopHeldTurn(agent: String, chat: String, case: String, lateTool: String) {
        val turnId = messages(history(agent, chat)).last().getString("turn_id")
        val countBefore = messages(history(agent, chat)).count { it.optString("turn_id") == turnId && it.optString("type") == "plugin_call" }
        val stop = visibleNode(STOP) ?: error("Native Stop button missing")
        try { assertTrue("Native Stop click failed", stop.performAction(AccessibilityNodeInfo.ACTION_CLICK)) }
        finally { stop.recycle() }
        await("Stop did not cancel the current Turn") {
            val rows = messages(history(agent, chat)).filter { it.optString("turn_id") == turnId }
            rows.any { it.optString("turn_status") == "interrupted" } && rows.none { it.optString("turn_status") == "in_progress" } &&
                !api("GET", "/platform/phone").getBoolean("session_active") && !indicatorPresent()
        }
        continueCase(case)
        SystemClock.sleep(2000)
        val lateEvents = control("state").getJSONObject(case).getJSONArray("events")
        assertEquals("Fixture must attempt the delayed response", lateTool, lateEvents.getJSONObject(lateEvents.length() - 1).getString("requested_tool"))
        val after = messages(history(agent, chat)).filter { it.optString("turn_id") == turnId }
        assertEquals("Canceled Turn dispatched a later tool", countBefore, after.count { it.optString("type") == "plugin_call" })
        assertEquals("Canceled Turn left a pending approval", 0,
            api("GET", "/agents/$agent/approvals/snapshot?chat_id=$chat").getJSONArray("pending").length())
        assertSurface(false)
    }

    @Test fun realAgentSessionPersistsAcrossThinkingCompletionAndStops() {
        assumeTrue("Disposable emulator only", android.os.Build.HARDWARE in listOf("ranchu", "goldfish"))
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
                await("Native resident did not become ready") {
                    token = PhoneUseBridge.awaitAuthToken(100) ?: return@await false
                    runCatching { api("GET", "/platform/phone"); true }.getOrDefault(false)
                }
                val originalModel = api("GET", "/models/active").toString()
                val originalAgents = api("GET", "/agents").toString()
                val resident = MobileRuntimeProcess.findRuaServerPids()
                val suffix = System.currentTimeMillis().toString()
                val provider = "phone-session-provider-$suffix"
                val agents = listOf("phone-session-a-$suffix", "phone-session-b-$suffix")
                val createdAgents = mutableListOf<String>()
                var providerCreated = false
                model = JSONObject().put("provider_id", provider).put("model", "phone-session-e2e").put("thinking_strength", "off")
                try {
                    api("POST", "/models/custom-providers", JSONObject().put("id", provider).put("name", "Phone session fixture")
                        .put("base_url", "http://127.0.0.1:18099/v1").put("api_key", "fixture-key").put("chat_model", "OpenAIChatModel")
                        .put("models", JSONArray().put(JSONObject().put("id", "phone-session-e2e").put("name", "Phone session fixture"))))
                    providerCreated = true
                    for (agent in agents) {
                        api("POST", "/agents", JSONObject().put("id", agent).put("name", "Phone session fixture")
                            .put("workspace_dir", "${MainActivity.HOME_DIR}/.rua/workspaces/$agent"))
                        createdAgents.add(agent)
                    }
                    context.startActivity(Intent().setComponent(ComponentName("com.rua.mobile.test", "com.rua.mobile.PhoneFixtureJavaActivity"))
                        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK))
                    instrumentation.waitForIdleSync()
                    SystemClock.sleep(1000)
                    assertSurface(false)
                    val a = agents[0]
                    val b = agents[1]
                    val chatA = newChat(a)
                    val chatB = newChat(b)
                    submit(a, chatA, "neutral-$suffix", "neutral")
                    completed(a, chatA, "neutral-$suffix", absent = true)

                    val explicitRelease = submit(a, chatA, "release-$suffix", "release")
                    held(a, chatA, explicitRelease)
                    persistFor(5000)
                    continueCase(explicitRelease)
                    completed(a, chatA, explicitRelease)
                    await("Explicit release left the overlay", 3) { !indicatorPresent() }
                    assertSurface(false)

                    val retained = submit(a, chatA, "retain-$suffix", "retain")
                    held(a, chatA, retained)
                    submit(b, chatB, "busy-$suffix", "contender")
                    completed(b, chatB, "busy-$suffix")
                    assertSurface(true)
                    continueCase(retained)
                    completed(a, chatA, retained)
                    persistFor(3000)

                    // The existing session must bind Stop to this new Turn, rather than the completed Turn.
                    val inherited = submit(a, chatA, "inherited-$suffix", "stop-inherited")
                    held(a, chatA, inherited, observationExpected = false)
                    stopHeldTurn(a, chatA, inherited, "phone_observe")

                    val stopped = submit(a, chatA, "stop-$suffix", "stop")
                    held(a, chatA, stopped)
                    stopHeldTurn(a, chatA, stopped, "phone_click")
                    val counter = automation.windows.asSequence().mapNotNull { it.root }
                        .flatMap { root -> try { root.findAccessibilityNodeInfosByText("clicks=0").asSequence().toList().asSequence() } finally { root.recycle() } }.toList()
                    try { assertTrue("Delayed click changed the harmless target", counter.any { it.text?.toString() == "clicks=0" }) }
                    finally { counter.forEach { it.recycle() } }
                    assertSurface(false)

                    // The former contender can acquire only after A's session is gone.
                    val acquired = submit(b, chatB, "handoff-$suffix", "retain")
                    held(b, chatB, acquired)
                    continueCase(acquired)
                    completed(b, chatB, acquired)
                    assertSurface(true)
                    release(b, chatB, "cleanup-$suffix")
                    assertEquals("Session operations restarted the resident", resident, MobileRuntimeProcess.findRuaServerPids())
                } finally {
                    val cleanupFailures = mutableListOf<Throwable>()
                    for (agent in createdAgents) runCatching { api("DELETE", "/agents/$agent") }.exceptionOrNull()?.let { cleanupFailures.add(it) }
                    if (providerCreated) runCatching { api("DELETE", "/models/providers/$provider") }.exceptionOrNull()?.let { cleanupFailures.add(it) }
                    assertEquals("Global model changed", originalModel, api("GET", "/models/active").toString())
                    assertEquals("Original Agent inventory changed", originalAgents, api("GET", "/agents").toString())
                    assertTrue("Fixture resource cleanup failed: ${cleanupFailures.map { it.message }}", cleanupFailures.isEmpty())
                }
            }
        } finally {
            shell("settings put secure enabled_accessibility_services $oldServices")
            shell("settings put secure accessibility_enabled $oldEnabled")
        }
    }

    companion object {
        private const val LABEL = "Rua 正在操作手机"
        private const val STOP = "停止当前 Phone Use 会话"
    }
}
