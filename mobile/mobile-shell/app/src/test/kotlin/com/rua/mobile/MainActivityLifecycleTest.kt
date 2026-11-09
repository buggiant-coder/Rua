package com.rua.mobile

import android.util.Base64
import android.webkit.WebSettings
import android.webkit.WebView
import org.junit.Before
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.junit.rules.TemporaryFolder
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import java.io.File
import java.util.Locale

@RunWith(RobolectricTestRunner::class)
class MainActivityLifecycleTest {
    @get:Rule
    val temp = TemporaryFolder()

    @Before
    fun clearInstallCleanupState() {
        RuntimeEnvironment.application
            .getSharedPreferences(MobilePackageInstallCleanup.PREFS, android.content.Context.MODE_PRIVATE)
            .edit()
            .clear()
            .commit()
    }

    @Test
    fun nativeLaunchUsesResidentArgumentsWithoutHostShellOrCredentials() {
        val args = buildNativeRuaArgs("/app-resources/version/console", 8090)
        assertEquals(listOf("server", "start"), args.take(2))
        assertEquals("mobile-apk", args[args.indexOf("--profile") + 1])
        assertEquals("127.0.0.1", args[args.indexOf("--host") + 1])
        assertEquals("${MainActivity.HOME_DIR}/.rua", args[args.indexOf("--rua-root") + 1])
        assertEquals("/app-resources/version/console", args[args.indexOf("--console-static-dir") + 1])
        assertFalse(args.any { it.contains("PREFIX=") || it.contains("TOKEN") || it == "--no-auth" })
        assertFalse(args.any { it.contains("/usr/bin") || it.contains("rootfs") })
    }

    @Test
    fun nativeInitializationPropagatesFailure() {
        var invoked: List<String> = emptyList()
        assertFalse(initializeNativeRuaRoot { invoked = it; false })
        assertEquals(listOf("init", "--rua-root", "${MainActivity.HOME_DIR}/.rua"), invoked)
        assertTrue(initializeNativeRuaRoot { true })
    }

    @Test
    fun apkResourceExtractionRejectsPathsOutsideItsVersionDirectory() {
        val root = temp.newFolder("resources")
        assertEquals(File(root, "console/index.html"), NativePayload.safeResourceFile(root, "console/index.html"))
        for (path in listOf("../secret", "/absolute", "console/../../secret", "console/./file", "console//file")) {
            val failure = runCatching { NativePayload.safeResourceFile(root, path) }.exceptionOrNull()
            assertTrue("must reject $path", failure is IllegalArgumentException)
        }
    }

    @Test
    fun consoleTokenWaitsForResidentAcceptanceWithinOneBudget() {
        var time = 0L
        var probes = 0
        val accepted = awaitAcceptedConsoleToken(500, { "private-token" }, { _, remaining ->
            assertTrue(remaining in 1..500)
            probes += 1
            probes == 3
        }, { time }, { time += it })
        assertEquals("private-token", accepted)
        assertEquals(3, probes)
        assertEquals(200L, time)
    }

    @Test
    fun rejectedOrMissingConsoleTokenDoesNotOpenConsole() {
        var time = 0L
        assertNull(awaitAcceptedConsoleToken(250, { "stale-token" }, { _, _ -> false },
            { time }, { time += it }))
        assertEquals(250L, time)
        assertNull(awaitAcceptedConsoleToken(250, { null }, { _, _ -> error("must not probe") }))
    }

    @Test
    fun consoleWebViewSettingsUseHttpCacheAndDisableUserZoom() {
        val webView = WebView(RuntimeEnvironment.application)

        configureConsoleWebViewSettings(webView.settings)

        assertEquals(WebSettings.LOAD_DEFAULT, webView.settings.cacheMode)
        assertFalse(webView.settings.supportZoom())
        assertFalse(webView.settings.builtInZoomControls)
        assertFalse(webView.settings.displayZoomControls)
    }

    @Test
    fun pythonIndexRegionUsesChinaMirrorOnlyForMainlandSignals() {
        assertEquals("cn", pythonIndexRegionFor(Locale.SIMPLIFIED_CHINESE, "UTC"))
        assertEquals("cn", pythonIndexRegionFor(Locale.US, "Asia/Shanghai"))
        assertEquals("official", pythonIndexRegionFor(Locale.US, "UTC"))
    }

    @Test
    fun onlyTheExactLocalConsoleOriginIsTreatedAsOwned() {
        assertTrue(isOwnedConsoleOrigin("http://127.0.0.1:8090", 8090))
        assertTrue(isOwnedConsoleOrigin("http://127.0.0.1:8090/chat/1?x=2", 8090))

        assertFalse("localhost alias is a different origin", isOwnedConsoleOrigin("http://localhost:8090", 8090))
        assertFalse("a different port is not the resident", isOwnedConsoleOrigin("http://127.0.0.1:8091", 8090))
        assertFalse("https is not the bundled console", isOwnedConsoleOrigin("https://127.0.0.1:8090", 8090))
        assertFalse("external hosts never get console auth", isOwnedConsoleOrigin("http://example.com:8090", 8090))
        assertFalse(isOwnedConsoleOrigin(null, 8090))
        assertFalse(isOwnedConsoleOrigin("", 8090))
        assertFalse(isOwnedConsoleOrigin("not a url", 8090))
        assertFalse("an unpublished endpoint is never owned", isOwnedConsoleOrigin("http://127.0.0.1:0", 0))
    }

    @Test
    fun onlyHttpLinksMayBeHandedToAnExternalApp() {
        assertTrue(isHttpUrl("https://example.com/docs"))
        assertTrue(isHttpUrl("http://example.com"))

        assertFalse("javascript: must never be loaded or forwarded", isHttpUrl("javascript:alert(1)"))
        assertFalse("intent: handoffs must be blocked", isHttpUrl("intent://scan/#Intent;scheme=zxing;end"))
        assertFalse(isHttpUrl("file:///data/data/com.rua.mobile/files/home/.rua/x"))
        assertFalse(isHttpUrl("content://com.android.providers.downloads.documents/document/1"))
        assertFalse(isHttpUrl("tel:10086"))
        assertFalse(isHttpUrl("not a url"))
    }

    @Test
    fun runtimeProcessFinderOnlyTargetsOwnPackagedServerProcess() {
        val proc = temp.newFolder("proc")
        writeProcEntry(proc, pid = 101, uid = 42, cmdline = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000server\u0000start\u0000--profile\u0000mobile-apk")
        writeProcEntry(proc, pid = 102, uid = 42, cmdline = "/system/bin/sh\u0000")
        writeProcEntry(proc, pid = 103, uid = 99, cmdline = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000server\u0000start\u0000")
        writeProcEntry(proc, pid = 105, uid = 42, cmdline = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000chat\u0000hello")

        assertEquals(
            listOf(101),
            MobileRuntimeProcess.findRuaServerPids(procRoot = proc, appUid = 42)
        )
    }

    @Test
    fun runtimeRestartCleanupKillsPackagedServerAndClearsPidLock() {
        val proc = temp.newFolder("proc")
        val pidLock = temp.newFile("server.pid")
        val killed = mutableListOf<Int>()
        writeProcEntry(proc, pid = 101, uid = 42, cmdline = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000server\u0000start\u0000--profile\u0000mobile-apk")
        pidLock.writeText("101")

        val result = MobileRuntimeProcess.prepareForPackagedServerRestart(
            procRoot = proc,
            appUid = 42,
            pidLock = pidLock,
            killPid = { pid -> killed += pid },
            waitForNoServer = { true }
        )

        assertEquals(1, result.killed)
        assertTrue(result.completed)
        assertEquals(listOf(101), killed)
        assertFalse(pidLock.exists())
    }

    @Test
    fun runtimeRestartCleanupClearsStalePidLockWhenPidWasReusedByAnotherProcess() {
        val proc = temp.newFolder("proc")
        val pidLock = temp.newFile("server.pid")
        pidLock.writeText("15294")
        writeProcEntry(proc, pid = 15294, uid = 99, cmdline = "/system/bin/system_server\u0000")

        val result = MobileRuntimeProcess.prepareForPackagedServerRestart(
            procRoot = proc,
            appUid = 42,
            pidLock = pidLock,
            killPid = { error("no process should be killed") },
            waitForNoServer = { true }
        )

        assertEquals(0, result.killed)
        assertTrue(result.completed)
        assertFalse(pidLock.exists())
    }

    @Test
    fun runtimeRestartCleanupKeepsPidLockWhenKillFails() {
        val proc = temp.newFolder("proc")
        val pidLock = temp.newFile("server.pid")
        writeProcEntry(proc, pid = 101, uid = 42, cmdline = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000server\u0000start\u0000--profile\u0000mobile-apk")
        pidLock.writeText("101")

        val result = MobileRuntimeProcess.prepareForPackagedServerRestart(
            procRoot = proc,
            appUid = 42,
            pidLock = pidLock,
            killPid = { error("permission denied") },
            waitForNoServer = { false }
        )

        assertEquals(0, result.killed)
        assertFalse(result.completed)
        assertTrue(pidLock.exists())
    }

    @Test
    fun nativeLaunchCleansStalePidLockOnlyWhenServerPortIsClosed() {
        var cleanupCalls = 0
        val cleanup = {
            cleanupCalls += 1
            RestartCleanupResult(killed = 0, completed = true)
        }

        assertNull(cleanupBeforeNativeRuaLaunch(alreadyUp = true, cleanup = cleanup))
        assertEquals(0, cleanupCalls)

        val result = cleanupBeforeNativeRuaLaunch(alreadyUp = false, cleanup = cleanup)
        assertTrue(result?.completed ?: false)
        assertEquals(1, cleanupCalls)
    }

    @Test
    fun anAlreadyListeningResidentIsOnlyReusedAfterThePrivateHandshake() {
        // A resident from a previous bridge (older --no-auth runtime, earlier process) never
        // handshakes, so it must be replaced instead of silently serving a console without Phone Use.
        assertFalse(
            canReuseExistingResident(alreadyUp = true, handshakeToken = null)
        )
        assertFalse(
            canReuseExistingResident(alreadyUp = true, handshakeToken = "")
        )
        assertFalse(
            canReuseExistingResident(alreadyUp = false, handshakeToken = "token")
        )
        assertTrue(
            canReuseExistingResident(alreadyUp = true, handshakeToken = "token")
        )
    }

    @Test
    fun consoleIsOnlyLoadedWithASeededPrivateAuthToken() {
        // No handshake token means the resident never authenticated this bridge; the console must
        // not fall back to password login over an unverified connection.
        assertFalse(canLoadAuthenticatedConsole(documentStartSupported = true, handshakeToken = null))
        assertFalse(canLoadAuthenticatedConsole(documentStartSupported = true, handshakeToken = ""))

        // Without DOCUMENT_START_SCRIPT the token cannot be seeded before console code runs, so
        // loading anyway would leave the console unseeded even though a token exists.
        assertFalse(canLoadAuthenticatedConsole(documentStartSupported = false, handshakeToken = "token"))

        assertTrue(canLoadAuthenticatedConsole(documentStartSupported = true, handshakeToken = "token"))
    }

    @Test
    fun replacingAStaleResidentStillRunsThePackagedCleanup() {
        var cleanupCalls = 0
        val cleanup = {
            cleanupCalls += 1
            RestartCleanupResult(killed = 1, completed = true)
        }

        val reuse = canReuseExistingResident(alreadyUp = true, handshakeToken = null)
        val result = cleanupBeforeNativeRuaLaunch(reuse, cleanup)

        assertEquals(1, result?.killed)
        assertEquals(1, cleanupCalls)
    }

    @Test
    fun packageInstallCleanupRunsOncePerApkInstall() {
        val app = RuntimeEnvironment.application
        var cleanupCalls = 0

        val first = MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(
            context = app,
            packageUpdateTime = 100,
            cleanup = {
                cleanupCalls += 1
                RestartCleanupResult(killed = 1, completed = true)
            }
        )
        val second = MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(
            context = app,
            packageUpdateTime = 100,
            cleanup = {
                cleanupCalls += 1
                RestartCleanupResult(killed = 1, completed = true)
            }
        )

        assertEquals(1, first?.killed)
        assertNull(second)
        assertEquals(1, cleanupCalls)
    }

    @Test
    fun packageInstallCleanupRetriesWhenCleanupDoesNotComplete() {
        val app = RuntimeEnvironment.application
        var cleanupCalls = 0

        val first = MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(
            context = app,
            packageUpdateTime = 101,
            cleanup = {
                cleanupCalls += 1
                RestartCleanupResult(killed = 0, completed = false)
            }
        )
        val second = MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(
            context = app,
            packageUpdateTime = 101,
            cleanup = {
                cleanupCalls += 1
                RestartCleanupResult(killed = 0, completed = true)
            }
        )

        assertFalse(first?.completed ?: true)
        assertTrue(second?.completed ?: false)
        assertEquals(2, cleanupCalls)
    }

    @Test
    fun lifecycleReporterDeduplicatesAcknowledgedEvents() {
        val posted = mutableListOf<String>()
        val reporter = PlatformLifecycleReporter(
            endpoint = "http://127.0.0.1:8090/api/platform/lifecycle",
            postJson = { _, body -> posted += body },
            launchAsync = { task -> task.run() },
            logFailure = { _, _ -> }
        )

        reporter.reportForeground()
        reporter.reportForeground()
        reporter.reportBackground()
        reporter.reportBackground()

        assertEquals(
            listOf("""{"event":"foreground"}""", """{"event":"background"}"""),
            posted
        )
    }

    @Test
    fun mobileFileBridgeBuildsCreateDocumentPickerRequest() {
        var captured: MobileSaveBlobRequest? = null
        @Suppress("DEPRECATION")
        val bridge = MobileFileBridge(RuntimeEnvironment.application) { request ->
            captured = request
            "OK"
        }

        val result = bridge.saveBlobWithPicker(
            "req-1",
            "../bad\nname.md",
            Base64.encodeToString("hello".toByteArray(), Base64.NO_WRAP),
            "text/markdown"
        )

        assertEquals("OK", result)
        val request = captured ?: error("picker request was not captured")
        assertEquals("req-1", request.requestId)
        assertEquals(".._bad_name.md", request.name)
        assertEquals("text/markdown", request.mime)
        assertEquals("hello", String(request.bytes))
    }

    @Test
    fun activityTeardownKeepsTheProcessScopedBridgeAlive() {
        val context = RuntimeEnvironment.application
        val bridge = PhoneUseBridge.start(context)

        // Backgrounding or destroying the Activity must not end the private channel: it is owned
        // by the process, so a rotation would otherwise drop the resident's only auth handshake.
        val controller = org.robolectric.Robolectric.buildActivity(MainActivity::class.java).setup()
        controller.pause().stop().destroy()

        // Same singleton means onDestroy neither closed the bridge nor cleared the bound resident.
        // The real socket itself is not assertable here: Robolectric stubs LocalSocket, so binding
        // fails closed off-device. Device coverage of the bind path comes from the emulator run.
        assertSame(bridge, PhoneUseBridge.start(context))
        assertFalse(PhoneUseBridge.isResidentConnected())
    }

    private fun writeProcEntry(proc: File, pid: Int, uid: Int, cmdline: String) {
        val dir = File(proc, pid.toString())
        dir.mkdirs()
        File(dir, "status").writeText("Name:\trua\nUid:\t$uid\t$uid\t$uid\t$uid\n")
        File(dir, "cmdline").writeBytes(cmdline.toByteArray())
    }
}
