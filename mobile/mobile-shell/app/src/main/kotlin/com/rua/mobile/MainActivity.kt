package com.rua.mobile

import android.annotation.SuppressLint
import android.app.Activity
import android.content.Context
import android.content.Intent
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.text.TextUtils
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.ContextCompat
import androidx.webkit.WebSettingsCompat
import androidx.webkit.WebViewCompat
import androidx.webkit.WebViewFeature
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.util.Locale
import java.util.TimeZone
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread
import org.json.JSONObject

internal fun packagedNativeConsoleStaticDir(): String =
    NativePayload.consoleDirectory

internal fun configureConsoleWebViewSettings(settings: WebSettings) {
    settings.apply {
        javaScriptEnabled = true
        domStorageEnabled = true
        allowFileAccess = true
        allowContentAccess = true
        mediaPlaybackRequiresUserGesture = false
        mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        useWideViewPort = true
        loadWithOverviewMode = true
        setSupportZoom(false)
        builtInZoomControls = false
        displayZoomControls = false
        cacheMode = WebSettings.LOAD_DEFAULT
        userAgentString = "$userAgentString Mobile/0.1.0"
    }
}

internal fun pythonIndexRegionFor(
    locale: Locale = Locale.getDefault(),
    timeZoneId: String = TimeZone.getDefault().id
): String {
    val country = locale.country.lowercase(Locale.ROOT)
    val languageTag = locale.toLanguageTag().lowercase(Locale.ROOT)
    val zone = timeZoneId.lowercase(Locale.ROOT)
    return if (
        country == "cn" ||
        languageTag == "zh-cn" ||
        zone == "asia/shanghai" ||
        zone == "asia/chongqing" ||
        zone == "asia/urumqi"
    ) {
        "cn"
    } else {
        "official"
    }
}

internal fun buildNativeRuaArgs(consoleDir: String, port: Int): List<String> = listOf(
    "server", "start", "--profile", "mobile-apk",
    "--host", "127.0.0.1", "--port", "$port",
    "--rua-root", "${MainActivity.HOME_DIR}/.rua",
    "--console-static-dir", consoleDir,
)

internal fun buildNativeRuaInitArgs(): List<String> =
    listOf("init", "--rua-root", "${MainActivity.HOME_DIR}/.rua")

internal fun initializeNativeRuaRoot(run: (List<String>) -> Boolean): Boolean =
    run(buildNativeRuaInitArgs())

internal fun awaitAcceptedConsoleToken(
    timeoutMs: Long,
    token: (Long) -> String?,
    accepts: (String, Int) -> Boolean,
    now: () -> Long = { android.os.SystemClock.elapsedRealtime() },
    pause: (Long) -> Unit = { Thread.sleep(it) },
): String? {
    val deadline = now() + timeoutMs
    while (now() < deadline) {
        val candidate = token((deadline - now()).coerceAtLeast(1)) ?: return null
        val remaining = (deadline - now()).coerceAtMost(1_000).toInt()
        if (remaining <= 0) return null
        if (accepts(candidate, remaining)) return candidate
        val delay = (deadline - now()).coerceAtMost(100)
        if (delay > 0) pause(delay)
    }
    return null
}

/**
 * Only the exact local Rua origin may receive the document-start auth token. Everything else is
 * handed to an external browser so the WebView never runs foreign pages with native JS bridges.
 */
internal fun isOwnedConsoleOrigin(url: String?, port: Int): Boolean {
    if (url.isNullOrEmpty() || port !in 1..65535) return false
    return try {
        val parsed = java.net.URI(url)
        parsed.scheme.equals("http", ignoreCase = true) &&
            parsed.host?.equals("127.0.0.1", ignoreCase = true) == true &&
            parsed.port == port
    } catch (_: Exception) {
        false
    }
}

internal fun isHttpUrl(url: String): Boolean = try {
    val scheme = java.net.URI(url).scheme
    scheme.equals("http", ignoreCase = true) || scheme.equals("https", ignoreCase = true)
} catch (_: Exception) {
    false
}

internal data class RestartCleanupResult(
    val killed: Int,
    val completed: Boolean
)

internal fun cleanupBeforeNativeRuaLaunch(
    alreadyUp: Boolean,
    cleanup: () -> RestartCleanupResult
): RestartCleanupResult? = if (alreadyUp) null else cleanup()

/**
 * An already-listening port is only adopted when the resident holding it completed this bridge's
 * private handshake. Otherwise it belongs to a previous bridge (older --no-auth runtime, previous
 * process) and must be replaced rather than reused, so Phone Use is never silently absent.
 */
internal fun canReuseExistingResident(alreadyUp: Boolean, handshakeToken: String?): Boolean =
    alreadyUp && !handshakeToken.isNullOrEmpty()

/**
 * The bundled console may only load once the private bridge has seeded its auth token. Without
 * both a handshake token and the document-start feature that delivers it, the console would fall
 * back to password login against a resident this bridge never authenticated.
 */
internal fun canLoadAuthenticatedConsole(
    documentStartSupported: Boolean,
    handshakeToken: String?,
): Boolean = documentStartSupported && !handshakeToken.isNullOrEmpty()

internal object MobileRuntimeProcess {
    private const val DEFAULT_STOP_TIMEOUT_MS = 3_000L

    internal fun isPackagedResidentServerCommandLine(cmdline: String): Boolean {
        val args = cmdline.split('\u0000').filter { it.isNotEmpty() }
        val executable = args.getOrNull(0) ?: return false
        val nativePayload = executable.startsWith("/data/app/") &&
            executable.contains("com.rua.mobile-") && executable.endsWith("/lib/arm64/librua.so")
        return (nativePayload || executable == NativePayload.executable?.absolutePath) &&
            args.getOrNull(1) == "server" &&
            args.getOrNull(2) == "start"
    }

    fun findRuaServerPids(
        procRoot: File = File("/proc"),
        appUid: Int = android.os.Process.myUid()
    ): List<Int> {
        return procRoot.listFiles()
            .orEmpty()
            .mapNotNull { procDir ->
                val pid = procDir.name.toIntOrNull() ?: return@mapNotNull null
                val uid = readProcUid(procDir) ?: return@mapNotNull null
                if (uid != appUid) return@mapNotNull null
                val cmdline = File(procDir, "cmdline").readTextOrNull() ?: return@mapNotNull null
                if (isPackagedResidentServerCommandLine(cmdline)) pid else null
            }
            .sorted()
    }

    fun killRuaServerProcesses(
        procRoot: File = File("/proc"),
        appUid: Int = android.os.Process.myUid(),
        killPid: (Int) -> Unit = { pid -> android.os.Process.killProcess(pid) }
    ): Int {
        return killPids(findRuaServerPids(procRoot, appUid), killPid)
    }

    fun prepareForPackagedServerRestart(
        procRoot: File = File("/proc"),
        appUid: Int = android.os.Process.myUid(),
        pidLock: File = File("${MainActivity.HOME_DIR}/.rua/server.pid"),
        killPid: (Int) -> Unit = { pid -> android.os.Process.killProcess(pid) },
        waitForNoServer: () -> Boolean = {
            waitForNoRuaServerProcesses(procRoot, appUid, DEFAULT_STOP_TIMEOUT_MS)
        }
    ): RestartCleanupResult {
        val pids = findRuaServerPids(procRoot, appUid)
        val killed = killPids(pids, killPid)
        val stopped = pids.isEmpty() || (killed > 0 && waitForNoServer())
        if (stopped) {
            clearPidLock(pidLock)
        } else {
            Log.w(MainActivity.TAG, "packaged resident server still visible after restart cleanup; keeping pid lock")
        }
        return RestartCleanupResult(killed = killed, completed = stopped)
    }

    private fun killPids(pids: List<Int>, killPid: (Int) -> Unit): Int {
        var killed = 0
        for (pid in pids) {
            try {
                killPid(pid)
                killed += 1
                Log.i(MainActivity.TAG, "killed stale packaged resident server pid=$pid")
            } catch (ex: Throwable) {
                Log.w(MainActivity.TAG, "failed to kill stale packaged resident server pid=$pid", ex)
            }
        }
        return killed
    }

    private fun waitForNoRuaServerProcesses(
        procRoot: File,
        appUid: Int,
        timeoutMs: Long
    ): Boolean {
        val deadline = System.currentTimeMillis() + timeoutMs
        while (System.currentTimeMillis() < deadline) {
            if (findRuaServerPids(procRoot, appUid).isEmpty()) return true
            Thread.sleep(100)
        }
        return findRuaServerPids(procRoot, appUid).isEmpty()
    }

    private fun clearPidLock(pidLock: File) {
        if (!pidLock.exists()) return
        if (pidLock.delete()) {
            Log.i(MainActivity.TAG, "cleared packaged rua-server pid lock")
        } else {
            Log.w(MainActivity.TAG, "failed to clear packaged rua-server pid lock: ${pidLock.absolutePath}")
        }
    }

    private fun readProcUid(procDir: File): Int? {
        val status = File(procDir, "status").readTextOrNull() ?: return null
        val uidLine = status.lineSequence().firstOrNull { it.startsWith("Uid:") } ?: return null
        return uidLine.split(Regex("\\s+")).getOrNull(1)?.toIntOrNull()
    }

    private fun File.readTextOrNull(): String? = try {
        readText()
    } catch (_: Throwable) {
        null
    }
}

internal object MobilePackageInstallCleanup {
    internal const val PREFS = "mobile_install_cleanup"
    internal const val KEY_CLEANED_PACKAGE_UPDATE_TIME = "cleaned_package_update_time"

    @Synchronized
    fun cleanupCurrentPackageInstallIfNeeded(
        context: Context,
        packageUpdateTime: Long = currentPackageUpdateTime(context),
        cleanup: () -> RestartCleanupResult = {
            MobileRuntimeProcess.prepareForPackagedServerRestart()
        }
    ): RestartCleanupResult? {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        if (!shouldCleanupForPackageUpdate(
                cleanedPackageUpdateTime = prefs.getLong(KEY_CLEANED_PACKAGE_UPDATE_TIME, -1L),
                currentPackageUpdateTime = packageUpdateTime
            )
        ) {
            return null
        }

        val result = cleanup()
        if (result.completed) {
            prefs.edit().putLong(KEY_CLEANED_PACKAGE_UPDATE_TIME, packageUpdateTime).apply()
        }
        return result
    }

    internal fun shouldCleanupForPackageUpdate(
        cleanedPackageUpdateTime: Long,
        currentPackageUpdateTime: Long
    ): Boolean = cleanedPackageUpdateTime != currentPackageUpdateTime

    @Suppress("DEPRECATION")
    private fun currentPackageUpdateTime(context: Context): Long {
        val info = context.packageManager.getPackageInfo(context.packageName, 0)
        return info.lastUpdateTime
    }
}

internal class PlatformLifecycleReporter(
    private val endpoint: String? = null,
    private val postJson: (String, String) -> Unit = ::postResidentJson,
    private val launchAsync: (Runnable) -> Unit = { task ->
        thread(name = "MobileLifecyclePost") { task.run() }
    },
    private val sleepMs: (Long) -> Unit = Thread::sleep,
    private val logFailure: (String, Throwable) -> Unit = { msg, ex ->
        Log.w(MainActivity.TAG, msg, ex)
    }
) {
    private val desiredEvent = AtomicReference<String?>(null)
    private val residentGeneration = java.util.concurrent.atomic.AtomicLong(0)
    private val acknowledgedEvent = AtomicReference<Pair<Long, String>?>(null)
    private val foregroundAcknowledgement = AtomicReference<(() -> Unit)?>(null)
    private val workerRunning = AtomicBoolean(false)

    fun reportForeground(onAcknowledged: (() -> Unit)? = null) {
        if (onAcknowledged != null) {
            foregroundAcknowledgement.set(onAcknowledged)
        }
        report("foreground")
    }

    fun reportBackground() {
        foregroundAcknowledgement.set(null)
        report("background")
    }

    fun residentChanged() {
        residentGeneration.incrementAndGet()
        if (desiredEvent.get() != null) startWorker()
    }

    private fun report(event: String) {
        desiredEvent.set(event)
        if (acknowledgedEvent.get() == (residentGeneration.get() to event) && !workerRunning.get()) {
            notifyAcknowledged(event)
            return
        }
        startWorker()
    }

    private fun startWorker() {
        if (!workerRunning.compareAndSet(false, true)) return
        launchAsync(Runnable { runWorker() })
    }

    private fun runWorker() {
        var retryDelayMs = INITIAL_RETRY_DELAY_MS
        try {
            while (true) {
                val event = desiredEvent.get() ?: return
                if (acknowledgedEvent.get() == (residentGeneration.get() to event)) return
                val generation = residentGeneration.get()
                try {
                    val target = endpoint ?: PhoneUseBridge.residentPort()?.let {
                        "${PhoneUseBridge.ownedOrigin(it)}/api/platform/lifecycle"
                    } ?: error("Private resident endpoint is not ready")
                    postJson(target, """{"event":"$event"}""")
                } catch (ex: Throwable) {
                    logFailure("lifecycle POST failed for event=$event", ex)
                    sleepMs(retryDelayMs)
                    retryDelayMs = (retryDelayMs * 2).coerceAtMost(MAX_RETRY_DELAY_MS)
                    continue
                }
                acknowledgedEvent.set(generation to event)
                retryDelayMs = INITIAL_RETRY_DELAY_MS
                if (generation == residentGeneration.get()) notifyAcknowledged(event)
                try {
                    Log.d(MainActivity.TAG, "posted lifecycle event: $event")
                } catch (ex: Throwable) {
                    logFailure("lifecycle acknowledgement log failed", ex)
                }
            }
        } finally {
            workerRunning.set(false)
            if (desiredEvent.get()?.let { residentGeneration.get() to it } != acknowledgedEvent.get()) {
                startWorker()
            }
        }
    }

    private fun notifyAcknowledged(event: String) {
        if (event != "foreground" || desiredEvent.get() != event) return
        val callback = foregroundAcknowledgement.getAndSet(null) ?: return
        try {
            callback()
        } catch (ex: Throwable) {
            logFailure("foreground acknowledgement callback failed", ex)
        }
    }

    companion object {
        private const val INITIAL_RETRY_DELAY_MS = 250L
        private const val MAX_RETRY_DELAY_MS = 5_000L
    }
}

internal fun postResidentJson(endpoint: String, body: String) {
    val connection = (URL(endpoint).openConnection() as HttpURLConnection).apply {
        requestMethod = "POST"
        connectTimeout = 750
        readTimeout = 750
        doOutput = true
        setRequestProperty("Content-Type", "application/json")
        PhoneUseBridge.authorize(this, endpoint)
    }
    try {
        connection.outputStream.use { output ->
            output.write(body.toByteArray(Charsets.UTF_8))
        }
        val code = connection.responseCode
        if (code !in 200..299) {
            throw java.io.IOException("lifecycle POST returned HTTP $code")
        }
    } finally {
        connection.disconnect()
    }
}

internal val processLifecycleReporter by lazy { PlatformLifecycleReporter() }









class MainActivity : AppCompatActivity() {

    companion object {
        const val TAG = "MobileMain"

        private const val AUTH_TOKEN_WAIT_TIMEOUT_MS = 8_000L

        val RUA_PORT: Int get() = PhoneUseBridge.residentPort() ?: 0
        const val FILES_DIR = "/data/data/com.rua.mobile/files"
        const val HOME_DIR = "/data/data/com.rua.mobile/files/home"

        const val RUA_LOG_PATH = "$HOME_DIR/.rua/logs/startup.log"
        const val PHONE_USE_SOCKET_PATH = "$FILES_DIR/phone-use.sock"
    }

    private val ruaUrl: String get() = PhoneUseBridge.ownedOrigin()

    private lateinit var rootLayout: FrameLayout
    private var webView: WebView? = null
    private var errorView: View? = null
    private lateinit var errorMsg: TextView

    private lateinit var splashView: View
    private lateinit var splashProgress: ProgressBar
    private lateinit var splashStatus: TextView

    private val uiHandler = Handler(Looper.getMainLooper())
    private var notificationPermissionCallback: ((Boolean) -> Unit)? = null



    private val ruaBootStarted = AtomicBoolean(false)
    private val bootstrapPreparing = AtomicBoolean(false)

    private val lifecycleReporter = processLifecycleReporter


    private var permissionDialog: AlertDialog? = null



    private var fileChooserCallback: ValueCallback<Array<Uri>>? = null
    private var saveBlobRequest: MobileSaveBlobRequest? = null
    private var pendingNotificationRuaUri: String? = null




    private val runtimeImportLauncher = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) thread(name = "LinuxRuntimeImport") {
            try {
                contentResolver.openInputStream(uri)?.use { RuntimePreparation.importOffline(applicationContext, it) }
                    ?: error("无法打开离线材料")
            } catch (error: Exception) {
                Log.w(TAG, "Linux runtime import failed", error)
                runOnUiThread { android.widget.Toast.makeText(this, error.message, android.widget.Toast.LENGTH_LONG).show() }
            }
        }
    }
    private val runtimeMaterialsSummary by lazy {
        val manifest = LinuxRuntimeInstaller.manifest(this)
        "Debian · Python · Node · uv\n下载约 ${manifest.artifacts.sumOf { it.size } / 1_000_000} MB · 需 ${String.format(Locale.ROOT, "%.1f", manifest.requiredFreeBytes / 1_000_000_000.0)} GB 可用空间"
    }
    private var setupView: RuntimeSetupView? = null
    private var preparedConsoleToken: String? = null
    private val runtimeStatusRefresh = object : Runnable {
        override fun run() {
            if (isDestroyed) return
            val ready = RuntimeToolchains.compatible(this@MainActivity)
            if (setupView != null && ready) {
                rootLayout.removeView(setupView)
                setupView = null
                showWebView(preparedConsoleToken)
            } else {
                setupView?.render(RuntimePreparation.phaseName(this@MainActivity), LinuxRuntimeInstaller.status, RuntimePreparation.running)
            }
            uiHandler.removeCallbacks(this)
            if (setupView != null) uiHandler.postDelayed(this, 1000)
        }
    }

    private fun showRuntimeSetup(consoleToken: String?) {
        preparedConsoleToken = consoleToken
        if (setupView == null) {
            setupView = RuntimeSetupView(this, runtimeMaterialsSummary,
                onAction = {
                    if (RuntimePreparation.running) RuntimePreparation.cancel(applicationContext)
                    else RuntimePreparation.retry(applicationContext)
                },
                onImport = { runtimeImportLauncher.launch(arrayOf("application/zip", "application/octet-stream")) },
                onPermissions = { showPermissionsDialog(firstRun = true) },
            ).also { rootLayout.addView(it, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)) }
        }
        setupView?.render(RuntimePreparation.phaseName(this), LinuxRuntimeInstaller.status, RuntimePreparation.running)
        uiHandler.removeCallbacks(runtimeStatusRefresh)
        uiHandler.post(runtimeStatusRefresh)
    }

    private val notificationPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) { granted ->
        val callback = notificationPermissionCallback
        notificationPermissionCallback = null
        callback?.invoke(granted)
    }

    private val fileChooserLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val cb = fileChooserCallback
        fileChooserCallback = null
        if (cb == null) return@registerForActivityResult
        val uris: Array<Uri>? = if (result.resultCode == Activity.RESULT_OK) {
            WebChromeClient.FileChooserParams.parseResult(result.resultCode, result.data)
        } else null
        cb.onReceiveValue(uris)
    }

    private fun requestNotificationPermission(callback: (Boolean) -> Unit) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            PermissionGuide.hasNotificationPermission(this)
        ) {
            callback(true)
            return
        }
        if (notificationPermissionCallback != null) return
        notificationPermissionCallback = callback
        notificationPermissionLauncher.launch(android.Manifest.permission.POST_NOTIFICATIONS)
    }

    private val saveBlobLauncher = registerForActivityResult(
        ActivityResultContracts.StartActivityForResult()
    ) { result ->
        val request = saveBlobRequest
        saveBlobRequest = null
        if (request == null) return@registerForActivityResult
        if (result.resultCode != Activity.RESULT_OK) {
            notifyMobileSaveBlobResult(request.requestId, "cancelled", "")
            return@registerForActivityResult
        }
        val uri = result.data?.data
        if (uri == null) {
            notifyMobileSaveBlobResult(request.requestId, "error", "missing destination uri")
            return@registerForActivityResult
        }
        try {
            contentResolver.openOutputStream(uri).use { os ->
                if (os == null) error("openOutputStream returned null")
                os.write(request.bytes)
                os.flush()
            }
            notifyMobileSaveBlobResult(request.requestId, "ok", "")
        } catch (ex: Exception) {
            Log.e(TAG, "save blob picker write failed", ex)
            notifyMobileSaveBlobResult(
                request.requestId,
                "error",
                "${ex.javaClass.simpleName}: ${ex.message ?: ""}",
            )
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Log.i("BootProf", "[t-MainActivity.onCreate] entry")
        rootLayout = FrameLayout(this)
        setContentView(rootLayout)
        captureUserNotificationIntent(intent)





        showSplash("准备中...")
        startBootstrapAndRua()
    }

    override fun onDestroy() {
        uiHandler.removeCallbacks(runtimeStatusRefresh)

        permissionDialog?.dismiss()
        permissionDialog = null
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent?) {
        super.onNewIntent(intent)
        setIntent(intent)
        captureUserNotificationIntent(intent)
    }

    private fun beginSaveBlobWithPicker(request: MobileSaveBlobRequest): String {
        if (saveBlobRequest != null) return "ERROR:save_picker_busy"
        saveBlobRequest = request
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT).apply {
            addCategory(Intent.CATEGORY_OPENABLE)
            type = request.mime.ifEmpty { "application/octet-stream" }
            putExtra(Intent.EXTRA_TITLE, request.name)
            addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        }
        uiHandler.post {
            try {
                saveBlobLauncher.launch(intent)
            } catch (ex: Exception) {
                Log.e(TAG, "save blob picker launch failed", ex)
                if (saveBlobRequest?.requestId == request.requestId) {
                    saveBlobRequest = null
                }
                notifyMobileSaveBlobResult(
                    request.requestId,
                    "error",
                    "${ex.javaClass.simpleName}: ${ex.message ?: ""}",
                )
            }
        }
        return "OK"
    }

    private fun notifyMobileSaveBlobResult(
        requestId: String,
        status: String,
        message: String,
    ) {
        val js = "window.__mobileSaveBlobResult && window.__mobileSaveBlobResult(" +
            "${JSONObject.quote(requestId)},${JSONObject.quote(status)},${JSONObject.quote(message)})"
        uiHandler.post {
            webView?.evaluateJavascript(js, null)
        }
    }

    private fun captureUserNotificationIntent(intent: Intent?) {
        if (intent?.action != MobileNotificationBridge.ACTION_OPEN_USER_NOTIFICATION) return
        val uri = intent.getStringExtra(MobileNotificationBridge.EXTRA_RUA_URI)
            ?.let(::safeRuaUriOrNull)
        if (uri != null) {
            pendingNotificationRuaUri = uri
            dispatchPendingNotificationRuaUri()
        }
    }

    private fun dispatchPendingNotificationRuaUri() {
        val uri = pendingNotificationRuaUri ?: return
        val activeWebView = webView ?: return
        pendingNotificationRuaUri = null
        val js = "window.dispatchEvent(new CustomEvent(" +
            "'rua:user-notification-activated'," +
            "{detail:{uri:${JSONObject.quote(uri)}}}))"
        uiHandler.post {
            activeWebView.evaluateJavascript(js, null)
        }
    }

    override fun onResume() {
        super.onResume()
        lifecycleReporter.reportForeground {
            uiHandler.post {
                webView?.evaluateJavascript(
                    "window.dispatchEvent(new Event('rua:foreground'))",
                    null,
                )
            }
        }
        SyncBackgroundWorker.refreshScheduleAsync(this, ruaUrl)

        if (!bootstrapPreparing.get() && NativePayload.consoleDirectory.isNotEmpty()) {
            tryStartBootFlow()
        }

        if (permissionDialog?.isShowing == true) {
            permissionDialog?.dismiss()
            showPermissionsDialog(firstRun = false)
        }
    }

    override fun onStop() {
        webView?.evaluateJavascript(
            "window.dispatchEvent(new Event('rua:background'))",
            null,
        )
        lifecycleReporter.reportBackground()
        super.onStop()
    }


    private fun tryStartBootFlow() {
        if (ruaBootStarted.compareAndSet(false, true)) {
            uiHandler.post {
                splashStatus.text = "正在启动 Rua..."
                splashProgress.isIndeterminate = true
                splashProgress.progress = 0
            }
            thread(name = "MobileBoot") { bootFlow() }
        }
    }

    private fun startBootstrapAndRua() {
        bootstrapPreparing.set(true)
        thread(name = "RuaApkResources") {
            try {
                NativePayload.prepare(applicationContext)
                bootstrapPreparing.set(false)
                tryStartBootFlow()
            } catch (ex: Exception) {
                Log.e(TAG, "APK preparation failed", ex)
                bootstrapPreparing.set(false)
                uiHandler.post { showError("应用资源准备失败: ${ex.message}") }
            }
        }
    }







    private fun maybeShowPermissionsDialog(firstRun: Boolean) {
        if (PermissionGuide.standardItems().all { it.isGranted(this) }) return

        uiHandler.postDelayed({
            if (isFinishing || isDestroyed) return@postDelayed
            showPermissionsDialog(firstRun)
        }, 800)
    }


    private fun showRuntimeManagement() {
        val updating = RuntimePreparation.running
        AlertDialog.Builder(this).setTitle("工具环境管理")
            .setMessage(LinuxRuntimeInstaller.status + "\n\n更新前请结束正在使用环境的工具、插件和终端。已有依赖和工作会保留。")
            .setPositiveButton(if (updating) "取消更新" else "更新工具环境") { _, _ ->
                if (updating) RuntimePreparation.cancel(applicationContext)
                else RuntimePreparation.update(applicationContext)
            }.setNegativeButton("关闭", null).show()
    }

    private fun showPermissionsDialog(firstRun: Boolean) {
        permissionDialog?.dismiss()
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val textColor = Color.parseColor(if (night) "#E9ECEF" else "#0C0F12")
        val secondaryColor = Color.parseColor(if (night) "#B8BCC2" else "#6C757D")

        val container = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 32, 48, 32)
        }


        container.addView(TextView(this).apply {
            text = "手Rua 是 AI 助理, 能帮你:\n· 操作手机 (点击 / 输入 / 截屏)\n· 写代码、查资料、对话\n· 后台跑 agent 自动完成任务"
            textSize = 14f
            setTextColor(secondaryColor)
            setPadding(0, 0, 0, 24)
        })

        if (firstRun) {
            container.addView(TextView(this).apply {
                text = "运行环境会在后台继续准备，设置完成后返回应用即可。"
                textSize = 12f
                setTextColor(secondaryColor)
                setPadding(0, 0, 0, 16)
            })
        }

        container.addView(TextView(this).apply {
            text = "按需要开启以下权限："
            setTextColor(textColor)
            textSize = 13f
            setTypeface(typeface, Typeface.BOLD)
            setPadding(0, 8, 0, 12)
        })


        PermissionGuide.standardItems().forEach { item ->
            container.addView(buildPermissionRow(item))
        }
        container.addView(Button(this).apply {
            val granted=androidx.core.content.ContextCompat.checkSelfPermission(this@MainActivity,android.Manifest.permission.CAMERA)==android.content.pm.PackageManager.PERMISSION_GRANTED
            text=if(granted) "相机已授权" else "授权相机（可见预览单次拍照）"
            isEnabled=!granted
            setOnClickListener { androidx.core.app.ActivityCompat.requestPermissions(this@MainActivity,arrayOf(android.Manifest.permission.CAMERA),4207) }
        })


        container.addView(View(this).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT, 2
            ).apply { setMargins(0, 24, 0, 24) }
            setBackgroundColor(Color.LTGRAY)
        })


        container.addView(TextView(this).apply {
            text = "其他系统设置"
            setTextColor(textColor)
            textSize = 13f
            setTypeface(typeface, Typeface.BOLD)
            setPadding(0, 0, 0, 8)
        })
        container.addView(TextView(this).apply {
            text = PermissionGuide.manualSteps()
            textSize = 12f
            setTextColor(secondaryColor)
            setLineSpacing(0f, 1.3f)
        })

        val scroll = ScrollView(this).apply { addView(container) }

        permissionDialog = AlertDialog.Builder(this)
            .setTitle("权限设置")
            .setView(scroll)
            .setPositiveButton("完成", null)
            .setNegativeButton("稍后再说", null)
            .setCancelable(false)
            .show()
    }

    private fun buildPermissionRow(item: PermissionGuide.Item): View {
        val granted = item.isGranted(this)
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(0, 12, 0, 12)
        }

        row.addView(TextView(this).apply {
            text = if (granted) "✓" else "✗"
            textSize = 20f
            setTextColor(if (granted) Color.parseColor("#4CAF50") else Color.parseColor("#F44336"))
            setPadding(0, 0, 24, 0)
        })

        val nameBox = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            layoutParams = LinearLayout.LayoutParams(
                0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f
            )
        }
        nameBox.addView(TextView(this).apply {
            text = item.name
            textSize = 14f
            setTextColor(Color.parseColor(if (night) "#E9ECEF" else "#0C0F12"))
        })
        nameBox.addView(TextView(this).apply {
            text = item.description
            textSize = 11f
            setTextColor(Color.parseColor(if (night) "#B8BCC2" else "#6C757D"))
        })
        row.addView(nameBox)

        row.addView(Button(this).apply {
            text = if (granted) "已授权" else "去设置"
            isEnabled = !granted
            textSize = 12f
            minWidth = 0
            setOnClickListener { item.jumpTo(this@MainActivity) }
        })
        return row
    }


    private fun bootFlow() {
        // Activity recreation only observes the service; it never kills an initializing resident.
        ContextCompat.startForegroundService(this, Intent(this, NativeRuntimeService::class.java))


        val watcherStop = java.util.concurrent.atomic.AtomicBoolean(false)
        thread(name = "MobileLogWatcher") {
            val startTime = System.currentTimeMillis()
            while (!watcherStop.get()) {
                val elapsed = ((System.currentTimeMillis() - startTime) / 1000).toInt()
                val (progress, lastLine) = readLogProgress(RUA_LOG_PATH)
                uiHandler.post {

                    if (progress > splashProgress.progress) splashProgress.progress = progress
                    if (lastLine.isNotBlank()) {

                        splashStatus.text = "正在启动 Rua · ${elapsed}s"
                    } else {
                        splashStatus.text = "[${elapsed}s] 启动中..."
                    }
                }
                Thread.sleep(500)
            }
        }


        val pollTimeoutMs = 30_000L
        val launchStart = System.currentTimeMillis()
        val ready = PhoneUseBridge.awaitResidentPort(pollTimeoutMs) != null
        watcherStop.set(true)
        if (ready) {
            val elapsed = System.currentTimeMillis() - launchStart
            Log.i("RuaMobile", "[engine=native] ready in ${elapsed}ms")
            val consoleToken = awaitConsoleAuthToken()
            if (consoleToken == null) {
                val tail = readLastLines(RUA_LOG_PATH, 10)
                Log.e(
                    "RuaMobile",
                    "[engine=native] resident is listening but did not complete the private handshake"
                )
                uiHandler.post {
                    rootLayout.removeView(splashView)
                    showError(
                        "Rua 服务已启动，但未建立私有连接，Phone Use 不可用。\n\n" +
                            "请强行停止手Rua后重新打开。\n\n日志末尾:\n$tail"
                    )
                }
                return
            }
            uiHandler.post {
                splashProgress.progress = 100
                splashStatus.text = "✓ 就绪"
                rootLayout.removeView(splashView)
                if (!isDestroyed) showWebView(consoleToken)
            }
        } else {
            val tail = readLastLines(RUA_LOG_PATH, 10)
            val timeoutSec = pollTimeoutMs / 1000
            Log.e("RuaMobile", "[engine=native] not ready after ${timeoutSec}s")
            uiHandler.post {
                rootLayout.removeView(splashView)
                showError("rua ${timeoutSec}s 内未就绪\n\n日志末尾:\n$tail")
            }
        }
    }





    private fun readLogProgress(path: String): Pair<Int, String> = try {
        val lines = java.io.File(path).readLines()
        val all = lines.joinToString("\n")
        val markerProgress = when {
            all.contains("listening on") || all.contains("listener_ready") -> 95
            all.contains("router_built") || all.contains("lifecycle_started") -> 70
            all.contains("state_built") || all.contains("config_loaded") -> 40
            else -> 5
        }
        val contentLines = lines.count { it.isNotBlank() }
        val lineProgress = (10 + contentLines / 3).coerceAtMost(85)
        val progress = maxOf(markerProgress, lineProgress).coerceIn(2, 95)
        val lastLine = lines.lastOrNull { it.isNotBlank() }?.trim() ?: ""
        Pair(progress, lastLine)
    } catch (_: Exception) {
        Pair(0, "")
    }

    private fun readLastLines(path: String, n: Int): String = try {
        val lines = java.io.File(path).readLines()
        lines.takeLast(n).joinToString("\n")
    } catch (_: Exception) { "(no log)" }


    private fun showSplash(initialStatus: String = "准备中...") {
        val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
        val foreground = Color.parseColor(if (night) "#E9ECEF" else "#0C0F12")
        val secondary = Color.parseColor(if (night) "#B8BCC2" else "#6C757D")
        val layout = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            setPadding(64, 64, 64, 64)
            setBackgroundColor(Color.parseColor(if (night) "#1F1F1F" else "#FFFFFF"))
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }
        val title = TextView(this).apply {
            text = "手Rua"
            textSize = 28f
            setTextColor(foreground)
            gravity = Gravity.CENTER
        }

        splashProgress = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            max = 100
            progress = 0
            isIndeterminate = true
            layoutParams = LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT
            ).apply { setMargins(64, 64, 64, 24) }
        }
        splashStatus = TextView(this).apply {
            text = initialStatus
            textSize = 14f
            setTextColor(secondary)
            gravity = Gravity.CENTER
            setPadding(48, 24, 48, 0)

            maxLines = 2
            isSingleLine = false
            ellipsize = android.text.TextUtils.TruncateAt.MIDDLE
        }
        val hint = TextView(this).apply {
            text = "首次启动将自动准备运行环境，请保持网络连接"
            textSize = 12f
            setTextColor(secondary)
            gravity = Gravity.CENTER
            setPadding(48, 48, 48, 0)
        }
        layout.addView(title)
        layout.addView(splashProgress)
        layout.addView(splashStatus)
        layout.addView(hint)
        splashView = layout
        rootLayout.addView(splashView)
    }



    /**
     * Waits off the UI thread for the resident to complete the private handshake. Absence of a
     * token is reported, never fabricated, so the console keeps its normal login path when the
     * private channel is unavailable.
     */
    private fun awaitConsoleAuthToken(
        timeoutMs: Long = AUTH_TOKEN_WAIT_TIMEOUT_MS,
    ): String? =
        awaitAcceptedConsoleToken(timeoutMs, PhoneUseBridge::awaitAuthToken, { token, budget ->
            val connection = URL("http://127.0.0.1:$RUA_PORT/api/auth/verify")
                .openConnection() as HttpURLConnection
            try {
                connection.connectTimeout = (budget / 2).coerceAtLeast(1)
                connection.readTimeout = (budget / 2).coerceAtLeast(1)
                connection.setRequestProperty("Authorization", "Bearer $token")
                connection.instanceFollowRedirects = false
                connection.responseCode == HttpURLConnection.HTTP_OK
            } catch (_: Exception) {
                false
            } finally {
                connection.disconnect()
            }
        }).also { token ->
            if (token == null) {
                Log.w(TAG, "resident authentication not ready within ${timeoutMs}ms")
            }
        }

    @SuppressLint("SetJavaScriptEnabled")
    private fun showWebView(consoleAuthToken: String?) {
        if (!RuntimeToolchains.compatible(this)) {
            showRuntimeSetup(consoleAuthToken)
            return
        }
        // Fail closed before a single byte of console HTML runs. An unseeded WebView would fall
        // back to the console password login against a resident this bridge never authenticated,
        // which is exactly the unauthenticated access the private channel replaces.
        if (!canLoadAuthenticatedConsole(
                WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT),
                consoleAuthToken,
            )
        ) {
            Log.e(
                TAG,
                "refusing to load the console without a seeded private auth token",
            )
            showError(
                "无法建立私有认证连接，Phone Use 不可用。\n\n" +
                    "请更新 Android System WebView 后重新打开手Rua。"
            )
            return
        }
        val wv = WebView(this).apply {
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
            configureConsoleWebViewSettings(settings)
            webViewClient = object : WebViewClient() {
                override fun shouldOverrideUrlLoading(
                    v: WebView?,
                    request: android.webkit.WebResourceRequest?
                ): Boolean = routeExternalNavigation(request?.url?.toString())

                override fun onPageStarted(v: WebView?, url: String?, f: android.graphics.Bitmap?) {
                    errorView?.visibility = View.GONE
                }
                override fun onPageFinished(v: WebView?, url: String?) {
                    super.onPageFinished(v, url)
                    dispatchPendingNotificationRuaUri()
                }
                override fun onReceivedError(
                    v: WebView?, req: android.webkit.WebResourceRequest?,
                    err: android.webkit.WebResourceError?
                ) {
                    super.onReceivedError(v, req, err)
                    if (req?.isForMainFrame == true) showError(err?.description?.toString() ?: "load error")
                }
            }
            webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(
                    webView: WebView?,
                    filePathCallback: ValueCallback<Array<Uri>>?,
                    fileChooserParams: FileChooserParams?
                ): Boolean {

                    fileChooserCallback?.onReceiveValue(null)
                    fileChooserCallback = filePathCallback
                    val intent = fileChooserParams?.createIntent() ?: Intent(Intent.ACTION_GET_CONTENT).apply {
                        type = "*/*"
                        addCategory(Intent.CATEGORY_OPENABLE)
                    }
                    return try {
                        fileChooserLauncher.launch(intent)
                        true
                    } catch (ex: Exception) {
                        Log.e(TAG, "file chooser launch failed", ex)
                        fileChooserCallback = null
                        false
                    }
                }
            }
            val isNight = (resources.configuration.uiMode and
                Configuration.UI_MODE_NIGHT_MASK) == Configuration.UI_MODE_NIGHT_YES
            setBackgroundColor(if (isNight) Color.BLACK else Color.WHITE)





            @Suppress("DEPRECATION")
            if (WebViewFeature.isFeatureSupported(WebViewFeature.FORCE_DARK)) {
                WebSettingsCompat.setForceDark(
                    settings,
                    if (isNight) WebSettingsCompat.FORCE_DARK_ON
                    else WebSettingsCompat.FORCE_DARK_OFF,
                )
                if (WebViewFeature.isFeatureSupported(WebViewFeature.FORCE_DARK_STRATEGY)) {
                    WebSettingsCompat.setForceDarkStrategy(
                        settings,
                        WebSettingsCompat.DARK_STRATEGY_WEB_THEME_DARKENING_ONLY,
                    )
                }
            }
            WebView.setWebContentsDebuggingEnabled(true)


            addJavascriptInterface(
                MobileFileBridge(this@MainActivity, this@MainActivity::beginSaveBlobWithPicker),
                MobileFileBridge.JS_NAME,
            )
            addJavascriptInterface(MobileStorageBridge(this@MainActivity), MobileStorageBridge.JS_NAME)
            addJavascriptInterface(MobileRuntimeBridge(
                { runOnUiThread { showRuntimeManagement() } },
                { runOnUiThread { showPermissionsDialog(firstRun = false) } },
            ), MobileRuntimeBridge.JS_NAME)
            addJavascriptInterface(
                MobileNotificationBridge(
                    this@MainActivity,
                    this@MainActivity::requestNotificationPermission,
                ),
                MobileNotificationBridge.JS_NAME,
            )
        }
        rootLayout.addView(wv)
        webView = wv
        uiHandler.removeCallbacks(runtimeStatusRefresh)
        SyncBackgroundWorker.refreshScheduleAsync(this, ruaUrl)

        val ev = buildErrorView()
        ev.visibility = View.GONE
        rootLayout.addView(ev)
        errorView = ev

        if (!installDocumentStartAuth(wv, consoleAuthToken)) {
            Log.e(TAG, "console auth seeding failed; refusing to load the console")
            rootLayout.removeView(wv)
            webView = null
            showError(
                "无法建立私有认证连接，Phone Use 不可用。\n\n" +
                    "请更新 Android System WebView 后重新打开手Rua。"
            )
            return
        }
        wv.loadUrl(ruaUrl)
    }

    /**
     * Seeds the existing console `rua_auth_token` before console code runs, so the bundled UI
     * never asks the user for a password. Fails closed: when the document-start script feature is
     * unavailable or the resident never handshakes, no script (and no unsafe evaluateJavascript
     * fallback) is injected.
     */
    /** Returns false when the token could not be seeded; the caller must then refuse to load. */
    private fun installDocumentStartAuth(wv: WebView, token: String?): Boolean {
        // showWebView already refused to load the console unless both preconditions hold.
        if (token == null) return false
        return try {
            WebViewCompat.addDocumentStartJavaScript(
                wv,
                PhoneUseBridge.documentStartAuthScript(token),
                setOf(PhoneUseBridge.ownedOrigin(RUA_PORT)),
            )
            true
        } catch (ex: Throwable) {
            Log.e(TAG, "failed to register document-start auth script", ex)
            false
        }
    }

    /**
     * Loads the bundled console in place, hands plain web links to an external browser, and blocks
     * every other scheme. The WebView carries native JS bridges, so foreign pages and `intent://`
     * style handoffs must never run inside it.
     */
    private fun routeExternalNavigation(url: String?): Boolean {
        if (isOwnedConsoleOrigin(url, RUA_PORT)) return false
        val target = url ?: return false
        if (!isHttpUrl(target)) {
            Log.w(TAG, "blocked non-http navigation from the console WebView")
            return true
        }
        Log.i(TAG, "routing non-console navigation to external browser")
        return try {
            startActivity(
                Intent(Intent.ACTION_VIEW, Uri.parse(target)).apply {
                    addCategory(Intent.CATEGORY_BROWSABLE)
                }
            )
            true
        } catch (ex: Exception) {
            Log.w(TAG, "failed to hand off external navigation", ex)
            true
        }
    }

    private fun buildErrorView(): View {
        val container = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(48, 200, 48, 48)
            setBackgroundColor(Color.WHITE)
            layoutParams = FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.MATCH_PARENT
            )
        }
        val title = TextView(this).apply {
            text = "无法连接 rua"
            textSize = 18f
            setTextColor(Color.BLACK)
        }
        errorMsg = TextView(this).apply {
            textSize = 13f
            setPadding(0, 24, 0, 32)
            setTextColor(Color.DKGRAY)
        }
        val retry = Button(this).apply {
            text = "重试"
            setOnClickListener {
                errorView?.visibility = View.GONE
                if (webView != null) {
                    webView?.loadUrl(ruaUrl)
                } else {
                    showSplash("重新检测...")
                    thread(name = "MobileBoot") { bootFlow() }
                }
            }
        }
        container.addView(title)
        container.addView(errorMsg)
        container.addView(retry)
        return container
    }

    private fun showError(msg: String) {
        if (errorView == null || !::errorMsg.isInitialized) {
            if (errorView != null) rootLayout.removeView(errorView)
            val ev = buildErrorView()
            rootLayout.addView(ev)
            errorView = ev
        }
        errorMsg.text = if (TextUtils.isEmpty(msg)) "(no detail)" else msg
        errorView?.visibility = View.VISIBLE
    }

    @Suppress("DEPRECATION")
    override fun onBackPressed() {
        // Back dismisses Console overlays, never routes or the resident app.
        // The system IME consumes Back first while the keyboard is visible.
        webView?.evaluateJavascript(
            "window.dispatchEvent(new Event('rua:back'))",
            null,
        )
    }
}
