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
class NativeAcpDeviceTest {
    private fun await(message: String, seconds: Int = 20, condition: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + seconds * 1000
        while (SystemClock.elapsedRealtime() < end) {
            if (condition()) return
            SystemClock.sleep(50)
        }
        fail(message)
    }
    private fun pids(file: File) = runCatching { file.readText().trim().split(' ').filter { it.toIntOrNull() != null } }.getOrDefault(emptyList())
    private fun gone(pids: List<String>) = pids.all { !File("/proc/$it").exists() }

    @Test fun installedRunnerChecksExecutesRequestsPermissionAndCancelsInGuest() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        assertTrue(RuntimeToolchains.ready(context))
        val workspace = File(context.filesDir, "home/.rua/workspaces/default")
        val npmrc = File(context.filesDir, "guest-home/.npmrc")
        val savedNpmrc = npmrc.takeIf { it.exists() }?.readText()
        val guestHome = File(context.filesDir, "guest-home")
        val installed = File(guestHome, ".local/lib/node_modules/@agentclientprotocol/codex-acp")
        val bin = File(guestHome, ".local/bin/codex-acp")
        // This test is only run on the task-owned AVD; preserve any prior fixture package.
        val backups = listOf(installed, bin).mapNotNull { file ->
            if (file.exists()) File(file.parentFile, file.name + ".native-acp-test-backup").also { check(!it.exists()); check(file.renameTo(it)) } to file else null
        }
        ActivityScenario.launch(MainActivity::class.java).use {
            var token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Private handshake missing")
            val helper = NativeAgentDeviceTest()
            fun api(method: String, path: String, body: JSONObject? = null) = helper.api(token, method, path, body?.toString())
            await("Resident ready", 30) { runCatching { api("GET", "/agents/default/acp"); true }.getOrDefault(false) }
            val runner = "/agents/default/acp/codex"
            val previous = api("GET", runner)
            val node = File(context.filesDir, "runtime/debian/usr/local/bin/node")
            val versionFile = File(node.canonicalFile.parentFile!!.parentFile!!.parentFile!!, ".rua-versions.json")
            val nodeMarker = versionFile.readText()
            var chatBase: String? = null
            var primaryFailure: Throwable? = null
            try {
                LinuxRuntimeInstaller.atomicText(npmrc, (savedNpmrc ?: "") + "\n@agentclientprotocol:registry=http://127.0.0.1:18100\n")
                api("PUT", runner, JSONObject().put("enabled", true).put("trusted", false).put("command", "codex-acp").put("args", org.json.JSONArray()).put("env", JSONObject()))
                val task = api("POST", "$runner/install/start")
                var taskState = JSONObject()
                await("Runner install finished: $task", 90) {
                    taskState = api("GET", "/agents/default/acp/install-tasks/${task.getString("task_id")}")
                    taskState.optString("status") in listOf("completed", "failed")
                }
                assertTrue(taskState.toString(), taskState.optBoolean("installed") && taskState.getJSONObject("check_result").getBoolean("ok"))
                assertEquals("Node toolchain must not be reinstalled by ACP", nodeMarker, versionFile.readText())
                assertTrue(api("GET", "$runner/check").getBoolean("ok"))
                val checkedPids = pids(File(workspace, "native-acp-server-pids"))
                assertEquals(2, checkedPids.size)
                await("Probe closes runner subtree") { gone(checkedPids) }
                val saved = api("GET", runner)
                assertEquals("codex-acp", saved.getString("command"))
                assertFalse(saved.toString().contains("runtime/debian") || saved.toString().contains("/usr/local"))

                val sentinel = File(workspace, "acp-host-sentinel").apply { mkdirs() }
                val executed = File(workspace, "acp-host-fallback-ran").apply { delete() }
                LinuxRuntimeInstaller.atomicText(File(sentinel, "codex-acp"), "#!/system/bin/sh\necho executed > '${executed.path}'\n")
                android.system.Os.chmod(File(sentinel, "codex-acp").path, 0x1ed)
                api("PUT", runner, JSONObject().put("env", JSONObject().put("PATH", sentinel.path)))
                for ((command, reason) in listOf(node to "node", File(context.filesDir, "runtime/debian/usr/bin/env") to "Linux Compatibility Runtime")) {
                    val backup = File(command.parentFile, command.name + ".native-acp-fixture")
                    check(command.renameTo(backup))
                    try {
                        val missing = api("GET", "$runner/check")
                        assertFalse(missing.toString(), missing.getBoolean("ok"))
                        assertTrue(missing.toString(), missing.getString("message").contains(reason))
                        assertFalse("Host command must not execute", executed.exists())
                    } finally { check(backup.renameTo(command)) }
                }
                api("PUT", runner, JSONObject().put("env", JSONObject()))
                assertTrue(api("GET", "$runner/check").getBoolean("ok"))
                val chat = api("POST", "/agents/default/chats", JSONObject("""{"name":"ACP native runtime","user_id":"native-e2e","channel":"console","meta":{}}"""))
                val base = "/agents/default/chats/${chat.getString("id")}"
                chatBase = base
                File(workspace, "native-acp-output.json").delete()
                api("POST", "$base/console/submit", JSONObject().put("message", "native-acp-probe").put("initial_acp", JSONObject().put("runner", "codex")))
                await("ACP actual guest execution") { File(workspace, "native-acp-output.json").exists() && api("GET", base).toString().contains("NATIVE_ACP_OK") }
                val serverPids = pids(File(workspace, "native-acp-server-pids"))
                RuntimeGateDeviceTest.assertUpdateBusy(context)
                assertEquals(2, serverPids.size)
                val output = JSONObject(File(workspace, "native-acp-output.json").readText())
                assertEquals(android.system.Os.stat(workspace.path).st_ino, android.system.Os.stat(output.getString("cwd")).st_ino)
                assertEquals("Linux aarch64", output.getString("python"))
                api("POST", "$base/console/submit", JSONObject().put("message", "native-acp-permission"))
                var permission = ""
                await("ACP permission relay") {
                    val pending = api("GET", "$base/acp/permissions").getJSONArray("pending")
                    if (pending.length() > 0) permission = pending.getJSONObject(0).getString("id")
                    permission.isNotEmpty()
                }
                api("POST", "$base/acp/permissions/$permission/decide", JSONObject().put("option_id", "allow_once"))
                await("Permission reached runner") { api("GET", base).toString().contains("NATIVE_ACP_PERMISSION_OK") }
                val calls = File(workspace, "native-acp-call-pids").apply { delete() }
                api("POST", "$base/console/submit", JSONObject().put("message", "native-acp-wait"))
                await("ACP request descendants") { pids(calls).size == 2 }
                val callPids = pids(calls)
                // A submit may have queued behind the preceding turn; read the running turn after its child starts.
                val history = api("GET", base)
                val messages = history.getJSONArray("messages")
                val turnId = (0 until messages.length()).map { messages.getJSONObject(it) }
                    .last { it.optString("turn_status") == "in_progress" }.getString("turn_id")
                val epoch = history.getJSONObject("meta").getJSONObject("authority_placement").getString("authority_epoch")
                val stopped = api("POST", "$base/console/stop", JSONObject().put("turn_id", turnId).put("authority_epoch", epoch))
                assertTrue(stopped.toString(), stopped.getBoolean("stopped"))
                await("ACP Stop cleans request subtree") { gone(callPids) }
                await("Stopped ACP turn finishes persisting before exit and re-entry") {
                    val settled = api("GET", base).getJSONArray("messages")
                    (0 until settled.length()).map { settled.getJSONObject(it) }
                        .none { it.optString("turn_id") == turnId && it.optString("turn_status") == "in_progress" }
                }
                assertTrue(serverPids.all { File("/proc/$it").exists() })
                api("POST", "$base/acp/exit")
                await("ACP exit cleans runner subtree") { gone(serverPids) }
                assertTrue(api("POST", "$base/acp/enter", JSONObject().put("runner", "codex")).getBoolean("ok"))
                val residentPids = pids(File(workspace, "native-acp-server-pids"))
                context.stopService(Intent(context, NativeRuntimeService::class.java))
                await("Resident Stop cleans ACP subtree", 25) { gone(residentPids) }
            } catch (error: Throwable) { primaryFailure = error; throw error }
            finally {
                runCatching {
                    androidx.core.content.ContextCompat.startForegroundService(context, Intent(context, NativeRuntimeService::class.java))
                    await("Resident ready for cleanup", 30) { runCatching { token = PhoneUseBridge.awaitAuthToken(100) ?: token; api("GET", "/agents/default/acp"); true }.getOrDefault(false) }
                    chatBase?.let { api("POST", "$it/acp/exit") }
                    api("PUT", runner, previous)
                }.onFailure { error -> if (primaryFailure != null) primaryFailure.addSuppressed(error) else throw error }
                if (savedNpmrc == null) npmrc.delete() else LinuxRuntimeInstaller.atomicText(npmrc, savedNpmrc)
                bin.delete(); installed.deleteRecursively()
                for ((backup, original) in backups) check(backup.renameTo(original))
            }
        }
    }
}
