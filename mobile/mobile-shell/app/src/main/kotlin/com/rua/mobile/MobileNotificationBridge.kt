package com.rua.mobile

import android.app.Activity
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.util.Log
import android.webkit.JavascriptInterface
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import java.util.concurrent.atomic.AtomicInteger
import org.json.JSONObject

internal data class MobileUserNotification(
    val targetDeviceId: String,
    val senderId: String,
    val senderName: String,
    val title: String,
    val body: String?,
    val ruaUri: String?,
)

internal fun parseUserNotificationPayload(payload: String): MobileUserNotification? {
    return try {
        val root = JSONObject(payload)
        val sender = root.getJSONObject("sender")
        val targetDeviceId = root.getString("target_device_id")
        val senderId = sender.getString("id")
        val senderName = sender.getString("name")
        val title = root.getString("title")
        if (
            targetDeviceId.isBlank() ||
            senderId.isBlank() ||
            senderName.isBlank() ||
            title.isBlank()
        ) {
            return null
        }
        MobileUserNotification(
            targetDeviceId = targetDeviceId,
            senderId = senderId,
            senderName = senderName,
            title = title,
            body = if (root.isNull("body")) null else root.optString("body").takeIf(String::isNotEmpty),
            ruaUri = if (root.isNull("rua_uri")) {
                null
            } else {
                root.optString("rua_uri").let(::safeRuaUriOrNull)
            },
        )
    } catch (_: Throwable) {
        null
    }
}

internal fun safeRuaUriOrNull(raw: String): String? {
    if (raw.isEmpty() || raw.trim() != raw || raw.any { it.isISOControl() }) return null
    return try {
        Uri.parse(raw).takeIf { uri ->
            uri.scheme == "rua" && !uri.host.isNullOrEmpty() && uri.userInfo == null && uri.port == -1
        }?.let { raw }
    } catch (_: Throwable) {
        null
    }
}

internal fun userNotificationIntent(
    context: Context,
    request: MobileUserNotification,
): Intent = Intent(context, MainActivity::class.java).apply {
    action = MobileNotificationBridge.ACTION_OPEN_USER_NOTIFICATION
    request.ruaUri?.let { putExtra(MobileNotificationBridge.EXTRA_RUA_URI, it) }
    addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
}

internal class NotificationPermissionGate(
    private val hasPermission: () -> Boolean,
    private val requestPermission: ((Boolean) -> Unit) -> Unit,
    private val present: (MobileUserNotification) -> Unit,
) {
    private var requestInFlight = false
    private var pending: MobileUserNotification? = null

    @Synchronized
    fun submit(notification: MobileUserNotification): Boolean {
        if (hasPermission()) {
            present(notification)
            return true
        }
        pending = notification
        if (requestInFlight) return true
        requestInFlight = true
        return try {
            requestPermission(::onPermissionResult)
            true
        } catch (_: Throwable) {
            requestInFlight = false
            pending = null
            false
        }
    }

    @Synchronized
    private fun onPermissionResult(granted: Boolean) {
        requestInFlight = false
        val notification = pending
        pending = null
        if (granted && notification != null) present(notification)
    }
}

class MobileNotificationBridge(
    private val activity: Activity,
    requestNotificationPermission: (((Boolean) -> Unit) -> Unit) = { callback -> callback(false) },
) {
    private val handler = Handler(activity.mainLooper)
    private val permissionGate = NotificationPermissionGate(
        hasPermission = { PermissionGuide.hasNotificationPermission(activity) },
        requestPermission = requestNotificationPermission,
        present = ::present,
    )

    @JavascriptInterface
    fun capability(): String = CAPABILITY

    @JavascriptInterface
    fun showUserNotification(payload: String): Boolean {
        val request = parseUserNotificationPayload(payload) ?: return false
        handler.post {
            try {
                permissionGate.submit(request)
            } catch (error: Throwable) {
                Log.w(TAG, "user notification presentation failed", error)
            }
        }
        return true
    }

    private fun present(request: MobileUserNotification) {
        if (!NotificationManagerCompat.from(activity).areNotificationsEnabled()) return
        val manager = activity.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            manager.createNotificationChannel(
                NotificationChannel(
                    CHANNEL_ID,
                    "Rua user notifications",
                    NotificationManager.IMPORTANCE_DEFAULT,
                ).apply {
                    description = "Best-effort alerts requested by Rua features"
                }
            )
        }

        val notificationId = nextId.getAndIncrement()
        val pendingIntent = PendingIntent.getActivity(
            activity,
            notificationId,
            userNotificationIntent(activity, request),
            PendingIntent.FLAG_UPDATE_CURRENT or
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) {
                    PendingIntent.FLAG_IMMUTABLE
                } else {
                    0
                },
        )
        val builder = NotificationCompat.Builder(activity, CHANNEL_ID)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(request.title)
            .setContentText(request.body ?: request.senderName)
            .setSubText(request.senderName)
            .setAutoCancel(true)
            .setContentIntent(pendingIntent)
            .setPriority(NotificationCompat.PRIORITY_DEFAULT)
        request.body?.let { builder.setStyle(NotificationCompat.BigTextStyle().bigText(it)) }
        NotificationManagerCompat.from(activity).notify(notificationId, builder.build())
    }

    companion object {
        const val JS_NAME = "RuaNotificationNative"
        const val CAPABILITY = "user-notification-v1"
        private const val TAG = "MobileNotification"
        const val CHANNEL_ID = "rua_user_notifications"
        const val ACTION_OPEN_USER_NOTIFICATION =
            "com.rua.mobile.action.OPEN_USER_NOTIFICATION"
        const val EXTRA_RUA_URI = "com.rua.mobile.extra.RUA_URI"
        private val nextId = AtomicInteger(20_000)
    }
}
