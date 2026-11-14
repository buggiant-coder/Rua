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
import java.net.HttpURLConnection
import java.net.URL

@RunWith(AndroidJUnit4::class)
class NativeAgentDeviceTest {
    internal fun api(token: String, method: String, path: String, body: String? = null, binary: ByteArray? = null, timeoutMillis: Int = 10000): JSONObject {
        val port = PhoneUseBridge.awaitResidentPort(5000) ?: error("Native resident has not published its HTTP port")
        val connection = URL("http://127.0.0.1:$port/api$path").openConnection() as HttpURLConnection
        try {
            connection.requestMethod = method
            connection.connectTimeout = 5000
            connection.readTimeout = timeoutMillis
            connection.setRequestProperty("Authorization", "Bearer $token")
            if (body != null || binary != null) {
                connection.doOutput = true
                connection.setRequestProperty("Content-Type", if (binary != null) "application/zip" else "application/json")
                connection.outputStream.use { it.write(binary ?: body!!.toByteArray()) }
            }
            val code = connection.responseCode
            val text = (if (code in 200..299) connection.inputStream else connection.errorStream).bufferedReader().use { it.readText() }
            check(code in 200..299) { "$method $path: $code $text" }
            return if (text.isBlank()) JSONObject() else if (text.trimStart().startsWith("[")) JSONObject().put("items", org.json.JSONArray(text)) else JSONObject(text)
        } finally { connection.disconnect() }
    }

    internal fun approveFixtureCalls(token: String, chatId: String) {
        val pending = api(token, "GET", "/agents/default/approvals/snapshot?chat_id=$chatId").optJSONArray("pending") ?: return
        for (index in 0 until pending.length()) {
            val id = pending.getJSONObject(index).getString("id")
            api(token, "POST", "/agents/default/approvals/$id/decide", """{"decision":"run_once","decided_by":"native-runtime-device-fixture"}""")
        }
    }

    @Test
    fun agentRunsShellPythonAndNestedScriptAgainstTheInstalledDebian() = runFlow(false)

    @Test
    fun uploadedSkillsInstallDependenciesAndExecuteInTheSameGuest() = runFlow(true)

    private fun runFlow(skills: Boolean) {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertTrue(LinuxRuntimeInstaller.ready(context))
        assertFalse(File(context.filesDir, "usr/bin/bash").exists())
        ActivityScenario.launch(MainActivity::class.java).use {
            val token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Private handshake missing")
            val provider = "native-runtime-fixture-${System.currentTimeMillis()}"
            val deadline = SystemClock.elapsedRealtime() + 30_000
            while (true) {
                try { api(token, "GET", "/agents/default/security/approval-mode"); break } catch (error: Exception) {
                    if (SystemClock.elapsedRealtime() > deadline) throw error
                    SystemClock.sleep(100)
                }
            }
            if (skills) {
                assertTrue(RuntimeToolchains.ready(context))
                val imported = api(token, "POST", "/agents/default/skills/upload?enable=true&overwrite=true", binary = File(context.filesDir, "runtime-test/skills.zip").readBytes())
                assertEquals(2, imported.getInt("count"))
                val listed = api(token, "GET", "/agents/default/skills").toString()
                assertTrue(listed.contains("native-python-fixture"))
                assertTrue(listed.contains("native-node-fixture"))
            }
            api(token, "POST", "/models/custom-providers", """{"id":"$provider","name":"Native Runtime Fixture","base_url":"http://127.0.0.1:18099/v1","api_key":"fixture-key","chat_model":"OpenAIChatModel","models":[{"id":"native-runtime-model","name":"Native Runtime Fixture"}]}""")
            val previousModel = api(token, "GET", "/models/active")
            try {
                api(token, "PUT", "/models/active", """{"provider_id":"$provider","model":"native-runtime-model"}""")
                val chat = api(token, "POST", "/agents/default/chats", """{"name":"Debian real Agent acceptance","user_id":"native-e2e","channel":"console","meta":{},"model":{"provider_id":"$provider","model":"native-runtime-model","thinking_strength":"off"}}""")
                val id = chat.getString("id")
                val submitted = api(token, "POST", "/agents/default/chats/$id/console/submit", JSONObject().put("message", if (skills) "native-runtime-skills" else "native-runtime-agent-check").toString())
                check(submitted.getBoolean("accepted"))
                val end = SystemClock.elapsedRealtime() + 180_000
                var snapshot = ""
                while (SystemClock.elapsedRealtime() < end) {
                    approveFixtureCalls(token, id)
                    snapshot = api(token, "GET", "/agents/default/chats/$id").toString()
                    if (snapshot.contains("NATIVE_AGENT_ALL_PASS") || snapshot.contains("NATIVE_AGENT_FAILED") || snapshot.contains("\"turn_status\":\"failed\"")) break
                    SystemClock.sleep(250)
                }
                assertTrue(snapshot.take(5000), snapshot.contains("NATIVE_AGENT_ALL_PASS"))
                if (skills) {
                    assertTrue("Inventory must report probed Node", snapshot.contains("v24.18.0"))
                    assertTrue("Inventory must report probed uv", snapshot.contains("0.13.7"))
                    val workspace = File(context.filesDir, "home/.rua/workspaces/default")
                    assertTrue(File(workspace, "python-skill-output.json").isFile)
                    assertTrue(File(workspace, "node-skill-output.json").isFile)
                    assertEquals("/usr/bin/python3", android.system.Os.readlink(File(workspace, "skills/native-python-fixture/.venv/bin/python").path))
                    assertTrue(File(workspace, "skills/native-python-fixture/.venv/lib/python3.13/site-packages/rua_fixture_dependency/__init__.py").isFile)
                    assertTrue(File(workspace, "skills/native-node-fixture/node_modules/rua-fixture-package/index.js").isFile)
                } else {
                val output = File(context.filesDir, "home/.rua/workspaces/default/native-output.json")
                assertTrue("Guest writes are visible in canonical host workspace", output.isFile)
                val result = JSONObject(output.readText())
                assertEquals(0, result.getInt("uid"))
                assertTrue(result.getString("cwd").endsWith("/.rua-python-sandbox"))
                }
                for (mode in if (skills) listOf("node-stop", "python-timeout") else listOf("python-stop", "python-timeout", "bash-stop")) {
                    val marker = File(context.filesDir, "guest-home/native-$mode-pids")
                    marker.delete()
                    val controlChat = api(token, "POST", "/agents/default/chats", """{"name":"$mode","user_id":"native-e2e","channel":"console","meta":{},"model":{"provider_id":"$provider","model":"native-runtime-model","thinking_strength":"off"}}""")
                    val controlId = controlChat.getString("id")
                    val controlSubmit = api(token, "POST", "/agents/default/chats/$controlId/console/submit", """{"message":"native-runtime-$mode"}""")
                    val startedDeadline = SystemClock.elapsedRealtime() + 20_000
                    fun childPids(): List<String> = runCatching {
                        marker.readText().trim().split(' ').filter { it.toIntOrNull() != null }
                    }.getOrDefault(emptyList())
                    while (childPids().size != 2 && SystemClock.elapsedRealtime() < startedDeadline) {
                        approveFixtureCalls(token, controlId)
                        SystemClock.sleep(50)
                    }
                    assertTrue("$mode descendants must actually start", marker.isFile)
                    val pids = childPids()
                    assertEquals(2, pids.size)
                RuntimeGateDeviceTest.assertUpdateBusy(context)
                    if (mode.endsWith("stop")) {
                        val active = controlSubmit.getJSONObject("control").getJSONObject("active_turn")
                        val stop = JSONObject().put("turn_id", active.getString("turn_id")).put("authority_epoch", active.getString("authority_epoch"))
                        assertTrue(api(token, "POST", "/agents/default/chats/$controlId/console/stop", stop.toString()).getBoolean("stopped"))
                    }
                    val cleanupDeadline = SystemClock.elapsedRealtime() + 15_000
                    while (pids.any { File("/proc/$it").exists() } && SystemClock.elapsedRealtime() < cleanupDeadline) SystemClock.sleep(100)
                    assertTrue("$mode left running descendants: $pids", pids.none { File("/proc/$it").exists() })
                }

            } finally {
                if (previousModel.optString("provider_id").isEmpty()) api(token, "DELETE", "/models/active")
                else api(token, "PUT", "/models/active", previousModel.getJSONObject("default_llm").toString())
            }
        }
    }
}
