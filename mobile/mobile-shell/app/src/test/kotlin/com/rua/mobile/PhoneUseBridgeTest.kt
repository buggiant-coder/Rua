package com.rua.mobile

import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.File
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

@RunWith(RobolectricTestRunner::class)
class PhoneUseBridgeTest {
    @get:Rule
    val temp = TemporaryFolder()

    private val residentCmdLine =
        "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000server\u0000start\u0000--profile\u0000mobile-apk"

    // --- peer identity policy -------------------------------------------------------------

    @Test
    fun nativeHttpRequestsDoNotFollowRedirects() {
        val connection = java.net.URL("http://127.0.0.1:${MainActivity.RUA_PORT}/api/platform/phone")
            .openConnection() as java.net.HttpURLConnection
        PhoneUseBridge.authorize(connection, connection.url.toString())
        assertFalse(connection.instanceFollowRedirects)
        connection.disconnect()
    }

    @Test
    fun peerPolicyAcceptsOnlyThePackagedResidentBinary() {
        assertEquals(
            PhoneUsePeerDecision.ACCEPT,
            PhoneUsePeerPolicy.evaluate(2000, 2000, "development:4142", "development:4142", residentCmdLine)
        )
    }

    @Test
    fun peerPolicyRejectsAnotherUidEvenWithTheRightExecutable() {
        assertEquals(
            PhoneUsePeerDecision.UID_MISMATCH,
            PhoneUsePeerPolicy.evaluate(3000, 2000, "dev:1", "dev:1", residentCmdLine)
        )
    }

    @Test
    fun peerPolicyRejectsSameUidToolChildThatIsNotTheResident() {
        assertEquals(
            PhoneUsePeerDecision.COMMAND_LINE_MISMATCH,
            PhoneUsePeerPolicy.evaluate(
                peerUid = 2000,
                expectedUid = 2000,
                peerExeIdentity = "dev:1",
                expectedExeIdentity = "dev:1",
                peerCommandLine = "/data/app/com.rua.mobile-test/lib/arm64/librua.so\u0000chat\u0000hello"
            )
        )
        assertEquals(
            PhoneUsePeerDecision.COMMAND_LINE_MISMATCH,
            PhoneUsePeerPolicy.evaluate(
                peerUid = 2000,
                expectedUid = 2000,
                peerExeIdentity = "dev:1",
                expectedExeIdentity = "dev:1",
                peerCommandLine = "/system/bin/sh\u0000-c\u0000id"
            )
        )
    }

    @Test
    fun peerPolicyRejectsAnotherExecutableFromTheSamePrefix() {
        assertEquals(
            PhoneUsePeerDecision.EXECUTABLE_MISMATCH,
            PhoneUsePeerPolicy.evaluate(
                peerUid = 2000,
                expectedUid = 2000,
                peerExeIdentity = "dev:999",
                expectedExeIdentity = "dev:1",
                peerCommandLine = residentCmdLine
            )
        )
    }

    @Test
    fun peerPolicyFailsClosedWhenPeerFactsCannotBeRead() {
        assertEquals(
            PhoneUsePeerDecision.EXECUTABLE_UNRESOLVED,
            PhoneUsePeerPolicy.evaluate(2000, 2000, null, "dev:1", residentCmdLine)
        )
        assertEquals(
            PhoneUsePeerDecision.COMMAND_LINE_UNRESOLVED,
            PhoneUsePeerPolicy.evaluate(2000, 2000, "dev:1", "dev:1", null)
        )
        assertEquals(
            PhoneUsePeerDecision.EXECUTABLE_UNRESOLVED,
            PhoneUsePeerPolicy.evaluate(2000, 2000, "dev:1", null, residentCmdLine)
        )
    }

    @Test
    fun peerPolicyComparesExecutableIdentityNotPathSpelling() {
        // Same inode reached through a different path (symlink/hardlink layout) is the same binary.
        assertEquals(
            PhoneUsePeerDecision.ACCEPT,
            PhoneUsePeerPolicy.evaluate(
                peerUid = 2000,
                expectedUid = 2000,
                peerExeIdentity = "dev:1",
                expectedExeIdentity = "dev:1",
                peerCommandLine = residentCmdLine,
            )
        )
    }

    @Test
    fun authenticatorResolvesProcExeAndCmdlineForTheResident() {
        val proc = temp.newFolder("proc")
        val prefix = temp.newFolder("prefix")
        val rua = File(prefix, "bin/rua")
        rua.parentFile?.mkdirs()
        rua.writeText("binary")
        val pidDir = File(proc, "4242").apply { mkdirs() }
        File(pidDir, "cmdline").writeBytes(residentCmdLine.toByteArray())
        createSymlink(rua, File(pidDir, "exe"))
        // Both /proc/<pid>/exe and the expected binary resolve to the same inode on a device.
        val identity: (File) -> String? = { "dev:1" }

        assertEquals(
            PhoneUsePeerDecision.ACCEPT,
            PhoneUsePeerAuthenticator.authenticate(
                peerUid = 2000,
                peerPid = 4242,
                appUid = 2000,
                procRoot = proc,
                expectedExe = rua,
                fileIdentity = identity,
            )
        )
        assertEquals(
            PhoneUsePeerDecision.UID_MISMATCH,
            PhoneUsePeerAuthenticator.authenticate(
                peerUid = 9999,
                peerPid = 4242,
                appUid = 2000,
                procRoot = proc,
                expectedExe = rua,
                fileIdentity = identity,
            )
        )
    }

    @Test
    fun authenticatorRejectsAResidentRunningADifferentExecutable() {
        val proc = temp.newFolder("proc-other-exe")
        val pidDir = File(proc, "4242").apply { mkdirs() }
        File(pidDir, "cmdline").writeBytes(residentCmdLine.toByteArray())
        File(pidDir, "exe").writeText("__placeholder__")

        assertEquals(
            PhoneUsePeerDecision.EXECUTABLE_MISMATCH,
            PhoneUsePeerAuthenticator.authenticate(
                peerUid = 2000,
                peerPid = 4242,
                appUid = 2000,
                procRoot = proc,
                expectedExe = File("/data/app/com.rua.mobile-test/lib/arm64/librua.so"),
                // The peer's executable resolves to a different inode than the packaged binary.
                fileIdentity = { file -> if (file.name == "exe") "dev:2" else "dev:1" },
            )
        )
    }

    @Test
    fun authenticatorRejectsResidentWhenProcEntryIsGone() {
        val proc = temp.newFolder("proc-absent")
        val prefix = temp.newFolder("prefix-absent")
        val rua = File(prefix, "bin/rua")

        assertEquals(
            PhoneUsePeerDecision.EXECUTABLE_UNRESOLVED,
            PhoneUsePeerAuthenticator.authenticate(
                peerUid = 2000,
                peerPid = 5150,
                appUid = 2000,
                procRoot = proc,
                expectedExe = rua,
                fileIdentity = { null },
            )
        )
    }

    @Test
    fun androidFileIdentityFailsClosedForUnstatablePaths() {
        assertNull(androidFileIdentity(File(temp.root, "definitely-not-here")))
    }

    // --- frame limit / protocol --------------------------------------------------------------

    @Test
    fun frameLimitIsSixtyFourKibibytes() {
        assertEquals(65_536, PhoneUseProtocol.MAX_FRAME_BYTES)
        assertFalse(PhoneUseProtocol.frameTooLarge(PhoneUseProtocol.MAX_FRAME_BYTES))
        assertTrue(PhoneUseProtocol.frameTooLarge(PhoneUseProtocol.MAX_FRAME_BYTES + 1))
    }

    @Test
    fun frameReaderSplitsNewlineDelimitedFrames() {
        val reader = PhoneUseFrameReader(
            ByteArrayInputStream("{\"id\":\"1\"}\n{\"id\":\"2\"}\n".toByteArray())
        )

        assertEquals("""{"id":"1"}""", String(reader.readFrame()!!))
        assertEquals("""{"id":"2"}""", String(reader.readFrame()!!))
        assertNull(reader.readFrame())
    }

    @Test
    fun frameReaderRefusesFramesOverTheLimit() {
        val oversized = "x".repeat(PhoneUseProtocol.MAX_FRAME_BYTES + 1) + "\n"

        var threw = false
        try {
            PhoneUseFrameReader(ByteArrayInputStream(oversized.toByteArray())).readFrame()
        } catch (_: PhoneUseFrameTooLargeException) {
            threw = true
        }

        assertTrue("oversized frame must be rejected", threw)
    }

    @Test
    fun parseRequestRequiresIdMethodAndObjectParams() {
        val ok = PhoneUseProtocol.parseRequest("""{"id":"a","method":"status","params":{}}""".toByteArray())
        assertEquals("a", ok.id)
        assertEquals(PhoneUseProtocol.METHOD_STATUS, ok.method)

        listOf(
            "",
            "not json",
            """{"method":"status","params":{}}""",
            """{"id":"a","params":{}}""",
            """{"id":"a","method":"status","params":[]}"""
        ).forEach { frame ->
            var threw = false
            try {
                PhoneUseProtocol.parseRequest(frame.toByteArray())
            } catch (_: PhoneUseProtocolException) {
                threw = true
            }
            assertTrue("frame should be rejected: $frame", threw)
        }
    }

    @Test
    fun parseRequestPreservesParamsSoStatusCanCheckTheGeneration() {
        val parsed = PhoneUseProtocol.parseRequest(
            """{"id":"s","method":"status","params":{"generation":"gen-1"}}""".toByteArray()
        )

        // Dropping params here would make every status request look generation-less and turn a
        // stale-runtime mismatch into a generic bad request.
        assertEquals("gen-1", parsed.params?.optString(PhoneUseProtocol.PARAM_GENERATION))

        val noParams = PhoneUseProtocol.parseRequest("""{"id":"h","method":"hello"}""".toByteArray())
        assertNull(noParams.params)
    }

    // --- session behaviour ---------------------------------------------------------------

    @Test
    fun residentPublishesItsActualListenerThroughThePrivateSession() {
        val state = newState(accessibility = false, camera = false)
        val session = PhoneUseSession(state)
        java.net.ServerSocket(0, 1, java.net.InetAddress.getByName("127.0.0.1")).use { listener ->
            val responses = runSession(session, peerPid = 101,
                input = frame("""{"id":"h","method":"hello","params":{}}""") +
                    frame("""{"id":"r","method":"resident_ready","params":{"generation":"${state.generation}","port":${listener.localPort}}}"""))
            val ready = responses.first { it.optString("id") == "r" }
            assertFalse("the APK must discover the bound port through its private channel", ready.has("error"))
            assertEquals(listener.localPort, ready.getJSONObject("result").getInt("port"))
        }
    }

    @Test
    fun foreignOrStaleResidentCannotPublishAConsoleEndpoint() {
        val state = newState(accessibility = false, camera = false)
        val session = PhoneUseSession(state)
        val ready = """{"id":"r","method":"resident_ready","params":{"generation":"${state.generation}","port":43123}}"""
        assertEquals(PhoneUseProtocol.ERROR_UNAUTHORIZED,
            errorOf(runSession(session, peerPid = 102, input = frame(ready)), "r"))
        runSession(session, peerPid = 101, input = frame("""{"id":"h","method":"hello","params":{}}"""))
        assertEquals(PhoneUseProtocol.ERROR_UNAUTHORIZED,
            errorOf(runSession(session, peerPid = 102, input = frame(ready)), "r"))
        assertEquals(PhoneUseProtocol.ERROR_GENERATION_MISMATCH,
            errorOf(runSession(session, peerPid = 101, input = frame(ready.replace(state.generation, "stale"))), "r"))
    }

    @Test
    fun handshakeAloneNeverPublishesAnEndpointAndEachGenerationStartsEmpty() {
        val state = newState(accessibility = false, camera = false)
        assertNull(state.residentPort())
        state.bindResident(101)
        assertNull("handshake is not HTTP readiness", state.residentPort())
        for (port in listOf(-1, 0, 65536)) assertFalse(state.publishPort(101, port))
        assertFalse(state.publishPort(102, 43123))
        assertTrue(state.publishPort(101, 43123))
        assertEquals(43123, state.residentPort())
        assertFalse("a generation cannot switch origins under an open WebView", state.publishPort(101, 43124))
        assertTrue(state.publishPort(101, 43123))
        assertNull(newState(accessibility = false, camera = false).residentPort())
        assertNull(PhoneUseBridge.authorizationHeaderFor("http://127.0.0.1:0/api/agents", 0, "token"))
    }

    @Test
    fun helloReturnsProtocolGenerationAndTokenThenStatusReportsNativeState() {
        val state = newState(accessibility = true, camera = false)
        val session = PhoneUseSession(state)

        val statusBeforeHello = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"s0","method":"status","params":{}}"""),
        )
        assertEquals(PhoneUseProtocol.ERROR_UNAUTHORIZED, errorOf(statusBeforeHello, "s0"))

        val afterHello = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}""") +
                statusFrame("s1", state.generation),
        )
        assertEquals(2, afterHello.size)

        val helloResult = afterHello.first { it.optString("id") == "h" }.getJSONObject("result")
        assertEquals(1, helloResult.getInt("protocol_version"))
        assertEquals(state.generation, helloResult.getString("generation"))
        assertEquals(64, helloResult.getString("auth_token").length)

        val statusResult = afterHello.first { it.optString("id") == "s1" }.getJSONObject("result")
        assertEquals(state.generation, statusResult.getString("generation"))
        assertTrue(statusResult.getBoolean("accessibility"))
        assertFalse(statusResult.getBoolean("camera_permission"))
        assertEquals("android", statusResult.getString("platform"))
    }

    @Test
    fun statusReportsPermissionRequiredUntilTheResidentHandshakes() {
        val state = newState(accessibility = true, camera = true)
        val session = PhoneUseSession(state)

        val responses = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}""") +
                statusFrame("s1", state.generation),
        )

        val statusResult = responses.first { it.optString("id") == "s1" }.getJSONObject("result")
        assertEquals(PhoneUseProtocol.STATE_READY, statusResult.getString("state"))
        assertTrue(statusResult.getBoolean("accessibility"))
        assertTrue(statusResult.getBoolean("camera_permission"))
    }

    @Test
    fun unknownMethodIsRejectedWithoutDroppingTheConnection() {
        val state = newState()
        val session = PhoneUseSession(state)

        val responses = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"x","method":"tap","params":{}}""") +
                statusFrame("s", state.generation),
        )

        assertEquals(PhoneUseProtocol.ERROR_UNKNOWN_METHOD, errorOf(responses, "x"))
        assertEquals(PhoneUseProtocol.ERROR_UNAUTHORIZED, errorOf(responses, "s"))
    }

    @Test
    fun statusFromAPeerThatNeverCompletedHelloIsDenied() {
        val state = newState()

        val responses = runSession(
            PhoneUseSession(state),
            peerPid = 101,
            input = statusFrame("s", state.generation),
        )

        assertEquals(PhoneUseProtocol.ERROR_UNAUTHORIZED, errorOf(responses, "s"))
        assertFalse(state.isConnected())
    }

    @Test
    fun secondLiveResidentIsDeniedWhileTheFirstStaysBound() {
        val state = PhoneUseBridgeState(
            generation = "gen",
            authToken = "token",
            statusProvider = { PhoneUseNativeStatus(false, false) },
        )

        assertEquals(PhoneUseBindResult.BOUND, state.bindResident(101))

        val secondSession = PhoneUseSession(state)
        val denied = runSession(
            secondSession,
            peerPid = 202,
            input = frame("""{"id":"h","method":"hello","params":{}}"""),
        )
        assertEquals(PhoneUseProtocol.ERROR_RESIDENT_MISMATCH, errorOf(denied, "h"))
        assertFalse(state.isBoundResident(202))
        assertTrue(state.isBoundResident(101))
    }

    @Test
    fun bridgeNeverRebindsToADifferentResidentPid() {
        val state = PhoneUseBridgeState(
            generation = "gen",
            authToken = "token",
            statusProvider = { PhoneUseNativeStatus(false, false) },
        )

        assertEquals(PhoneUseBindResult.BOUND, state.bindResident(101))
        // The old resident being gone must NOT open the channel to another pid; recovery is a
        // normal app restart, which builds a new bridge with a new generation.
        assertEquals(PhoneUseBindResult.CONFLICT, state.bindResident(202))
        assertTrue(state.isBoundResident(101))
        assertFalse(state.isBoundResident(202))
    }

    @Test
    fun statusRequiresTheExactCurrentGeneration() {
        val state = newState()
        val session = PhoneUseSession(state)

        val stale = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}""") +
                statusFrame("s-stale", "generation-from-a-previous-bridge-start"),
        )
        assertEquals(PhoneUseProtocol.ERROR_GENERATION_MISMATCH, errorOf(stale, "s-stale"))

        val missing = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}""") +
                statusFrame("s-missing", null),
        )
        assertEquals(PhoneUseProtocol.ERROR_GENERATION_MISMATCH, errorOf(missing, "s-missing"))

        val current = runSession(
            session,
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}""") +
                statusFrame("s-current", state.generation),
        )
        assertTrue(current.any { it.optString("id") == "s-current" && it.has("result") })
    }

    @Test
    fun laterStatusConnectionsNeedNoRepeatedHelloFromTheBoundResident() {
        val state = newState()
        // Production sequence: the resident says hello once during startup, then every later call
        // opens a fresh private socket carrying only a status request.
        val hello = runSession(
            PhoneUseSession(state),
            peerPid = 101,
            input = frame("""{"id":"h","method":"hello","params":{}}"""),
        )
        assertTrue(hello.any { it.optString("id") == "h" && it.has("result") })

        repeat(3) { index ->
            val responses = runSession(
                PhoneUseSession(state),
                peerPid = 101,
                input = statusFrame("s$index", state.generation),
            )
            assertTrue(
                "status-only connection $index must be served",
                responses.any { it.optString("id") == "s$index" && it.has("result") },
            )
        }
    }

    @Test
    fun eachBridgeStartUsesAFreshGenerationAndToken() {
        val first = newState()
        val second = newState()

        assertNotEquals(first.generation, second.generation)
        assertEquals(64, first.authToken.length)
        assertEquals(64, second.authToken.length)
        assertTrue(first.authToken.matches(Regex("[0-9a-f]{64}")))
        assertNotEquals(first.authToken, second.authToken)
    }

    // --- owned origin / native HTTP auth ----------------------------------------------------

    @Test
    fun authHeaderIsOnlyReturnedForTheExactLocalOrigin() {
        assertEquals(
            "Bearer secret",
            PhoneUseBridge.authorizationHeaderFor("http://127.0.0.1:8090/api/sync/status", 8090, "secret")
        )
        assertNull(PhoneUseBridge.authorizationHeaderFor("http://localhost:8090/api/sync/status", 8090, "secret"))
        assertNull(PhoneUseBridge.authorizationHeaderFor("http://127.0.0.1:9999/api/sync/status", 8090, "secret"))
        assertNull(PhoneUseBridge.authorizationHeaderFor("https://127.0.0.1:8090/api/sync/status", 8090, "secret"))
        assertNull(PhoneUseBridge.authorizationHeaderFor("http://127.0.0.1:8090/api/sync/status", 8090, null))
        assertNull(PhoneUseBridge.authorizationHeaderFor("http://127.0.0.1:8090/api/sync/status", 8090, ""))
    }

    @Test
    fun documentStartScriptOnlySeedsTheExistingConsoleToken() {
        val script = PhoneUseBridge.documentStartAuthScript("abc123")

        assertTrue(script.contains("localStorage.setItem"))
        assertTrue(script.contains("\"rua_auth_token\""))
        assertTrue(script.contains("\"abc123\""))
        assertFalse("no native control JS may be injected", script.contains("fetch("))
        assertFalse(script.contains("XMLHttpRequest"))
    }

    // --- helpers ---------------------------------------------------------------------------

    private fun newState(
        accessibility: Boolean = false,
        camera: Boolean = false,
    ): PhoneUseBridgeState = PhoneUseBridgeState(
        generation = java.util.UUID.randomUUID().toString(),
        authToken = PhoneUseBridge.randomAuthToken(),
        statusProvider = { PhoneUseNativeStatus(accessibility, camera) },
    )

    private fun frame(json: String): String = "$json\n"

    private fun statusFrame(id: String, generation: String?): String {
        val params = if (generation == null) {
            "{}"
        } else {
            """{"generation":${JSONObject.quote(generation)}}"""
        }
        return frame("""{"id":"$id","method":"status","params":$params}""")
    }

    private fun runSession(
        session: PhoneUseSession,
        peerPid: Int,
        input: String,
        output: ByteArrayOutputStream = ByteArrayOutputStream(),
    ): List<JSONObject> {
        session.serve(peerPid, ByteArrayInputStream(input.toByteArray()), output)
        return output.toString("UTF-8")
            .lineSequence()
            .filter { it.isNotBlank() }
            .map { JSONObject(it) }
            .toList()
    }

    private fun errorOf(responses: List<JSONObject>, id: String): String? =
        responses.firstOrNull { it.optString("id") == id }
            ?.optJSONObject("error")
            ?.optString("code")

    private fun createSymlink(target: File, link: File) {
        java.nio.file.Files.createSymbolicLink(link.toPath(), target.toPath())
    }
    @Test
    fun aNewOwnedResidentGetsANewBridgeGenerationAndClosedTokensAreUnavailable() {
        val context = org.robolectric.RuntimeEnvironment.getApplication()
        try {
            val first = PhoneUseBridge.startForResident(context)
            assertEquals(PhoneUseBindResult.BOUND, first.state.bindResident(101))
            assertEquals(first.authToken, PhoneUseBridge.instanceAuthToken())
            first.close()
            assertNull(PhoneUseBridge.instanceAuthToken())
            val second = PhoneUseBridge.startForResident(context)
            assertNotEquals(first.state.generation, second.state.generation)
            assertNotEquals(first.authToken, second.authToken)
            assertEquals(PhoneUseBindResult.CONFLICT, first.state.bindResident(202))
            assertEquals(PhoneUseBindResult.BOUND, second.state.bindResident(202))
            assertEquals(second.authToken, PhoneUseBridge.awaitAuthToken(100))
        } finally { PhoneUseBridge.resetForTests() }
    }

}
