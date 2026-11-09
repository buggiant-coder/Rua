package com.rua.mobile

import android.content.Context
import android.content.pm.PackageManager
import android.net.LocalServerSocket
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.util.Log
import androidx.core.content.ContextCompat
import java.io.BufferedInputStream
import java.io.BufferedOutputStream
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL
import java.security.SecureRandom
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import org.json.JSONObject

/**
 * Newline-delimited JSON wire contract for the private Phone Use bridge.
 *
 * The resident runtime connects over a filesystem Unix domain socket that lives in the APK's
 * private files dir. Requests are `{id, method, params}`; every response is either
 * `{id, result}` or `{id, error:{code, message}}`.
 */
internal object PhoneUseProtocol {
    const val VERSION = 1
    const val MAX_FRAME_BYTES = 64 * 1024
    const val READ_TIMEOUT_MS = 5_000
    const val AUTH_TOKEN_BYTES = 32
    const val SOCKET_FILE_NAME = "phone-use.sock"
    const val AUTH_TOKEN_STORAGE_KEY = "rua_auth_token"

    const val METHOD_HELLO = "hello"
    const val METHOD_STATUS = "status"
    const val METHOD_RESIDENT_READY = "resident_ready"

    const val ERROR_BAD_FRAME = "bad_frame"
    const val ERROR_BAD_REQUEST = "bad_request"
    const val ERROR_UNKNOWN_METHOD = "unknown_method"
    const val ERROR_UNAUTHORIZED = "unauthorized"
    const val ERROR_RESIDENT_MISMATCH = "resident_mismatch"
    const val ERROR_GENERATION_MISMATCH = "generation_mismatch"

    const val PARAM_GENERATION = "generation"

    const val STATE_READY = "ready"
    const val STATE_PERMISSION_REQUIRED = "permission_required"
    const val PLATFORM = "android"

    fun frameTooLarge(frameBytes: Int, maxFrameBytes: Int = MAX_FRAME_BYTES): Boolean =
        frameBytes > maxFrameBytes

    fun helloResult(generation: String, authToken: String): JSONObject = JSONObject()
        .put("protocol_version", VERSION)
        .put("generation", generation)
        .put("auth_token", authToken)

    fun statusResult(
        generation: String,
        status: PhoneUseNativeStatus,
        connected: Boolean,
    ): JSONObject = JSONObject()
        .put("generation", generation)
        .put("accessibility", status.accessibilityConnected)
        .put("camera_permission", status.cameraPermissionGranted)
        .put("state", if (status.executing) "executing" else if (connected) STATE_READY else STATE_PERMISSION_REQUIRED)
        .put("platform", PLATFORM)

    fun resultEnvelope(id: String, result: JSONObject): JSONObject = JSONObject()
        .put("id", id)
        .put("result", result)

    fun errorEnvelope(id: String, code: String, message: String): JSONObject = JSONObject()
        .put("id", id)
        .put("error", JSONObject().put("code", code).put("message", message))

    fun parseRequest(frame: ByteArray): PhoneUseRequest {
        val text = String(frame, Charsets.UTF_8).trim()
        if (text.isEmpty()) throw PhoneUseProtocolException(ERROR_BAD_REQUEST, "empty frame")
        val root = try {
            JSONObject(text)
        } catch (_: Exception) {
            throw PhoneUseProtocolException(ERROR_BAD_REQUEST, "malformed json")
        }
        val id = root.optString("id", "")
        if (id.isEmpty()) throw PhoneUseProtocolException(ERROR_BAD_REQUEST, "missing id")
        val method = root.optString("method", "")
        if (method.isEmpty()) throw PhoneUseProtocolException(ERROR_BAD_REQUEST, "missing method")
        val params = root.opt("params")
        if (params != null && params !is JSONObject) {
            throw PhoneUseProtocolException(ERROR_BAD_REQUEST, "params must be an object")
        }
        return PhoneUseRequest(id = id, method = method, params = params as? JSONObject)
    }
}

internal class PhoneUseProtocolException(val code: String, message: String) : Exception(message)

internal data class PhoneUseRequest(
    val id: String,
    val method: String,
    val params: JSONObject? = null,
)

/** Native permission state reported by [PhoneUseProtocol.METHOD_STATUS]; no actions exist yet. */
internal data class PhoneUseNativeStatus(
    val accessibilityConnected: Boolean,
    val cameraPermissionGranted: Boolean,
    val executing: Boolean = false,
)

internal enum class PhoneUsePeerDecision(val allowed: Boolean, val reason: String) {
    ACCEPT(true, "accept"),
    UID_MISMATCH(false, "uid_mismatch"),
    EXECUTABLE_UNRESOLVED(false, "executable_unresolved"),
    EXECUTABLE_MISMATCH(false, "executable_mismatch"),
    COMMAND_LINE_UNRESOLVED(false, "command_line_unresolved"),
    COMMAND_LINE_MISMATCH(false, "command_line_mismatch"),
}

/**
 * Peer identity policy: UID match is necessary but not sufficient, because tools spawned by the
 * resident share the APK UID. The peer must also be the packaged `rua` binary running the
 * resident `server start` command line.
 */
internal object PhoneUsePeerPolicy {
    fun evaluate(
        peerUid: Int,
        expectedUid: Int,
        peerExeIdentity: String?,
        expectedExeIdentity: String?,
        peerCommandLine: String?,
        requireResidentCommandLine: Boolean = true,
        isResidentCommandLine: (String) -> Boolean = MobileRuntimeProcess::isPackagedResidentServerCommandLine,
    ): PhoneUsePeerDecision {
        if (peerUid != expectedUid) return PhoneUsePeerDecision.UID_MISMATCH
        if (expectedExeIdentity.isNullOrEmpty()) return PhoneUsePeerDecision.EXECUTABLE_UNRESOLVED
        if (peerExeIdentity.isNullOrEmpty()) return PhoneUsePeerDecision.EXECUTABLE_UNRESOLVED
        if (peerExeIdentity != expectedExeIdentity) return PhoneUsePeerDecision.EXECUTABLE_MISMATCH
        if (!requireResidentCommandLine) return PhoneUsePeerDecision.ACCEPT
        if (peerCommandLine.isNullOrEmpty()) return PhoneUsePeerDecision.COMMAND_LINE_UNRESOLVED
        return if (isResidentCommandLine(peerCommandLine)) {
            PhoneUsePeerDecision.ACCEPT
        } else {
            PhoneUsePeerDecision.COMMAND_LINE_MISMATCH
        }
    }
}

/** Reads /proc based peer facts and applies [PhoneUsePeerPolicy]; fails closed when unreadable. */
internal object PhoneUsePeerAuthenticator {
    fun authenticate(
        peerUid: Int,
        peerPid: Int,
        appUid: Int = android.os.Process.myUid(),
        procRoot: File = File("/proc"),
        expectedExe: File,
        requireResidentCommandLine: Boolean = true,
        // Stat follows `/proc/<pid>/exe` to the real executable; injectable so JVM tests can
        // exercise the policy without the Android native filesystem.
        fileIdentity: (File) -> String? = ::androidFileIdentity,
    ): PhoneUsePeerDecision {
        if (peerUid != appUid) return PhoneUsePeerDecision.UID_MISMATCH
        val peerDir = File(procRoot, peerPid.toString())
        return PhoneUsePeerPolicy.evaluate(
            peerUid = peerUid,
            expectedUid = appUid,
            peerExeIdentity = fileIdentity(File(peerDir, "exe")),
            expectedExeIdentity = fileIdentity(expectedExe),
            peerCommandLine = readTextOrNull(File(peerDir, "cmdline")),
            requireResidentCommandLine = requireResidentCommandLine,
        )
    }

    private fun readTextOrNull(file: File): String? = try {
        if (file.isFile) file.readText() else null
    } catch (_: Throwable) {
        null
    }
}

/**
 * Identifies the executable by device+inode of the resolved file rather than by path string.
 * `/proc/<pid>/exe` is a symlink and the APK executable path changes on upgrades, so path
 * equality would both reject legitimate layouts and invite path-based spoofing. An unresolvable or
 * unreadable executable yields null, which fails closed.
 */
internal fun androidFileIdentity(file: File): String? = try {
    val stat = android.system.Os.stat(file.absolutePath)
    // A zero inode never identifies a real file; treating it as unresolved keeps this fail-closed
    // under environments that stub stat instead of reporting an error.
    if (stat.st_ino == 0L) null else "${stat.st_dev}:${stat.st_ino}"
} catch (_: Throwable) {
    null
}

internal class PhoneUseFrameTooLargeException(maxBytes: Int) :
    IOException("phone use frame exceeds $maxBytes bytes")

/** Bounded newline-delimited frame reader; refuses to buffer more than [maxFrameBytes]. */
internal class PhoneUseFrameReader(
    input: InputStream,
    private val maxFrameBytes: Int = PhoneUseProtocol.MAX_FRAME_BYTES,
) {
    private val input = BufferedInputStream(input, 1024)
    private val buffer = ByteArrayOutputStream(256)

    fun readFrame(): ByteArray? {
        while (true) {
            val next = input.read()
            if (next < 0) return null
            if (next == '\n'.code) {
                val frame = buffer.toByteArray()
                buffer.reset()
                return frame
            }
            // Compare against this reader's own limit: a caller-narrowed maxFrameBytes must not
            // silently accept frames up to the global constant.
            if (PhoneUseProtocol.frameTooLarge(buffer.size() + 1, maxFrameBytes)) {
                throw PhoneUseFrameTooLargeException(maxFrameBytes)
            }
            buffer.write(next)
        }
    }
}

internal enum class PhoneUseBindResult { BOUND, CONFLICT }

/**
 * Bridge-scoped state: one generation/auth token per bridge start, and exactly one resident pid
 * bound for the lifetime of this bridge generation.
 *
 * A different pid is always denied, even when the previously bound resident is gone: the bridge
 * never reconnects to a different resident. After the old resident exits, the APK owner creates a
 * new bridge, generation and token for the next resident.
 */
internal class PhoneUseBridgeState(
    val generation: String,
    val authToken: String,
    private val statusProvider: () -> PhoneUseNativeStatus,
) {
    private val bindLock = Any()
    private var residentPid: Int? = null
    private var httpPort: Int? = null

    fun bindResident(candidatePid: Int): PhoneUseBindResult = synchronized(bindLock) {
        val current = residentPid
        when (current) {
            null -> {
                residentPid = candidatePid
                PhoneUseBindResult.BOUND
            }
            candidatePid -> PhoneUseBindResult.BOUND
            else -> PhoneUseBindResult.CONFLICT
        }
    }

    fun isBoundResident(pid: Int): Boolean = synchronized(bindLock) { residentPid == pid }

    fun isConnected(): Boolean = synchronized(bindLock) { residentPid != null }

    fun matchesGeneration(candidate: String?): Boolean = candidate == generation

    fun publishPort(pid: Int, port: Int): Boolean = synchronized(bindLock) {
        if (residentPid != pid || port !in 1..65535 || (httpPort != null && httpPort != port)) false
        else { httpPort = port; true }
    }

    fun residentPort(): Int? = synchronized(bindLock) { httpPort }

    fun statusJson(): JSONObject =
        PhoneUseProtocol.statusResult(generation, statusProvider(), isConnected())
}

/** Serves one authenticated peer connection; transport agnostic so it is testable off-device. */
internal class PhoneUseSession(private val state: PhoneUseBridgeState) {
    fun serve(peerPid: Int, input: InputStream, output: OutputStream) {
        val reader = PhoneUseFrameReader(input)
        val writer = BufferedOutputStream(output)
        try {
            while (true) {
                val frame = try {
                    reader.readFrame()
                } catch (_: PhoneUseFrameTooLargeException) {
                    write(
                        writer,
                        PhoneUseProtocol.errorEnvelope(
                            "",
                            PhoneUseProtocol.ERROR_BAD_FRAME,
                            "frame exceeds ${PhoneUseProtocol.MAX_FRAME_BYTES} bytes",
                        ),
                    )
                    return
                } ?: return
                if (!handleFrame(peerPid, frame, writer)) return
            }
        } finally {
            runCatching { writer.flush() }
        }
    }

    private fun handleFrame(peerPid: Int, frame: ByteArray, writer: OutputStream): Boolean {
        val request = try {
            PhoneUseProtocol.parseRequest(frame)
        } catch (ex: PhoneUseProtocolException) {
            write(writer, PhoneUseProtocol.errorEnvelope("", ex.code, ex.message ?: ""))
            return true
        }
        when (request.method) {
            PhoneUseProtocol.METHOD_HELLO -> {
                if (state.bindResident(peerPid) == PhoneUseBindResult.CONFLICT) {
                    write(
                        writer,
                        PhoneUseProtocol.errorEnvelope(
                            request.id,
                            PhoneUseProtocol.ERROR_RESIDENT_MISMATCH,
                            "another resident runtime is already bound",
                        ),
                    )
                    return false
                }
                write(
                    writer,
                    PhoneUseProtocol.resultEnvelope(
                        request.id,
                        PhoneUseProtocol.helloResult(state.generation, state.authToken),
                    ),
                )
            }
            PhoneUseProtocol.METHOD_RESIDENT_READY -> {
                val params = request.params
                val port = params?.optInt("port", 0) ?: 0
                when {
                    !state.isBoundResident(peerPid) -> write(writer, PhoneUseProtocol.errorEnvelope(
                        request.id, PhoneUseProtocol.ERROR_UNAUTHORIZED, "hello required before resident_ready"))
                    !state.matchesGeneration(params?.optString(PhoneUseProtocol.PARAM_GENERATION)) ->
                        write(writer, PhoneUseProtocol.errorEnvelope(request.id,
                            PhoneUseProtocol.ERROR_GENERATION_MISMATCH, "resident_ready generation mismatch"))
                    !state.publishPort(peerPid, port) -> write(writer, PhoneUseProtocol.errorEnvelope(
                        request.id, PhoneUseProtocol.ERROR_BAD_REQUEST, "invalid or changed resident port"))
                    else -> write(writer, PhoneUseProtocol.resultEnvelope(request.id,
                        JSONObject().put("generation", state.generation).put("port", port)))
                }
            }
            PhoneUseProtocol.METHOD_STATUS -> {
                if (!state.isBoundResident(peerPid)) {
                    write(
                        writer,
                        PhoneUseProtocol.errorEnvelope(
                            request.id,
                            PhoneUseProtocol.ERROR_UNAUTHORIZED,
                            "hello required before status",
                        ),
                    )
                    return true
                }
                if (!state.matchesGeneration(
                        request.params?.optString(PhoneUseProtocol.PARAM_GENERATION)
                    )
                ) {
                    write(
                        writer,
                        PhoneUseProtocol.errorEnvelope(
                            request.id,
                            PhoneUseProtocol.ERROR_GENERATION_MISMATCH,
                            "status generation does not match the current bridge",
                        ),
                    )
                    return true
                }
                write(writer, PhoneUseProtocol.resultEnvelope(request.id, state.statusJson()))
            }
            "observe" -> {
                if (!state.isBoundResident(peerPid) || !state.matchesGeneration(request.params?.optString(PhoneUseProtocol.PARAM_GENERATION))) {
                    write(writer, PhoneUseProtocol.errorEnvelope(request.id, "generation_mismatch", "Observe requires the bound resident generation"))
                    return false
                }
                val params = request.params ?: JSONObject()
                var reservation:PhoneOperation?=null
                val operation = java.util.concurrent.FutureTask {
                    val active=PhoneDeviceControl.state.begin(params.getString("owner"),params.getString("operation_id"))
                    reservation=active
                    try {
                    val service = MobileAccessibilityService.instance ?: throw IllegalStateException("accessibility permission is required")
                    val result=service.observe(params.getString("owner"), params.optInt("max_nodes", 30), params.optInt("max_depth", 10))
                    check(!active.cancelled.get()) {"Phone observation cancelled"}
                    result
                    } finally {PhoneDeviceControl.state.finish(active)}
                }
                android.os.Handler(android.os.Looper.getMainLooper()).post(operation)
                try {
                    val result = operation.get(3, java.util.concurrent.TimeUnit.SECONDS)
                    write(writer, PhoneUseProtocol.resultEnvelope(request.id, result))
                } catch (error: Exception) {
                    PhoneDeviceControl.state.cancel(params.optString("owner"),params.optString("operation_id"))
                    reservation?.let {PhoneDeviceControl.state.finish(it)}
                    operation.cancel(false)
                    write(writer, PhoneUseProtocol.errorEnvelope(request.id, "observation_failed", (error.cause?.message ?: error.message ?: "observation failed").take(300)))
                }
            }
            "list_apps" -> {
                if (!state.isBoundResident(peerPid) || !state.matchesGeneration(request.params?.optString(PhoneUseProtocol.PARAM_GENERATION))) {
                    write(writer,PhoneUseProtocol.errorEnvelope(request.id,"generation_mismatch","Bound resident required")); return false
                }
                val service = MobileAccessibilityService.instance
                if(service == null) write(writer,PhoneUseProtocol.errorEnvelope(request.id,"unavailable","Phone Use is not connected"))
                else write(writer,PhoneUseProtocol.resultEnvelope(request.id,JSONObject().put("apps",org.json.JSONArray(PhoneApps.list(service))).put("limit",200)))
            }
            "screenshot" -> {
                if (!state.isBoundResident(peerPid) || !state.matchesGeneration(request.params?.optString(PhoneUseProtocol.PARAM_GENERATION))) {
                    write(writer,PhoneUseProtocol.errorEnvelope(request.id,"generation_mismatch","Bound resident required")); return false
                }
                try {
                    val params=request.params ?: error("Missing params")
                    val service=MobileAccessibilityService.instance ?: error("Accessibility service not connected")
                    val result=PhoneScreenshot.capture(service,params.getString("owner"),params.getString("operation_id"),params.optInt("max_nodes",30),params.optInt("max_depth",10)).get(4,java.util.concurrent.TimeUnit.SECONDS)
                    write(writer,PhoneUseProtocol.resultEnvelope(request.id,result))
                } catch(e:Exception) {write(writer,PhoneUseProtocol.errorEnvelope(request.id,"screenshot_failed",(e.cause?.message?:e.message?:"Screenshot failed").take(300)))}
            }
            "list_cameras", "take_photo" -> {
                if (!state.isBoundResident(peerPid) || !state.matchesGeneration(request.params?.optString(PhoneUseProtocol.PARAM_GENERATION))) {
                    write(writer,PhoneUseProtocol.errorEnvelope(request.id,"generation_mismatch","Bound resident required")); return false
                }
                try {
                    val context=PhoneUseBridge.applicationContext() ?: error("Phone Use host unavailable")
                    val params=request.params ?: error("Missing params")
                    val result=if(request.method=="list_cameras")PhoneCamera.list(context)
                    else PhoneCamera.take(context,params.getString("owner"),params.getString("operation_id"),params.getString("camera_id")).get(13,java.util.concurrent.TimeUnit.SECONDS)
                    write(writer,PhoneUseProtocol.resultEnvelope(request.id,result))
                }catch(e:Exception){write(writer,PhoneUseProtocol.errorEnvelope(request.id,"camera_failed",(e.cause?.message?:e.message?:"Camera failed").take(300)))}
            }
            "click", "long_press", "type_text", "swipe", "back", "home", "launch_app", "cancel" -> {
                if (!state.isBoundResident(peerPid) || !state.matchesGeneration(request.params?.optString(PhoneUseProtocol.PARAM_GENERATION))) {
                    write(writer, PhoneUseProtocol.errorEnvelope(request.id,"generation_mismatch","Bound resident required")); return false
                }
                try {
                    val params = request.params ?: error("Missing params")
                    val owner = params.getString("owner")
                    val operationId = params.getString("operation_id")
                    val result = if (request.method == "cancel") {
                        PhoneDeviceControl.state.cancel(owner, operationId)
                        JSONObject().put("cancelled",true)
                    } else (MobileAccessibilityService.instance ?: error("Accessibility service not connected")).controller.interact(owner, operationId, request.method, params).get(4, java.util.concurrent.TimeUnit.SECONDS)
                    write(writer, PhoneUseProtocol.resultEnvelope(request.id,result))
                } catch(e: Exception) { write(writer, PhoneUseProtocol.errorEnvelope(request.id,"interaction_failed",(e.cause?.message?:e.message?:"Interaction failed").take(300))) }
            }
            else -> write(
                writer,
                PhoneUseProtocol.errorEnvelope(
                    request.id,
                    PhoneUseProtocol.ERROR_UNKNOWN_METHOD,
                    "unsupported method",
                ),
            )
        }
        return true
    }

    private fun write(writer: OutputStream, envelope: JSONObject) {
        writer.write(envelope.toString().toByteArray(Charsets.UTF_8))
        writer.write('\n'.code)
        writer.flush()
    }
}

/**
 * Application-process owned private bridge for Phone Use.
 *
 * Scope of this slice: handshake plus native permission status only. No screenshots, no actions,
 * no external MCP/TCP endpoint, no watchdog or automatic recovery.
 */
internal class PhoneUseBridge private constructor(context: Context) {
    private val appContext: Context = context.applicationContext
    private val socketFile: File = socketPath(appContext)
    private val closed = AtomicBoolean(false)

    @Volatile
    private var server: LocalServerSocket? = null

    @Volatile
    private var boundSocket: LocalSocket? = null

    internal val state = PhoneUseBridgeState(
        generation = UUID.randomUUID().toString(),
        authToken = randomAuthToken(),
        statusProvider = ::readNativeStatus,
    )

    val authToken: String get() = state.authToken

    private fun startAccepting(): Boolean = try {
        socketFile.parentFile?.mkdirs()
        if (socketFile.exists() && !socketFile.delete()) {
            Log.w(TAG, "failed to remove stale phone use socket: ${socketFile.absolutePath}")
        }
        val bound = LocalSocket()
        bound.bind(LocalSocketAddress(socketFile.absolutePath, LocalSocketAddress.Namespace.FILESYSTEM))
        val listening = LocalServerSocket(bound.fileDescriptor)
        // The listener owns the file descriptor; keep our binding alive so nothing closes it early.
        boundSocket = bound
        server = listening
        thread(name = "PhoneUseBridgeAccept", isDaemon = true) { acceptLoop(listening) }
        Log.i(TAG, "phone use bridge listening on private filesystem socket")
        true
    } catch (ex: Throwable) {
        Log.e(TAG, "phone use bridge failed to bind private socket", ex)
        false
    }

    fun close() {
        closed.set(true)
        runCatching { server?.close() }
        server = null
        runCatching { boundSocket?.close() }
        boundSocket = null
    }

    private fun acceptLoop(listening: LocalServerSocket) {
        while (!closed.get()) {
            val client = try {
                listening.accept()
            } catch (ex: Throwable) {
                if (closed.get()) return
                // Fail closed: no accept retry loop, no watchdog. Recovery is a normal app restart.
                Log.e(TAG, "phone use bridge accept failed; channel unavailable", ex)
                return
            }
            thread(name = "PhoneUseBridgePeer", isDaemon = true) { servePeer(client) }
        }
    }

    private fun servePeer(client: LocalSocket) {
        try {
            // The read timeout is what bounds a stalled peer. Without it a single silent client
            // could pin a bridge thread forever, so failing to set it rejects the connection
            // rather than serving an unbounded read.
            try {
                client.soTimeout = PhoneUseProtocol.READ_TIMEOUT_MS
            } catch (ex: Throwable) {
                Log.w(TAG, "phone use read timeout unavailable; rejecting peer", ex)
                return
            }
            val credentials = try {
                client.peerCredentials
            } catch (ex: Throwable) {
                Log.w(TAG, "phone use bridge peer credentials unavailable; rejecting", ex)
                return
            }
            val decision = PhoneUsePeerAuthenticator.authenticate(
                credentials.uid, credentials.pid, expectedExe = NativePayload.executable(appContext),
            )
            val parentPid = runCatching {
                File("/proc/${credentials.pid}/status").readLines().first {it.startsWith("PPid:")}.substringAfter(":").trim().toInt()
            }.getOrNull()
            if (parentPid != android.os.Process.myPid()) {
                Log.w(TAG,"rejected Phone Use peer not launched by APK host")
                return
            }
            if (!decision.allowed) {
                Log.w(
                    TAG,
                    "rejected phone use bridge peer uid=${credentials.uid} pid=${credentials.pid} " +
                        "reason=${decision.reason}",
                )
                return
            }
            Log.i(TAG, "phone use bridge peer accepted pid=${credentials.pid}")
            PhoneUseSession(state).serve(credentials.pid, client.inputStream, client.outputStream)
        } catch (ex: Throwable) {
            Log.w(TAG, "phone use bridge connection ended: ${ex.javaClass.simpleName}")
        } finally {
            runCatching { client.close() }
        }
    }

    private fun readNativeStatus(): PhoneUseNativeStatus = PhoneUseNativeStatus(
        executing = PhoneDeviceControl.state.current() != null,
        accessibilityConnected = MobileAccessibilityService.instance != null,
        cameraPermissionGranted = ContextCompat.checkSelfPermission(
            appContext,
            android.Manifest.permission.CAMERA,
        ) == PackageManager.PERMISSION_GRANTED,
    )

    companion object {
        private const val TAG = "PhoneUseBridge"
        private const val TOKEN_WAIT_POLL_MS = 100L

        @Volatile
        private var instance: PhoneUseBridge? = null
        private val startLock = Any()

        /** Idempotent per process; started from normal APK startup before the resident launches. */
        fun start(context: Context): PhoneUseBridge = synchronized(startLock) {
            instance ?: PhoneUseBridge(context).also { bridge ->
                instance = bridge
                bridge.startAccepting()
            }
        }

        /** The APK owner calls this only after the previous resident has exited. */
        fun startForResident(context: Context): PhoneUseBridge = synchronized(startLock) {
            instance?.close()
            instance = null
            start(context)
        }

        fun socketPath(context: Context): File =
            File(context.filesDir, PhoneUseProtocol.SOCKET_FILE_NAME)

        fun ownedOrigin(port: Int = MainActivity.RUA_PORT): String = "http://127.0.0.1:$port"

        fun isOwnedLocalOrigin(endpoint: String, port: Int = MainActivity.RUA_PORT): Boolean = try {
            val url = URL(endpoint)
            port in 1..65535 && url.protocol.equals("http", ignoreCase = true) &&
                url.host.equals("127.0.0.1", ignoreCase = true) &&
                url.port == port
        } catch (_: Exception) {
            false
        }

        internal fun authorizationHeaderFor(
            endpoint: String,
            port: Int = MainActivity.RUA_PORT,
            token: String? = instanceAuthToken(),
        ): String? {
            if (token.isNullOrEmpty()) return null
            return if (isOwnedLocalOrigin(endpoint, port)) "Bearer $token" else null
        }

        /** Adds the resident auth header only for the exact local Rua origin. */
        fun authorize(connection: HttpURLConnection, endpoint: String) {
            connection.instanceFollowRedirects = false
            val header = authorizationHeaderFor(endpoint) ?: return
            if (!isOwnedLocalOrigin(connection.url.toString())) return
            connection.setRequestProperty("Authorization", header)
        }

        fun isResidentConnected(): Boolean = instanceAuthToken() != null
        internal fun applicationContext(): Context? = instance?.appContext

        /** Only the authenticated resident may publish its OS-assigned listener. Never probe foreign ports. */
        fun residentPort(): Int? = instance?.let { bridge ->
            if (!bridge.closed.get()) bridge.state.residentPort() else null
        }

        fun awaitResidentPort(timeoutMs: Long): Int? {
            val deadline = android.os.SystemClock.elapsedRealtime() + timeoutMs
            while (android.os.SystemClock.elapsedRealtime() < deadline) {
                residentPort()?.let { return it }
                try {
                    Thread.sleep(TOKEN_WAIT_POLL_MS)
                } catch (_: InterruptedException) {
                    Thread.currentThread().interrupt()
                    return null
                }
            }
            return residentPort()
        }

        /** Token to seed the bundled console with, or null when no resident has handshaked. */
        fun instanceAuthToken(): String? = instance?.let { bridge ->
            if (!bridge.closed.get() && bridge.state.isConnected()) bridge.authToken else null
        }

        /** Blocks until the bound resident completes the handshake, or returns null. */
        fun awaitAuthToken(timeoutMs: Long): String? {
            val deadline = System.currentTimeMillis() + timeoutMs
            while (System.currentTimeMillis() < deadline) {
                instanceAuthToken()?.let { return it }
                try {
                    Thread.sleep(TOKEN_WAIT_POLL_MS)
                } catch (_: InterruptedException) {
                    Thread.currentThread().interrupt()
                    return null
                }
            }
            return instanceAuthToken()
        }

        /**
         * Document-start script that only seeds the existing console auth token. No control JS is
         * injected; the caller must restrict it to the exact local Rua origin.
         */
        fun documentStartAuthScript(authToken: String): String =
            "(function(){try{window.localStorage.setItem(" +
                JSONObject.quote(PhoneUseProtocol.AUTH_TOKEN_STORAGE_KEY) + "," +
                JSONObject.quote(authToken) + ");}catch(e){}})();"

        internal fun randomAuthToken(random: SecureRandom = SecureRandom()): String {
            val bytes = ByteArray(PhoneUseProtocol.AUTH_TOKEN_BYTES)
            random.nextBytes(bytes)
            val hex = "0123456789abcdef"
            val builder = StringBuilder(bytes.size * 2)
            for (byte in bytes) {
                val value = byte.toInt() and 0xFF
                builder.append(hex[value ushr 4]).append(hex[value and 0x0F])
            }
            return builder.toString()
        }

        internal fun resetForTests() = synchronized(startLock) {
            instance?.close()
            instance = null
        }
    }
}
