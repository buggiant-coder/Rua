package com.rua.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import java.io.File
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/** Owns the native resident independently of Activity/WebView and guest readiness. */
class NativeRuntimeService : Service() {
    private val starting = AtomicBoolean(false)
    private val stopped = AtomicBoolean(false)
    private val processLock = Any()
    private var child: Process? = null
    private var phoneBridge: PhoneUseBridge? = null
    private var dnsSync: DnsSyncManager? = null
    private val mainHandler = Handler(Looper.getMainLooper())
    private var operationMonitor: ForegroundOperationMonitor? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private var snapshot: ForegroundOperationSnapshot? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        dnsSync = DnsSyncManager(this) {
            PhoneUseBridge.residentPort()?.let { port ->
                runCatching {
                    postResidentJson("${PhoneUseBridge.ownedOrigin(port)}/api/platform/network-change", "{}")
                }.onFailure { Log.w("RuaRuntime", "Network-change notification failed", it) }
            }
        }.also { it.start() }
        val manager = getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT >= 26) {
            manager.createNotificationChannel(NotificationChannel(CHANNEL, "手Rua", NotificationManager.IMPORTANCE_LOW))
        }
        startForeground(NOTIFICATION, buildNotification())
    }

    internal fun buildNotification(): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(com.rua.mobile.R.drawable.ic_notification).setContentTitle("手Rua")
            .setContentText(ForegroundOperationSnapshot.notificationText(snapshot))
            .setContentIntent(open).setOngoing(true).apply {
                ForegroundOperationSnapshot.notificationProgress(snapshot)?.let {
                    setProgress(100, it, false)
                }
            }.build()
    }

    internal fun applyForegroundOperationSnapshot(value: ForegroundOperationSnapshot) {
        if (stopped.get()) return
        snapshot = value
        if (value.locksRequired) {
            if (wakeLock == null) {
                wakeLock = getSystemService(PowerManager::class.java)
                    .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "rua:resident").apply {
                        setReferenceCounted(false)
                        acquire()
                    }
            }
            if (wifiLock == null) {
                @Suppress("DEPRECATION")
                val lock = getSystemService(WifiManager::class.java)
                    .createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "rua:resident")
                wifiLock = lock.apply { setReferenceCounted(false); acquire() }
            }
        } else releaseResidencyLocks()
        if (value.residencyState != "suspended") {
            if (multicastLock == null) {
                multicastLock = getSystemService(WifiManager::class.java)
                    .createMulticastLock("rua:discovery").apply {
                        setReferenceCounted(false)
                        acquire()
                    }
            }
        } else releaseMulticastLock()
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION, buildNotification())
    }

    private fun releaseResidencyLocks() {
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
        wifiLock?.let { if (it.isHeld) it.release() }
        wifiLock = null
    }

    private fun releaseMulticastLock() {
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
    }

    private fun monitorResident(port: Int) {
        mainHandler.post {
            if (stopped.get()) return@post
            processLifecycleReporter.residentChanged()
            operationMonitor = ForegroundOperationMonitor.forResidentService(
                PhoneUseBridge.ownedOrigin(port),
                object : ForegroundOperationMonitorListener {
                    override fun onSnapshot(snapshot: ForegroundOperationSnapshot) {
                        mainHandler.post { applyForegroundOperationSnapshot(snapshot) }
                    }
                    override fun onUnavailable() {
                        mainHandler.post {
                            if (!stopped.get()) {
                                releaseResidencyLocks()
                                releaseMulticastLock()
                            }
                        }
                    }
                },
            ).also { it.start() }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (starting.compareAndSet(false, true)) thread(name = "RuaNativeResident") {
            try {
                MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(applicationContext)
                val cleanup = MobileRuntimeProcess.prepareForPackagedServerRestart()
                check(cleanup.completed) { "Previous resident is still running" }
                NativePayload.prepare(applicationContext)
                synchronized(processLock) {
                    check(!stopped.get()) { "Host stopped" }
                    RuntimePreparation.start(this)
                }
                File(MainActivity.RUA_LOG_PATH).parentFile!!.mkdirs()
                check(initializeNativeRuaRoot { args ->
                    val init = synchronized(processLock) {
                        check(!stopped.get()) { "Host stopped" }
                        NativePayload.process(this, args).start().also { child = it }
                    }
                    if (!init.waitFor(30, TimeUnit.SECONDS)) {
                        init.destroyForcibly(); init.waitFor(); false
                    } else init.exitValue() == 0
                }) { "Rua initialization failed" }
                val resident = synchronized(processLock) {
                    check(!stopped.get()) { "Host stopped" }
                    phoneBridge = PhoneUseBridge.startForResident(applicationContext)
                    NativePayload.process(this,
                        buildNativeRuaArgs(NativePayload.consoleDirectory, BuildConfig.RUA_PORT))
                        .start().also { child = it }
                }
                val port = PhoneUseBridge.awaitResidentPort(30_000)
                    ?: error("Resident did not publish its private HTTP endpoint")
                monitorResident(port)
                Log.i(TAG, "private resident ready on loopback port=$port")
                Log.i(TAG, "native resident started pid owned by APK")
                val code = resident.waitFor()
                Log.i(TAG, "native resident exited code=$code")
            } catch (ex: Exception) {
                Log.e(TAG, "native resident failed", ex)
            } finally {
                stopSelf()
            }
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        stopped.set(true)
        operationMonitor?.stop()
        operationMonitor = null
        releaseResidencyLocks()
        releaseMulticastLock()
        RuntimePreparation.stop(this)
        dnsSync?.stop()
        val process = synchronized(processLock) {
            phoneBridge?.close()
            phoneBridge = null
            child.also { child = null }
        }
        thread(name = "RuaNativeStop") {
            process?.destroy()
            if (process != null && !process.waitFor(10, TimeUnit.SECONDS)) {
                process.destroyForcibly(); process.waitFor()
            }
        }
        super.onDestroy()
    }

    companion object {
        private const val TAG = "RuaNativeHost"
        private const val CHANNEL = "rua-runtime"
        private const val NOTIFICATION = 1201
    }
}
