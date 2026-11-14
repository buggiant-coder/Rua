package com.rua.mobile

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.graphics.Path
import android.graphics.Rect
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONObject
import java.util.concurrent.CompletableFuture
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

internal class PhoneOperation(val invocation: PhoneInvocation, val requestId: String, val action: String? = null) {
    val owner: String get() = invocation.owner
    var expectedPackage: String? = null
    var previousWindow: Int? = null
    val cancelled = AtomicBoolean(false)
    val submitted = AtomicBoolean(false)
    val result = CompletableFuture<JSONObject>()
}

internal class PhoneControlState(private val sessionIsCurrent: (PhoneInvocation) -> Boolean) {
    private val active = AtomicReference<PhoneOperation?>(null)
    private val cancelledIds = LinkedHashSet<Pair<PhoneInvocation, String>>()
    var onOperationChanged: (() -> Unit)? = null
    fun current(): PhoneOperation? = active.get()
    @Synchronized
    fun begin(invocation: PhoneInvocation, id: String, action: String? = null): PhoneOperation {
        val owner = invocation.owner
        require(owner.isNotBlank() && owner.length <= 128 && id.isNotBlank() && id.length <= 200) { "Invalid invocation identity" }
        check(!cancelledIds.contains(invocation to id)) { "Phone Use request was cancelled" }
        val operation = PhoneOperation(invocation, id, action)
        check(active.compareAndSet(null, operation)) { "Phone Use busy; observe again before retrying" }
        onOperationChanged?.invoke()
        return operation
    }
    @Synchronized
    fun cancel(invocation: PhoneInvocation, id: String) {
        cancelledIds.add(invocation to id)
        while (cancelledIds.size > 256) cancelledIds.remove(cancelledIds.first())
        active.get()?.takeIf { it.invocation == invocation && it.requestId == id }?.let {
            it.cancelled.set(true)
            it.result.complete(JSONObject().put("state", if (it.submitted.get()) "outcome_unknown" else "cancelled").put("submitted", it.submitted.get()))
            onOperationChanged?.invoke()
        }
    }
    @Synchronized
    fun cancelCurrent() {
        active.get()?.let { cancel(it.invocation, it.requestId) }
    }
    @Synchronized
    fun <T> dispatch(operation: PhoneOperation, send: () -> T): T {
        check(active.get() === operation && !operation.cancelled.get() && sessionIsCurrent(operation.invocation)) { "Phone Use cancelled" }
        operation.submitted.set(true)
        return send()
    }
    fun finish(operation: PhoneOperation) {
        if (active.compareAndSet(operation, null)) onOperationChanged?.invoke()
    }
    @Synchronized
    fun ifCurrent(operation: PhoneOperation, action: () -> Unit) {
        if (active.get() === operation) action()
    }
    @Synchronized
    fun reset() {
        active.get()?.let { cancel(it.invocation, it.requestId); finish(it) }
        cancelledIds.clear()
    }
}

internal class PhoneController(private val service: MobileAccessibilityService) {
    val state = PhoneDeviceControl.state
    private val handler = Handler(Looper.getMainLooper())

    fun attach() {
        PhoneDeviceControl.refresh()
    }

    fun close() {
        if (MobileAccessibilityService.instance === service) PhoneDeviceControl.clear()
        service.observations.clear()
    }

    @Suppress("DEPRECATION")
    fun interact(invocation: PhoneInvocation, requestId: String, action: String, params: JSONObject, generation: String): CompletableFuture<JSONObject> {
        val owner = invocation.owner
        require(action in setOf("click", "long_press", "type_text", "swipe", "back", "home", "launch_app")) { "Unsupported action" }
        val op = PhoneDeviceControl.begin(invocation, requestId, generation, action)
        handler.post {
            try {
                check(!op.cancelled.get()) { "Phone Use cancelled" }
                check(PhoneDeviceControl.hasSurface(invocation)) { "Phone Use session control unavailable" }
                val record = service.observations.consume(owner, params.getString("observation_id"))
                val snapshot = record.result
                val root = PhoneTreeCapture.targetRoot(service) ?: error("No active window")
                try {
                    val display = (service.getSystemService(android.content.Context.WINDOW_SERVICE) as WindowManager).defaultDisplay
                    check(root.windowId == snapshot.getInt("window_id") && root.packageName?.toString() == snapshot.getString("package_name") && display.rotation == snapshot.getInt("rotation")) { "Window changed; observe again" }
                    val pkg = root.packageName?.toString().orEmpty()
                    if (action in setOf("back", "home", "launch_app")) {
                        op.previousWindow = root.windowId
                        check(!op.cancelled.get()) { "Phone Use cancelled" }
                        if (action == "launch_app") {
                            val target = params.getString("package_name")
                            check(target != service.packageName && target != "com.android.settings" && !target.contains("permissioncontroller")) { "Cannot launch control or authorization surfaces" }
                            check(PhoneApps.list(service).any { it.optString("package_name") == target }) { "App was not found in launchable applications" }
                            val intent = service.packageManager.getLaunchIntentForPackage(target) ?: error("App is not launchable")
                            op.expectedPackage = target
                            intent.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
                            state.dispatch(op) { service.startActivity(intent) }; complete(op,true)
                        } else {
                            if (action == "home") {
                                op.expectedPackage = service.packageManager.resolveActivity(android.content.Intent(android.content.Intent.ACTION_MAIN).addCategory(android.content.Intent.CATEGORY_HOME), android.content.pm.PackageManager.MATCH_DEFAULT_ONLY)?.activityInfo?.packageName
                            }
                            complete(op,state.dispatch(op) { service.performGlobalAction(if(action=="back") AccessibilityService.GLOBAL_ACTION_BACK else AccessibilityService.GLOBAL_ACTION_HOME) })
                        }
                        return@post
                    }
                    check(pkg != service.packageName && pkg != "com.android.settings" && !pkg.contains("permissioncontroller") && pkg != "com.android.systemui") { "Phone Use cannot operate its controls or system authorization surfaces" }
                    val token = params.optString("element_token")
                    var selected: AccessibilityNodeInfo? = null
                    val x: Float
                    val y: Float
                    if (token.isNotEmpty()) {
                        val nodes = snapshot.getJSONArray("nodes")
                        val item = (0 until nodes.length()).map { nodes.getJSONObject(it) }.firstOrNull { it.getString("token") == token } ?: error("Unknown node token")
                        var node = AccessibilityNodeInfo.obtain(root)
                        try {
                            val path = item.getJSONArray("path")
                            for (i in 0 until path.length()) {
                                val child = node.getChild(path.getInt(i)) ?: error("Node disappeared")
                                node.recycle(); node = child
                            }
                            check(node.refresh() && node.isVisibleToUser && node.isEnabled && !node.isPassword) { "Node is not actionable" }
                            val bounds = Rect(); node.getBoundsInScreen(bounds)
                            val expected = item.getJSONArray("bounds")
                            check(bounds == Rect(expected.getInt(0), expected.getInt(1), expected.getInt(2), expected.getInt(3)) && node.viewIdResourceName.orEmpty() == item.optString("view_id") && node.text?.toString().orEmpty().take(PhoneObservations.MAX_TEXT) == item.optString("text")) { "Node changed; observe again" }
                            x = bounds.exactCenterX(); y = bounds.exactCenterY(); selected = node; node = AccessibilityNodeInfo.obtain(root)
                        } finally { node.recycle() }
                    } else {
                        x = params.getDouble("x").toFloat(); y = params.getDouble("y").toFloat()
                    }
                    try {
                        check(x.isFinite() && y.isFinite() && x >= 0 && y >= 0 && x < snapshot.getInt("width") && y < snapshot.getInt("height")) { "Invalid coordinates" }
                        val overlay = PhoneDeviceControl.bounds()
                        check(!overlay.contains(x.toInt(), y.toInt())) { "Phone Use cannot click its stop controls" }
                        check(!op.cancelled.get()) { "Phone Use cancelled" }
                        if (action == "type_text") {
                            val text = params.getString("text")
                            require(text.length <= 2000) { "Text exceeds 2000 characters" }
                            val target = selected ?: error("Text input requires a node token")
                            check(target.isEditable) { "Target is not editable" }
                            val arguments = android.os.Bundle().apply { putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, text) }
                            complete(op, state.dispatch(op) { target.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, arguments) })
                        } else if (action == "long_press" && selected?.isLongClickable == true) {
                            complete(op, state.dispatch(op) { selected.performAction(AccessibilityNodeInfo.ACTION_LONG_CLICK) })
                        } else if (action == "click" && selected?.isClickable == true) {
                            val accepted = state.dispatch(op) { selected.performAction(AccessibilityNodeInfo.ACTION_CLICK) }
                            complete(op, accepted)
                        } else {
                            val path = Path().apply { moveTo(x, y) }
                            if (action == "swipe") {
                                val endX = params.getDouble("end_x").toFloat()
                                val endY = params.getDouble("end_y").toFloat()
                                check(endX.isFinite() && endY.isFinite() && endX >= 0 && endY >= 0 && endX < snapshot.getInt("width") && endY < snapshot.getInt("height")) { "Invalid swipe endpoint" }
                                val trajectory = Rect(minOf(x,endX).toInt()-1,minOf(y,endY).toInt()-1,maxOf(x,endX).toInt()+1,maxOf(y,endY).toInt()+1)
                                check(!Rect.intersects(trajectory, overlay)) { "Swipe intersects stop controls" }
                                path.lineTo(endX,endY)
                            }
                            val duration = when(action) { "long_press" -> 650L; "swipe" -> params.optLong("duration_ms",400); else -> 50L }
                            require(duration in 50..1000) { "Invalid gesture duration" }
                            val gesture = GestureDescription.Builder().addStroke(GestureDescription.StrokeDescription(path, 0, duration)).build()
                            check(!op.cancelled.get()) { "Phone Use cancelled" }
                            val accepted = state.dispatch(op) { service.dispatchGesture(gesture, object : AccessibilityService.GestureResultCallback() {
                                override fun onCompleted(gestureDescription: GestureDescription?) { complete(op, true) }
                                override fun onCancelled(gestureDescription: GestureDescription?) { complete(op, false) }
                            }, handler) }
                            if (!accepted) complete(op, false)
                        }
                    } finally { selected?.recycle() }
                } finally { root.recycle() }
            } catch (e: Exception) {
                op.result.complete(JSONObject().put("state", if(op.submitted.get()) "outcome_unknown" else "refused").put("submitted",op.submitted.get()).put("detail",e.message.orEmpty().take(300)))
                PhoneDeviceControl.fail(op)
                state.finish(op)
            }
        }
        handler.postDelayed({ if(!op.result.isDone) { PhoneDeviceControl.fail(op); state.ifCurrent(op) { state.cancel(invocation, requestId) } }; state.finish(op) }, 3_000)
        return op.result
    }

    private fun complete(op: PhoneOperation, accepted: Boolean, remaining: Int = 12) {
        if (op.cancelled.get()) { state.ifCurrent(op) { state.cancel(op.invocation, op.requestId) }; state.finish(op); return }
        handler.postDelayed({
            val result = JSONObject().put("state", if(accepted) "submitted" else "outcome_unknown").put("submitted",op.submitted.get()).put("effect_confirmed",false)
            if (!op.cancelled.get()) {
                if (op.previousWindow != null) {
                    @Suppress("DEPRECATION")
                    val settled = PhoneTreeCapture.targetRoot(service)?.let { current ->
                        try { if(op.expectedPackage != null) current.packageName?.toString() == op.expectedPackage else current.windowId != op.previousWindow }
                        finally { current.recycle() }
                    } ?: false
                    if (!settled) {
                        if (remaining > 0) { complete(op,accepted,remaining-1); return@postDelayed }
                        service.observations.clear()
                        result.put("observation_error","Navigation target not confirmed; observe again")
                        op.result.complete(result); state.finish(op); return@postDelayed
                    }
                }
                runCatching { service.observe(op.owner, 30, 10) }.onSuccess { result.put("observation",it) }.onFailure { result.put("observation_error",it.message.orEmpty().take(300)) }
            }
            op.result.complete(result); state.finish(op)
        }, 100)
    }
}
