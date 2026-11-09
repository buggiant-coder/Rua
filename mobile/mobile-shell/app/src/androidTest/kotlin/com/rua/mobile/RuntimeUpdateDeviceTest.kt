package com.rua.mobile

import android.content.ContextWrapper
import android.system.Os
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class RuntimeUpdateDeviceTest {
    private val context get() = InstrumentationRegistry.getInstrumentation().targetContext
    private fun fixture(root: File) = object : ContextWrapper(context) {
        override fun getFilesDir() = root
        override fun getApplicationContext(): android.content.Context = this
    }
    private fun copyFixture(): File {
        val dir = File(context.cacheDir, "update-fixture-${UUID.randomUUID()}").apply { mkdirs() }
        File(dir, "runtime").mkdirs(); File(dir, "tmp").mkdirs()
        val root = File(context.filesDir, "runtime/debian")
        val appFiles = context.applicationContext.filesDir
        val hidden = setOf("dev", "proc", "sys", "tmp", "sdcard", "storage", "root", "run/rua-cache",
            MainActivity.HOME_DIR.removePrefix("/"), File(appFiles, "home").path.removePrefix("/"), File(appFiles, "app-resources").path.removePrefix("/"))
        RuntimeTree.copy(root, File(dir, "runtime/debian"), { false }, root, hidden)
        val home = File(context.filesDir, "guest-home")
        RuntimeTree.copy(home, File(dir, "guest-home"), { false }, home, emptySet())
        Os.symlink(LinuxRuntimeInstaller.cache(context).path, LinuxRuntimeInstaller.cache(fixture(dir)).path)
        LinuxRuntimeInstaller.atomicText(File(dir, "home/workspace-sentinel"), "unchanged workspace")
        installFixtureDependencies(dir)
        return dir
    }
    private fun installFixtureDependencies(dir: File) {
        val materials = File(dir, "guest-home/.rua-update-fixture").apply { mkdirs() }
        val wheel = "rua_fixture_dependency-1.0-py3-none-any.whl"
        val temporary = File(materials, "$wheel.part")
        java.io.FileOutputStream(temporary).use { output ->
            InstrumentationRegistry.getInstrumentation().context.assets.open(wheel).use { it.copyTo(output) }
            output.fd.sync()
        }
        check(temporary.renameTo(File(materials, wheel)))
        NativePayload.syncDirectory(materials)
        val node = File(materials, "node").apply { mkdirs() }
        LinuxRuntimeInstaller.atomicText(File(node, "package.json"),
            """{"name":"rua-update-fixture","version":"1.0.0","bin":{"rua-update-node":"bin.js"}}""")
        LinuxRuntimeInstaller.atomicText(File(node, "bin.js"), "#!/usr/bin/env node\nconsole.log('NODE_UPDATE_OK');\n")
        Os.chmod(File(node, "bin.js").path, 0x1ed)
        LinuxRuntimeInstaller.guest(fixture(dir), File(dir, "runtime/debian"), File(dir, "tmp"), { false },
            listOf("/bin/bash", "-c", """set -eu
test -x /root/.venv/rua-mobile/bin/python || uv venv --python /usr/bin/python3 /root/.venv/rua-mobile
uv pip install --offline --python /root/.venv/rua-mobile/bin/python /root/.rua-update-fixture/$wheel
npm install --global --prefix /root/.local --offline --ignore-scripts --no-audit --no-fund /root/.rua-update-fixture/node
"""))
    }
    private fun probe(dir: File, expected: String) {
        val target = fixture(dir)
        LinuxRuntimeInstaller.guest(target, File(dir, "runtime/debian"), File(dir, "tmp"), { false },
            listOf("/bin/bash", "-c", "set -eu; test \"\$(node --version)\" = $expected; /root/.venv/rua-mobile/bin/python -c 'from rua_fixture_dependency import VALUE; assert VALUE == \"PYTHON_DEPENDENCY_OK\"'; test \"\$(/root/.local/bin/rua-update-node)\" = NODE_UPDATE_OK; file --version; test -z \"\$(dpkg --audit)\""))
        assertEquals("unchanged workspace", File(dir, "home/workspace-sentinel").readText())
    }
    private fun installOlderNode(dir: File) {
        val archive = File(context.filesDir, "runtime-test/node24.17.tar.gz")
        val sha = "faa0d59ba7fe7045c950ed09b190578fb8eee73e4358686d38fcc99ca58c1480"
        assertTrue(LinuxRuntimeInstaller.validCached(archive, LinuxRuntimeInstaller.Artifact(archive.name, archive.length(), sha)))
        val tools = File(dir, "runtime/debian/usr/local/lib/rua/toolchains")
        val destination = File(tools, sha).apply { mkdir() }
        archive.inputStream().use { LinuxRuntimeInstaller.extractRootfs(it, destination) }
        val root = File(dir, "runtime/debian")
        val commands = mapOf("node" to "bin/node", "npm" to "lib/node_modules/npm/bin/npm-cli.js", "npx" to "lib/node_modules/npm/bin/npx-cli.js")
        for ((name, command) in commands) {
            val link = File(root, "usr/local/bin/$name")
            check(link.delete())
            Os.symlink(File(destination, "node-v24.17.0-linux-arm64/$command").relativeTo(link.parentFile!!).path, link.path)
        }
        LinuxRuntimeInstaller.guest(fixture(dir), root, File(dir, "tmp"), { false },
            listOf("/bin/bash", "-c", "set -eu; node --version > /tmp/node; npm --version > /tmp/npm; npx --version > /tmp/npx"))
        val versions = JSONObject()
        for (name in commands.keys) versions.put(name, File(dir, "tmp/$name").readText().trim())
        assertEquals("v24.17.0", versions.getString("node"))
        LinuxRuntimeInstaller.atomicText(File(destination, ".rua-versions.json"), versions.toString())
    }

    @Test fun coveringApkReusesCompatibleEnvironmentWithZeroNetworkBytes() {
        val beforeBytes = RuntimeMirrorDownloader.receivedBytes.get()
        val beforeRequests = RuntimeMirrorDownloader.requests.get()
        val marker = File(context.filesDir, "runtime/debian/.rua-runtime-ready")
        val modified = marker.lastModified()
        val recipe = marker.readText()
        androidx.test.core.app.ActivityScenario.launch(MainActivity::class.java).use {
            assertNotNull(PhoneUseBridge.awaitAuthToken(30_000))
            val end = android.os.SystemClock.elapsedRealtime() + 30_000
            while (RuntimePreparation.running && android.os.SystemClock.elapsedRealtime() < end) android.os.SystemClock.sleep(50)
            assertFalse(RuntimePreparation.running)
            assertTrue(RuntimeToolchains.compatible(context))
            assertEquals(recipe, marker.readText())
            assertEquals(modified, marker.lastModified())
            assertEquals(0L, RuntimeMirrorDownloader.receivedBytes.get() - beforeBytes)
            assertEquals(0L, RuntimeMirrorDownloader.requests.get() - beforeRequests)
            LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/apk-reuse.json"),
                JSONObject().put("download_bytes", 0).put("download_requests", 0).put("native_dir", context.applicationInfo.nativeLibraryDir).toString())
        }
    }

    @Test fun pauseApkResourcesForExternalKill() {
        val fault = InstrumentationRegistry.getArguments().getString("fault_phase")!!
        require(fault in listOf("unpacking", "activated"))
        val dir = File(context.cacheDir, "update-fixture-${UUID.randomUUID()}").apply { mkdirs() }
        LinuxRuntimeInstaller.atomicText(File(dir, "runtime/debian/user-package"), "guest dependency")
        NativePayload.prepare(fixture(dir)) { phase ->
            if (phase == fault) {
                LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/update-cut.json"),
                    JSONObject().put("directory", dir.name).put("phase", phase).toString())
                while (true) android.os.SystemClock.sleep(1000)
            }
        }
        fail("APK extraction boundary was not reached")
    }

    @Test fun recoverApkResourcesAfterExternalKill() {
        val record = JSONObject(File(context.filesDir, "runtime-test/update-cut.json").readText())
        val name = record.getString("directory")
        require(name.startsWith("update-fixture-") && UUID.fromString(name.removePrefix("update-fixture-")).toString() == name.removePrefix("update-fixture-"))
        val dir = File(context.cacheDir, name)
        try {
            NativePayload.prepare(fixture(dir))
            assertEquals(context.applicationInfo.nativeLibraryDir, NativePayload.executable(fixture(dir)).parent)
            val expected = context.assets.open("rua-ui.sha256").bufferedReader().use { it.readText().trim() }
            val resources = File(dir, "app-resources/$expected")
            java.util.zip.ZipInputStream(context.assets.open("rua-ui.zip")).use { zip ->
                while (true) {
                    val entry = zip.nextEntry ?: break
                    if (!entry.isDirectory) assertArrayEquals(entry.name, zip.readBytes(), File(resources, entry.name).readBytes())
                }
            }
            assertTrue(File(resources, "xterm-headless-bridge.mjs").isFile)
            assertEquals("guest dependency", File(dir, "runtime/debian/user-package").readText())
        } finally { LinuxRuntimeInstaller.removeStaging(dir); NativePayload.prepare(context) }
    }

    @Test fun pauseUpdateForExternalKill() {
        val fault = InstrumentationRegistry.getArguments().getString("fault_phase")!!
        require(fault in listOf("copied", "root_saved", "home_saved", "home_committed", "activated"))
        val dir = copyFixture()
        RuntimeUpdate.install(fixture(dir), onPhase = { phase ->
            if (phase == fault) {
                LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/update-cut.json"),
                    JSONObject().put("directory", dir.name).put("phase", phase).toString())
                while (true) android.os.SystemClock.sleep(1000)
            }
        }, download = {})
        fail("Requested boundary was not reached")
    }

    @Test fun recoverUpdateAfterExternalKill() {
        val record = JSONObject(File(context.filesDir, "runtime-test/update-cut.json").readText())
        val name = record.getString("directory")
        require(name.startsWith("update-fixture-") && UUID.fromString(name.removePrefix("update-fixture-")).toString() == name.removePrefix("update-fixture-"))
        val dir = File(context.cacheDir, name)
        try {
            val target = fixture(dir)
            RuntimeUpdate.recover(target)
            assertTrue(RuntimeToolchains.ready(target))
            assertFalse(File(dir, "runtime/update.pending").exists())
            probe(dir, "v24.18.0")
            val expected = if (record.getString("phase") == "activated") "ready" else "rolled_back"
            assertEquals(expected, JSONObject(File(dir, "runtime/update.json").readText()).getString("phase"))
        } finally { LinuxRuntimeInstaller.removeStaging(dir) }
    }

    @Test fun olderCompatibleNodeUpdatesWithoutLosingDependenciesAndFailureRollsBack() {
        val dir = copyFixture()
        var inventory: Process? = null
        try {
            installOlderNode(dir)
            val target = fixture(dir)
            assertTrue(RuntimeToolchains.compatible(target))
            assertFalse(RuntimeToolchains.ready(target))
            RuntimePreparation.stop(context)
            val beforeRequests = RuntimeMirrorDownloader.requests.get()
            val beforeBytes = RuntimeMirrorDownloader.receivedBytes.get()
            try {
                RuntimePreparation.start(target)
                val end = android.os.SystemClock.elapsedRealtime() + 10_000
                val state = File(dir, "runtime/preparation.json")
                while ((!state.isFile || RuntimePreparation.running) && android.os.SystemClock.elapsedRealtime() < end) android.os.SystemClock.sleep(50)
                assertEquals("ready", JSONObject(state.readText()).getString("phase"))
                assertEquals(beforeRequests, RuntimeMirrorDownloader.requests.get())
                assertEquals(beforeBytes, RuntimeMirrorDownloader.receivedBytes.get())
                assertFalse("Ordinary preparation silently updated older Node", RuntimeToolchains.ready(target))
            } finally { RuntimePreparation.stop(target) }
            probe(dir, "v24.17.0")
            val nativeProbe = File(InstrumentationRegistry.getInstrumentation().context.applicationInfo.nativeLibraryDir, "librua-runtime-probe.so")
            val inventoryLog = File(dir, "inventory.log")
            val builder = ProcessBuilder(nativeProbe.path, "--inventory-update").redirectErrorStream(true).redirectOutput(inventoryLog)
            builder.environment().apply { clear(); putAll(NativePayload.process(target, emptyList()).environment()) }
            inventory = builder.start()
            val deadline = android.os.SystemClock.elapsedRealtime() + 10_000
            while (!inventoryLog.readText().contains("INVENTORY_v24.17.0_READY") && inventory.isAlive && android.os.SystemClock.elapsedRealtime() < deadline) android.os.SystemClock.sleep(50)
            assertTrue(inventoryLog.readText(), inventoryLog.readText().contains("INVENTORY_v24.17.0_READY"))
            try {
                RuntimeUpdate.install(target, onPhase = { if (it == "home_committed") error("injected activation failure") }, download = {})
                fail("Injected failure was ignored")
            } catch (error: IllegalStateException) { assertEquals("injected activation failure", error.message) }
            assertFalse(File(dir, "runtime/update.pending").exists())
            probe(dir, "v24.17.0")
            LinuxRuntimeInstaller.guest(target, File(dir, "runtime/debian"), File(dir, "tmp"), { false },
                listOf("/usr/bin/dpkg", "--remove", "file"))
            RuntimeUpdate.install(target, download = {})
            assertTrue(RuntimeToolchains.ready(target))
            assertFalse(File(dir, "runtime/update.pending").exists())
            probe(dir, "v24.18.0")
            inventory.outputStream.write("updated\n".toByteArray()); inventory.outputStream.flush()
            assertTrue(inventory.waitFor(10, java.util.concurrent.TimeUnit.SECONDS))
            assertEquals(inventoryLog.readText(), 0, inventory.exitValue())
            assertTrue(inventoryLog.readText(), inventoryLog.readText().contains("INVENTORY_v24.18.0_READY"))
            LinuxRuntimeInstaller.guest(target, File(dir, "runtime/debian"), File(dir, "tmp"), { false }, listOf("/usr/bin/file", "--version"))
            assertEquals("ready", JSONObject(File(dir, "runtime/update.json").readText()).getString("phase"))
        } finally {
            inventory?.destroy(); inventory?.waitFor(5, java.util.concurrent.TimeUnit.SECONDS)
            File(dir, "runtime/update.log").takeIf { it.isFile }?.let { LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/update-transaction.log"), it.readText()) }
            File(dir, "runtime/install.log").takeIf { it.isFile }?.let { LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/update-probe.log"), it.readText()) }
            File(dir, "runtime").listFiles().orEmpty().filter { it.name.startsWith("update-") }.forEach { staging ->
                File(staging, "runtime/install.log").takeIf { it.isFile }?.let { LinuxRuntimeInstaller.atomicText(File(context.filesDir, "runtime-test/update-candidate.log"), it.readText()) }
            }
            LinuxRuntimeInstaller.removeStaging(dir)
        }
    }
}
