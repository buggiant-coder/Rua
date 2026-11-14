package com.rua.mobile

import java.net.InetAddress
import java.net.ServerSocket
import java.net.SocketTimeoutException
import java.util.concurrent.CompletableFuture
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment

@RunWith(RobolectricTestRunner::class)
class PhoneUseStopTest {
    private data class Request(val line: String, val headers: Map<String, String>, val body: JSONObject)

    @Test
    fun nativeStopUsesCurrentTurnAndExactResidentOriginWithoutFollowingRedirects() {
        ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { resident ->
            ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { redirect ->
                resident.soTimeout = 3_000
                redirect.soTimeout = 500
                val received = CompletableFuture<Request>()
                thread(name = "PhoneUseStopFixture", isDaemon = true) {
                    try {
                        resident.accept().use { peer ->
                            peer.soTimeout = 3_000
                            val input = peer.getInputStream().bufferedReader()
                            val line = input.readLine()
                            val headers = linkedMapOf<String, String>()
                            while (true) {
                                val header = input.readLine() ?: error("Missing HTTP header terminator")
                                if (header.isEmpty()) break
                                headers[header.substringBefore(':').lowercase()] = header.substringAfter(':').trim()
                            }
                            val body = CharArray(headers.getValue("content-length").toInt())
                            var offset = 0
                            while (offset < body.size) {
                                val count = input.read(body, offset, body.size - offset)
                                check(count > 0)
                                offset += count
                            }
                            peer.getOutputStream().write(("HTTP/1.1 302 Found\r\n" +
                                "Location: http://127.0.0.1:${redirect.localPort}/unexpected\r\n" +
                                "Content-Length: 0\r\nConnection: close\r\n\r\n").toByteArray())
                            peer.getOutputStream().flush()
                            received.complete(Request(line, headers, JSONObject(String(body))))
                        }
                    } catch (error: Throwable) { received.completeExceptionally(error) }
                }
                try {
                    val bridge = PhoneUseBridge.startForResident(RuntimeEnvironment.getApplication())
                    bridge.state.bindResident(101)
                    assertTrue(bridge.state.publishPort(101, resident.localPort))
                    val first = PhoneInvocation("owner", PhoneChatAddress("project", "project-1", "chat-1"),
                        "turn-1", "authority-1", "configuration-1")
                    val lease = PhoneDeviceControl.sessions.activate(first)
                    val current = first.copy(turnId = "turn-2", authorityEpoch = "authority-2")
                    PhoneDeviceControl.bindTurn(current, bridge.state.generation)
                    PhoneDeviceControl.stopSession(lease.sessionId)
                    val request = received.get(4, TimeUnit.SECONDS)
                    assertEquals("POST /api/projects/project-1/chats/chat-1/console/stop HTTP/1.1", request.line)
                    assertTrue("Use only the captured resident credential",
                        request.headers["authorization"] == "Bearer ${bridge.authToken}")
                    assertEquals("turn-2", request.body.getString("turn_id"))
                    assertEquals("authority-2", request.body.getString("authority_epoch"))
                    assertEquals(2, request.body.length())
                    assertNull(PhoneDeviceControl.sessions.current())
                    assertTrue(runCatching { PhoneDeviceControl.sessions.activate(current) }.isFailure)
                    try {
                        redirect.accept().close()
                        fail("Native Stop must not forward its credential to an HTTP redirect")
                    } catch (_: SocketTimeoutException) { }
                } finally { PhoneUseBridge.resetForTests() }
            }
        }
    }
}
