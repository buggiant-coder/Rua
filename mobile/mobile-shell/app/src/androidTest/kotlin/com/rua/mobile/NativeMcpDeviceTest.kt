package com.rua.mobile

import android.content.Intent
import android.os.SystemClock
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

@RunWith(AndroidJUnit4::class)
class NativeMcpDeviceTest {
    private val fixture = NativeAgentDeviceTest()
    private fun await(message: String, seconds: Int = 15, condition: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + seconds * 1000
        while (!condition() && SystemClock.elapsedRealtime() < end) SystemClock.sleep(50)
        assertTrue(message, condition())
    }
    private fun pids(file: File): List<String> = runCatching { file.readText().trim().split(' ').filter { it.toIntOrNull() != null } }.getOrDefault(emptyList())
    private fun gone(pids: List<String>) = pids.all { !File("/proc/$it").exists() }

    @Test fun normalMcpApiUsesGuestAndReportsUnavailableRuntimeWithoutHostFallback() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        assertTrue(RuntimeToolchains.ready(context))
        val workspace = File(context.filesDir, "home/.rua/workspaces/default")
        LinuxRuntimeInstaller.atomicText(File(workspace, "native-mcp-fixture.js"), instrumentation.context.assets.open("native-mcp-fixture.js").bufferedReader().use { it.readText() })
        LinuxRuntimeInstaller.atomicText(File(workspace, "native-mcp-input"), "canonical workspace")
        val serverMarker = File(workspace, "native-mcp-server-pids")
        val callMarker = File(workspace, "native-mcp-call-pids")
        val key = "native-runtime-mcp-${System.currentTimeMillis()}"
        ActivityScenario.launch(MainActivity::class.java).use {
            var token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Private handshake missing")
            fun api(method: String, path: String, body: JSONObject? = null) = fixture.api(token, method, path, body?.toString())
            val base = "/agents/default/mcp/$key"
            val config = JSONObject().put("key", key).put("name", key).put("enabled", true).put("transport", "stdio")
                .put("command", "node").put("args", org.json.JSONArray().put("native-mcp-fixture.js"))
                .put("env", JSONObject().put("MCP_FIXTURE_SECRET", "fixture-secret")).put("call_timeout_secs", 2)
                .put("auto_connect", "lazy_on_first_use")
            await("Resident ready", 30) { runCatching { api("GET", "/agents/default/chats"); true }.getOrDefault(false) }
            val previousFixtures = api("GET", "/agents/default/mcp").getJSONArray("items")
            for (index in 0 until previousFixtures.length()) {
                val entry = previousFixtures.getJSONObject(index)
                if (entry.optString("key").startsWith("native-runtime-mcp-") && entry.optJSONArray("args")?.optString(0) == "native-mcp-fixture.js") {
                    api("DELETE", "/agents/default/mcp/${entry.getString("key")}")
                }
            }
            api("POST", "/agents/default/mcp", config)
            var primaryFailure: Throwable? = null
            try {
                val tools = api("GET", "$base/tools").toString()
                assertTrue(tools, tools.contains("probe") && tools.contains("wait"))
                val result = api("POST", "$base/call", JSONObject().put("tool", "probe").put("arguments", JSONObject())).toString()
                assertTrue(result, result.contains("NATIVE_MCP_OK") && result.contains("linux arm64") && result.contains("canonical workspace"))
                val output = JSONObject(File(workspace, "native-mcp-output.json").readText())
                assertTrue(output.getString("cwd").endsWith("home/.rua/workspaces/default"))
                val saved = api("GET", base)
                assertEquals("node", saved.getString("command"))
                assertFalse(saved.toString().contains("/usr/local") || saved.toString().contains("runtime/debian"))
                val serverPids = pids(serverMarker)
                assertEquals(2, serverPids.size)
                RuntimeGateDeviceTest.assertUpdateBusy(context)
                callMarker.delete()
                File(workspace, "native-mcp-calls").delete()
                val timeout = runCatching { api("POST", "$base/call", JSONObject().put("tool", "wait").put("arguments", JSONObject())) }.exceptionOrNull()
                assertNotNull("Waiting tool must time out", timeout)
                assertTrue(timeout!!.message.orEmpty(), timeout.message.orEmpty().contains("timeout", true) || timeout.message.orEmpty().contains("timed out", true))
                await("Actual request children started") { pids(callMarker).size == 2 }
                val callPids = pids(callMarker)
                await("Cancellation notification cleans request descendants") { gone(callPids) }
                assertEquals("A timed-out request must never be replayed", listOf("wait"), File(workspace, "native-mcp-calls").readLines())
                assertTrue("A cancelled request must preserve its shared server", serverPids.all { File("/proc/$it").exists() })
                api("PUT", base, JSONObject().put("enabled", false))
                await("Disable cleans server subtree") { gone(serverPids) }

                val root = File(context.filesDir, "runtime/debian")
                val sentinel = File(workspace, "host-sentinel").apply { mkdirs() }
                LinuxRuntimeInstaller.atomicText(File(sentinel, "node"), "#!/system/bin/sh\necho executed > '${File(workspace, "host-fallback-ran").path}'\n")
                android.system.Os.chmod(File(sentinel, "node").path, 0x1ed)
                for ((relative, reason) in listOf("usr/bin/env" to "Linux Compatibility Runtime", "usr/local/bin/node" to "node")) {
                    val command = File(root, relative)
                    val backup = File(command.parentFile, command.name + ".native-mcp-fixture")
                    check(command.renameTo(backup))
                    try {
                        val rejected = api("PUT", base, JSONObject().put("enabled", true).put("env", JSONObject().put("MCP_FIXTURE_SECRET", "fixture-secret").put("PATH", sentinel.path)))
                        assertEquals("failed", rejected.getJSONObject("application").getString("status"))
                        val status = api("GET", "/mcp/status").toString()
                        assertTrue(status, status.contains(key) && status.contains(reason))
                        val error = runCatching { api("GET", "$base/tools") }.exceptionOrNull()
                        assertTrue(error?.message.orEmpty(), error?.message.orEmpty().contains(reason))
                        assertFalse(File(workspace, "host-fallback-ran").exists())
                        api("PUT", base, JSONObject().put("enabled", false))
                    } finally { check(backup.renameTo(command)) }
                }
                val invalid = api("PUT", base, JSONObject().put("enabled", true).put("cwd", "missing-mcp-cwd"))
                assertEquals("failed", invalid.getJSONObject("application").getString("status"))
                assertTrue(api("GET", "/mcp/status").toString().contains("Working directory"))
                api("PUT", base, JSONObject().put("enabled", true).put("cwd", JSONObject.NULL).put("call_timeout_secs", 30)
                    .put("env", JSONObject().put("MCP_FIXTURE_SECRET", "fixture-secret")))
                assertTrue(api("GET", "$base/tools").toString().contains("probe"))
                assertTrue(api("POST", "$base/call", JSONObject().put("tool", "probe")).toString().contains("NATIVE_MCP_OK"))

                // A real Agent Stop drops the in-flight call; the protocol still cancels its children.
                val provider = "mcp-runtime-fixture-${System.currentTimeMillis()}"
                api("POST", "/models/custom-providers", JSONObject("""{"id":"$provider","name":"MCP fixture","base_url":"http://127.0.0.1:18099/v1","api_key":"fixture-key","chat_model":"OpenAIChatModel","models":[{"id":"native-runtime-model","name":"MCP fixture"}]}"""))
                val previous = api("GET", "/models/active")
                try {
                    api("PUT", "/models/active", JSONObject().put("provider_id", provider).put("model", "native-runtime-model"))
                    val chat = api("POST", "/agents/default/chats", JSONObject("""{"name":"MCP stop","user_id":"native-e2e","channel":"console","meta":{},"model":{"provider_id":"$provider","model":"native-runtime-model","thinking_strength":"off"}}"""))
                    val id = chat.getString("id")
                    callMarker.delete()
                    val submit = api("POST", "/agents/default/chats/$id/console/submit", JSONObject().put("message", "native-runtime-mcp-stop"))
                    await("Agent MCP descendants started", 30) { fixture.approveFixtureCalls(token, id); pids(callMarker).size == 2 }
                    val children = pids(callMarker)
                    val active = submit.getJSONObject("control").getJSONObject("active_turn")
                    assertTrue(api("POST", "/agents/default/chats/$id/console/stop", JSONObject().put("turn_id", active.getString("turn_id")).put("authority_epoch", active.getString("authority_epoch"))).getBoolean("stopped"))
                    await("Agent Stop cancels MCP request subtree") { gone(children) }
                } finally {
                    if (previous.optString("provider_id").isEmpty()) api("DELETE", "/models/active")
                    else api("PUT", "/models/active", previous.getJSONObject("default_llm"))
                }
                val residentChildren = pids(serverMarker)
                context.stopService(Intent(context, NativeRuntimeService::class.java))
                await("Resident stop cleans MCP server subtree", 25) { gone(residentChildren) }
            } catch (error: Throwable) {
                primaryFailure = error
                throw error
            } finally {
                // Reopen the owner only to remove this test's saved configuration.
                runCatching {
                    androidx.core.content.ContextCompat.startForegroundService(context, Intent(context, NativeRuntimeService::class.java))
                    await("Resident restarted for fixture cleanup", 30) { runCatching { token = PhoneUseBridge.awaitAuthToken(100) ?: token; api("GET", "/agents/default/mcp"); true }.getOrDefault(false) }
                    api("DELETE", base)
                }.onFailure { cleanup -> if (primaryFailure != null) primaryFailure.addSuppressed(cleanup) else throw cleanup }
            }
        }
    }
}
