package com.rua.mobile

import android.content.Context
import android.content.Intent
import android.graphics.PixelFormat
import android.graphics.Rect
import android.graphics.drawable.GradientDrawable
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import java.util.concurrent.FutureTask
import java.util.concurrent.TimeUnit

internal const val PHONE_USE_RETURN_ACTION = "com.rua.mobile.action.RETURN_FROM_PHONE_USE"

/** The visible device session is independent of the short operation reservation. */
internal object PhoneDeviceControl {
    val sessions = PhoneSessionState()
    val state = PhoneControlState { sessions.isCurrent(it) }
    private val handler = Handler(Looper.getMainLooper())
    private val lifecycleLock = Any()
    private var generation: String? = null
    private var context: Context? = null
    private var stopped: ((PhoneInvocation) -> Unit)? = null
    private var panel: View? = null
    private var manager: WindowManager? = null
    private var windowContext: Context? = null
    private var renderedSessionId: String? = null
    private var activityLabel: TextView? = null
    private var recentOperation: PhoneOperation? = null
    private var activityStartedAt = 0L
    private const val MIN_ACTIVITY_MS = 700L

    fun initialize(context: Context, generation: String, onStopped: (PhoneInvocation) -> Unit) = synchronized(lifecycleLock) {
        clear()
        state.reset()
        this.context = context.applicationContext
        this.generation = generation
        stopped = onStopped
        sessions.reset()
        sessions.onChanged = { handler.post { refreshSurface() } }
        state.onOperationChanged = { handler.post { refreshSurface() } }
    }

    fun begin(invocation: PhoneInvocation, requestId: String, expectedGeneration: String, action: String? = null): PhoneOperation {
        val operation = synchronized(lifecycleLock) {
            requireGeneration(expectedGeneration)
            val app = context ?: error("Phone Use host unavailable")
            check(MobileAccessibilityService.instance != null || Settings.canDrawOverlays(app)) {
                "Phone Use requires system overlay permission when the accessibility service is unavailable"
            }
            val reserved = state.begin(invocation, requestId, action)
            try { sessions.activate(invocation) }
            catch (error: Exception) { state.cancel(invocation, requestId); state.finish(reserved); throw error }
            recentOperation = reserved
            activityStartedAt = SystemClock.uptimeMillis()
            // Even a synchronous observation must remain visible for a rendered frame.
            handler.postDelayed({ refreshSurface() }, MIN_ACTIVITY_MS)
            reserved
        }
        try {
            onMain {
                synchronized(lifecycleLock) {
                    requireGeneration(expectedGeneration)
                    check(state.current() === operation) { "Phone Use operation is no longer active" }
                    refreshSurface()
                    check(sessions.isCurrent(invocation) && panel != null) { "Phone Use session control unavailable" }
                }
            }
        } catch (error: Exception) {
            fail(operation)
            state.finish(operation)
            throw error
        }
        return operation
    }

    fun release(invocation: PhoneInvocation, expectedGeneration: String): Boolean = synchronized(lifecycleLock) {
        requireGeneration(expectedGeneration)
        val released = sessions.release(invocation)
        if (released) {
            cancelOperation(invocation)
            returnToChat(invocation, expectedGeneration)
        }
        released
    }

    private fun returnToChat(invocation: PhoneInvocation, expectedGeneration: String) {
        val app = context
        handler.post {
            synchronized(lifecycleLock) {
                // A queued return must not interrupt a new session or a restarted bridge.
                if (generation == expectedGeneration && sessions.current() == null && app != null) {
                    refreshSurface()
                    val intent = Intent(app, MainActivity::class.java).apply {
                        action = PHONE_USE_RETURN_ACTION
                        putExtra(MobileNotificationBridge.EXTRA_RUA_URI, "rua:/${invocation.chatAddress.apiPath()}")
                        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                    }
                    try { (MobileAccessibilityService.instance ?: app).startActivity(intent) }
                    catch (error: Exception) { android.util.Log.w("PhoneControl", "Could not return to Rua Chat", error) }
                }
            }
        }
    }

    fun bindTurn(invocation: PhoneInvocation, expectedGeneration: String): Boolean = synchronized(lifecycleLock) {
        requireGeneration(expectedGeneration)
        val changed = sessions.bindTurn(invocation)
        if (changed) state.current()?.takeIf { it.invocation != invocation }?.let {
            state.cancel(it.invocation, it.requestId)
            PhoneCamera.cancel(it.invocation, it.requestId)
        }
        changed
    }

    fun cancel(invocation: PhoneInvocation, requestId: String, expectedGeneration: String) = synchronized(lifecycleLock) {
        requireGeneration(expectedGeneration)
        state.cancel(invocation, requestId)
        state.current()?.takeIf { it.invocation == invocation && it.requestId == requestId }?.let {
            PhoneCamera.cancel(invocation, requestId)
        }
    }

    fun fail(operation: PhoneOperation) {
        state.ifCurrent(operation) {
            sessions.release(operation.invocation)
            state.cancel(operation.invocation, operation.requestId)
        }
    }

    fun fail(invocation: PhoneInvocation, requestId: String, expectedGeneration: String) = synchronized(lifecycleLock) {
        if (generation != expectedGeneration) return@synchronized
        state.current()?.takeIf { it.invocation == invocation && it.requestId == requestId }?.let {
            fail(it)
            state.finish(it)
        }
    }

    private fun cancelOperation(invocation: PhoneInvocation) {
        state.current()?.takeIf { it.invocation == invocation }?.let {
            state.cancel(it.invocation, it.requestId)
            PhoneCamera.cancel(invocation, it.requestId)
        }
        MobileAccessibilityService.instance?.observations?.invalidateOwner(invocation.owner)
    }

    fun stopSession(expectedSessionId: String) {
        val request = synchronized(lifecycleLock) {
            val invocation = sessions.stop(expectedSessionId) ?: return@synchronized null
            cancelOperation(invocation)
            generation?.let { returnToChat(invocation, it) }
            invocation to stopped
        } ?: return
        if (Looper.myLooper() == Looper.getMainLooper()) refreshSurface() else refresh()
        request.second?.invoke(request.first)
    }

    fun clear() = synchronized(lifecycleLock) {
        val invocation = sessions.current()?.invocation
        sessions.clear()
        recentOperation = null
        invocation?.let(::cancelOperation)
        state.current()?.let { state.cancel(it.invocation, it.requestId); state.finish(it) }
        handler.post { refreshSurface() }
    }

    fun close(expectedGeneration: String) = synchronized(lifecycleLock) {
        if (generation != expectedGeneration) return@synchronized
        clear()
        sessions.onChanged = null
        state.onOperationChanged = null
        stopped = null
        context = null
        generation = null
        val oldPanel = panel
        val oldManager = manager
        handler.post {
            oldPanel?.let { runCatching { oldManager?.removeView(it) } }
            if (panel === oldPanel) { panel = null; manager = null; windowContext = null; renderedSessionId = null; activityLabel = null }
        }
    }

    fun refresh() { handler.post { refreshSurface() } }

    fun hasSurface(invocation: PhoneInvocation): Boolean = sessions.isCurrent(invocation) && panel != null

    fun bounds(): Rect = Rect().also { bounds ->
        panel?.let { view ->
            val location = IntArray(2)
            view.getLocationOnScreen(location)
            bounds.set(location[0], location[1], location[0] + view.width, location[1] + view.height)
        }
    }

    private fun refreshSurface() = synchronized(lifecycleLock) {
        val lease = sessions.current()
        val app = context
        if (lease == null || app == null) { hide(); return@synchronized }
        val service = MobileAccessibilityService.instance
        val displayContext = service ?: app
        if (panel != null && windowContext === displayContext && renderedSessionId == lease.sessionId) {
            activityLabel?.text = activityText(lease.invocation)
            return@synchronized
        }
        hide()
        try {
            check(service != null || Settings.canDrawOverlays(app)) {
                "Phone Use requires system overlay permission when the accessibility service is unavailable"
            }
            val density = displayContext.resources.displayMetrics.density
            fun dp(value: Int) = (value * density).toInt()
            val view = LinearLayout(displayContext).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(12), dp(4), dp(4), dp(10))
                background = GradientDrawable().apply { setColor(0xff353238.toInt()); cornerRadius = dp(24).toFloat() }
                elevation = dp(4).toFloat()
                addView(LinearLayout(displayContext).apply {
                    orientation = LinearLayout.HORIZONTAL
                    gravity = Gravity.CENTER_VERTICAL
                    addView(TextView(displayContext).apply {
                        text = "Rua 正在操作手机"
                        contentDescription = "Rua 正在操作手机"
                        textSize = 14f
                        setTextColor(android.graphics.Color.WHITE)
                    })
                    addView(Button(displayContext).apply {
                        text = "停止"
                        contentDescription = "停止当前 Phone Use 会话"
                        isAllCaps = false
                        setOnClickListener { stopSession(lease.sessionId) }
                    })
                })
                addView(TextView(displayContext).apply {
                    text = activityText(lease.invocation)
                    textSize = 12f
                    setTextColor(0xffdedbe2.toInt())
                    setPadding(0, 0, dp(8), 0)
                    activityLabel = this
                })
            }
            val type = if (service != null) WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY
                else WindowManager.LayoutParams.TYPE_APPLICATION_OVERLAY
            val params = WindowManager.LayoutParams(WindowManager.LayoutParams.WRAP_CONTENT, WindowManager.LayoutParams.WRAP_CONTENT,
                type, WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE, PixelFormat.TRANSLUCENT).apply {
                gravity = Gravity.TOP or Gravity.END
                x = dp(12); y = dp(56)
            }
            val windows = displayContext.getSystemService(Context.WINDOW_SERVICE) as WindowManager
            windows.addView(view, params)
            manager = windows
            panel = view
            windowContext = displayContext
            renderedSessionId = lease.sessionId
        } catch (error: Exception) {
            android.util.Log.e("PhoneControl", "Phone Use session control unavailable", error)
            sessions.release(lease.invocation)
            cancelOperation(lease.invocation)
        }
    }

    private fun activityText(invocation: PhoneInvocation): String {
        val operation = (state.current() ?: recentOperation?.takeIf {
            SystemClock.uptimeMillis() - activityStartedAt < MIN_ACTIVITY_MS
        })?.takeIf { it.invocation == invocation && !it.cancelled.get() }
            ?: return "Agent 正在等待下一步"
        return when (operation.action) {
            "observe", "screenshot" -> "Agent 正在观看屏幕"
            "click" -> "Agent 正在点击屏幕"
            "long_press" -> "Agent 正在长按屏幕"
            "type_text" -> "Agent 正在输入文字"
            "swipe" -> "Agent 正在滑动屏幕"
            "back" -> "Agent 正在返回上一页"
            "home" -> "Agent 正在返回桌面"
            "launch_app" -> "Agent 正在打开应用"
            "take_photo" -> "Agent 正在拍照"
            else -> "Agent 正在操作手机"
        }
    }

    private fun requireGeneration(expected: String) {
        check(generation == expected) { "Phone Use bridge generation changed" }
    }

    private fun hide() {
        panel?.let { view -> runCatching { manager?.removeView(view) } }
        panel = null
        manager = null
        windowContext = null
        renderedSessionId = null
        activityLabel = null
    }

    private fun onMain(action: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) action()
        else FutureTask<Unit> { action() }.also { handler.post(it) }.get(3, TimeUnit.SECONDS)
    }
}
