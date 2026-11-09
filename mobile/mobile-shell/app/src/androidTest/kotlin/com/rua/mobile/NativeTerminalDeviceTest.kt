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
class NativeTerminalDeviceTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val fixture = NativeAgentDeviceTest()
    private val workspace = File(MainActivity.HOME_DIR, ".rua/workspaces/default/native-terminal-acceptance")
    private val viewer = "terminal-owner-acceptance"
    private fun await(message: String, seconds: Int = 20, check: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + seconds * 1000L
        while (SystemClock.elapsedRealtime() < end) {
            if (check()) return
            SystemClock.sleep(50)
        }
        error(message)
    }
    private fun api(token: String, method: String, path: String, body: JSONObject? = null) =
        fixture.api(token, method, "/terminal$path" + (if (method == "GET" && !path.contains('?')) "?execution_device=local" else ""), body?.toString())
    private fun viewerBody() = JSONObject().put("browser_viewer_id", viewer).put("scrollback_lines", 2000)
    private fun open(token: String) = api(token, "POST", "/sessions", viewerBody()
        .put("execution_device", JSONObject().put("kind", "local")).put("cwd", workspace.path))
        .getJSONObject("session").getString("session_id")
    private fun children(token: String, id: String): List<String> {
        val marker = File(workspace, "owner-pids").also { it.delete() }
        val command = "python3 -c \"import os,subprocess,time; p=subprocess.Popen(['python3','-c','import time; time.sleep(1200)']); open('owner-pids','w').write(str(os.getpid())+' '+str(p.pid)); time.sleep(1200)\"\n"
        api(token, "POST", "/sessions/$id/input", viewerBody().put("data", command))
        await("Guest process tree did not start") { marker.isFile && marker.readText().trim().split(' ').size == 2 }
        val pids = marker.readText().trim().split(' ')
        val leader = NativeTerminalApiProbe.sessionLeader(pids.first())
        return pids + leader
    }
    private fun bridgePids(): List<String> = File("/proc").listFiles().orEmpty().filter {
        it.name.toIntOrNull() != null && runCatching {
            File(it, "cmdline").readText().contains("xterm-headless-bridge.mjs")
        }.getOrDefault(false)
    }.map { it.name }
    private fun gone(pids: List<String>) = pids.all { !File("/proc/$it").exists() }

    @Test fun terminalApiRunsDebianAndResidentExitClosesAllProcessOwners() {
        assertTrue(RuntimeToolchains.ready(context))
        workspace.mkdirs()
        ActivityScenario.launch(MainActivity::class.java).use {
            var token = ""
            await("Terminal API did not become ready", 30) {
                token = PhoneUseBridge.awaitAuthToken(100) ?: token
                runCatching { api(token, "GET", "/profiles"); true }.getOrDefault(false)
            }
            val profiles = api(token, "GET", "/profiles")
            assertEquals("proot", profiles.getString("default_profile_id"))
            assertEquals(1, profiles.getJSONArray("profiles").length())
            for (relative in listOf("usr/bin/env", "usr/local/bin/node")) {
                val source = File(context.filesDir, "runtime/debian/$relative")
                val backup = File(source.parentFile, source.name + ".terminal-test")
                check(source.renameTo(backup))
                try {
                    assertNotNull("Missing guest/Node must reject terminal creation", runCatching { open(token) }.exceptionOrNull())
                } finally { check(backup.renameTo(source)) }
            }
            val messages = mutableListOf<String>()
            NativeTerminalApiProbe(token, workspace) { messages.add(it); android.util.Log.i("NativeTerminalTest", it) }.run()
            assertTrue(messages.contains("TERMINAL_ALL_PASS"))
            val id = open(token)
            val pids = children(token, id)
            RuntimeGateDeviceTest.assertUpdateBusy(context)
            await("Node screen bridge missing") { bridgePids().isNotEmpty() }
            val bridges = bridgePids()
            context.stopService(Intent(context, NativeRuntimeService::class.java))
            await("Resident exit left terminal/bridge descendants", 30) { gone(pids + bridges) && MobileRuntimeProcess.findRuaServerPids().isEmpty() }
        }
    }

    @Test fun closingLastTerminalReleasesUpdateLeaseAndNextSessionStillWorks() {
        workspace.mkdirs()
        ActivityScenario.launch(MainActivity::class.java).use {
            val token = PhoneUseBridge.awaitAuthToken(30_000) ?: error("Handshake missing")
            await("Resident ready", 30) { runCatching { api(token, "GET", "/profiles"); true }.getOrDefault(false) }
            repeat(2) {
                val id = open(token)
                val marker = File(workspace, "update-terminal-version").also { it.delete() }
                api(token, "POST", "/sessions/$id/input", viewerBody().put("data", "node --version > update-terminal-version\n"))
                await("Terminal command did not execute") { marker.isFile && marker.readText().trim() == "v24.18.0" }
                RuntimeGateDeviceTest.assertUpdateBusy(context)
                api(token, "DELETE", "/sessions/$id?browser_viewer_id=$viewer")
                await("Last terminal left an idle Node bridge holding the update lease") { bridgePids().isEmpty() }
                val native = File(context.applicationInfo.nativeLibraryDir)
                val runtime = File(context.filesDir, "runtime")
                val lease = ProcessBuilder(File(native, "librua-runtime-gate.so").path, "--exclusive", File(runtime, "use.lock").path, File(runtime, "update.pending").path).redirectErrorStream(true).start()
                try { assertEquals("LOCKED", lease.inputStream.bufferedReader().readLine()) }
                finally { lease.outputStream.close(); lease.waitFor() }
            }
        }
    }

    /** Production detached TTL is ten minutes; no test-only policy override. */
    @Test fun detachedTerminalExpiresWithoutInterruptingAnotherSession() {
        assertTrue(RuntimeToolchains.ready(context))
        workspace.mkdirs()
        ActivityScenario.launch(MainActivity::class.java).use {
            var token = ""
            await("Terminal API did not become ready", 30) {
                token = PhoneUseBridge.awaitAuthToken(100) ?: token
                runCatching { api(token, "GET", "/profiles"); true }.getOrDefault(false)
            }
            val expiring = open(token)
            val pids = children(token, expiring)
            val other = open(token)
            try {
                api(token, "POST", "/sessions/$expiring/detach", viewerBody())
                assertFalse(gone(pids))
                await("Production detached TTL left guest descendants", 630) { gone(pids) }
                val marker = File(workspace, "ttl-other-alive").also { it.delete() }
                api(token, "POST", "/sessions/$other/input", viewerBody().put("data", "printf alive > ttl-other-alive\n"))
                await("TTL interrupted the other terminal") { marker.isFile && marker.readText() == "alive" }
            } finally {
                runCatching { api(token, "DELETE", "/sessions/$expiring?browser_viewer_id=$viewer") }
                api(token, "DELETE", "/sessions/$other?browser_viewer_id=$viewer")
            }
        }
    }
}
