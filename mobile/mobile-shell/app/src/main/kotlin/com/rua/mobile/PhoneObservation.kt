package com.rua.mobile

import android.accessibilityservice.AccessibilityService
import android.graphics.Rect
import android.os.SystemClock
import android.view.WindowManager
import android.view.accessibility.AccessibilityNodeInfo
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

internal data class PhoneObservationRecord(
    val id: String,
    val owner: String,
    val createdAt: Long,
    val result: JSONObject,
)

/** Bounded, instance-local observations. Tokens carry no authority outside this store. */
internal class PhoneObservations(
    private val clock: () -> Long = SystemClock::elapsedRealtime,
) {
    val generation: String = UUID.randomUUID().toString()
    private val records = LinkedHashMap<String, PhoneObservationRecord>()

    @Synchronized
    fun remember(owner: String, result: JSONObject): JSONObject {
        require(owner.isNotBlank() && owner.length <= 128) { "invalid observation owner" }
        records.entries.removeAll { clock() - it.value.createdAt > MAX_AGE_MS || it.value.owner == owner }
        while (records.size >= MAX_OBSERVATIONS) records.remove(records.keys.first())
        val id = UUID.randomUUID().toString()
        result.put("observation_id", id).put("service_generation", generation)
        val nodes = result.getJSONArray("nodes")
        for (index in 0 until nodes.length()) nodes.getJSONObject(index).put("token", "$id:$index")
        PhoneTreeCapture.trimToPayloadLimit(result, nodes, MAX_PAYLOAD_BYTES)
        records[id] = PhoneObservationRecord(id, owner, clock(), JSONObject(result.toString()))
        return result
    }

    @Synchronized
    fun consume(owner: String, id: String): PhoneObservationRecord {
        val record = records[id] ?: throw IllegalArgumentException("unknown observation; observe again")
        require(record.owner == owner) { "observation belongs to another Chat" }
        records.remove(id)
        require(clock() - record.createdAt <= MAX_AGE_MS) { "expired observation; observe again" }
        return record
    }

    @Synchronized
    fun clear() = records.clear()

    @Synchronized
    fun invalidateOwner(owner: String) { records.entries.removeAll { it.value.owner == owner } }

    @Synchronized
    fun invalidateOtherWindows(packageName:String,windowId:Int) {
        records.entries.removeAll { it.value.result.optString("package_name") != packageName || it.value.result.optInt("window_id",-1) != windowId }
    }

    companion object {
        const val MAX_AGE_MS = 30_000L
        const val MAX_OBSERVATIONS = 32
        /** Hard caps shared with the resident; the caller may pick any value up to these. */
        const val MAX_NODES = 120
        const val MAX_DEPTH = 32
        const val DEFAULT_NODES = 60
        const val DEFAULT_DEPTH = 24
        const val MAX_TEXT = 160
        /**
         * Traversal guard: a screen full of empty containers must not make the walk unbounded.
         * Visited containers are cheap and do not consume the emitted-node budget.
         */
        const val MAX_VISITED = 4_000
        /** Serialized tree guard, kept below the resident's observe response frame limit. */
        const val MAX_PAYLOAD_BYTES = 200 * 1024
        /**
         * Screenshot observations must leave room for the base64 image inside the resident's
         * media response limit, so their tree budget is much smaller.
         */
        const val MAX_SCREENSHOT_PAYLOAD_BYTES = 48 * 1024
    }
}

/**
 * Decides whether an accessibility node earns a place in the observation.
 *
 * Pure layout containers carry nothing the model can read or act on, yet deep nesting
 * (marketplace chat pages nest a dozen or more empty FrameLayouts) used to consume the whole
 * node budget before traversal ever reached the real content. Such nodes are still traversed,
 * they just never count. An id alone is not enough: `android:id/content` on a bare container is
 * noise, while every meaningful element is either readable or actionable.
 */
internal object PhoneNodeFilter {
    fun isMeaningful(
        text: String,
        description: String,
        clickable: Boolean,
        editable: Boolean,
        scrollable: Boolean,
        checkable: Boolean,
        longClickable: Boolean,
    ): Boolean =
        text.isNotBlank() || description.isNotBlank() ||
            clickable || editable || scrollable || checkable || longClickable
}

internal object PhoneTreeCapture {
    @Suppress("DEPRECATION")
    fun targetRoot(service: AccessibilityService): android.view.accessibility.AccessibilityNodeInfo? {
        val windows = service.windows
        val target = windows.firstOrNull { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_APPLICATION && it.isActive }
            ?: windows.firstOrNull { it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_APPLICATION && it.isFocused }
        target?.root?.let { return it }
        val root = service.rootInActiveWindow ?: return null
        if (windows.any { it.id == root.windowId && it.type == android.view.accessibility.AccessibilityWindowInfo.TYPE_ACCESSIBILITY_OVERLAY }) {
            root.recycle()
            return null
        }
        return root
    }
    @Suppress("DEPRECATION")
    fun capture(service: AccessibilityService, maxNodes: Int, maxDepth: Int): JSONObject {
        require(maxNodes in 1..PhoneObservations.MAX_NODES) { "invalid node limit" }
        require(maxDepth in 1..PhoneObservations.MAX_DEPTH) { "invalid depth limit" }
        val root = targetRoot(service) ?: throw IllegalStateException("no accessible foreground window")
        try {
            val display = (service.getSystemService(android.content.Context.WINDOW_SERVICE) as WindowManager).defaultDisplay
            val size = android.graphics.Point()
            display.getRealSize(size)
            val rotation = display.rotation
            val windowId = root.windowId
            val packageName = root.packageName?.toString().orEmpty()
            val nodes = JSONArray()
            var truncated = false
            var visited = 0
            fun visit(node: AccessibilityNodeInfo, path: List<Int>, depth: Int) {
                if (nodes.length() >= maxNodes) { truncated = true; return }
                if (visited >= PhoneObservations.MAX_VISITED) { truncated = true; return }
                visited++
                val bounds = Rect()
                node.getBoundsInScreen(bounds)
                if (!node.isVisibleToUser || bounds.isEmpty) return
                val text = if (node.isPassword) "" else node.text?.toString().orEmpty().take(PhoneObservations.MAX_TEXT)
                val description = if (node.isPassword) "" else node.contentDescription?.toString().orEmpty().take(PhoneObservations.MAX_TEXT)
                // Structural containers are traversed but never emitted, so they cannot starve
                // the budget before the walk reaches real content.
                if (PhoneNodeFilter.isMeaningful(text, description, node.isClickable, node.isEditable, node.isScrollable, node.isCheckable, node.isLongClickable)) {
                    nodes.put(JSONObject()
                        .put("path", JSONArray(path))
                        .put("text", text)
                        .put("description", description)
                        .put("view_id", node.viewIdResourceName?.take(200).orEmpty())
                        .put("class_name", node.className?.toString()?.take(120).orEmpty())
                        .put("bounds", JSONArray(listOf(bounds.left, bounds.top, bounds.right, bounds.bottom)))
                        .put("clickable", node.isClickable).put("editable", node.isEditable)
                        .put("scrollable", node.isScrollable).put("enabled", node.isEnabled)
                        .put("visible", node.isVisibleToUser).put("password", node.isPassword))
                }
                if (depth >= maxDepth) { if (node.childCount > 0) truncated = true; return }
                for (index in 0 until node.childCount) {
                    if (nodes.length() >= maxNodes) { truncated = true; break }
                    val child = node.getChild(index) ?: continue
                    try { visit(child, path + index, depth + 1) } finally { child.recycle() }
                }
            }
            visit(root, emptyList(), 0)
            val current = targetRoot(service) ?: throw IllegalStateException("window changed during observation")
            try {
                check(current.windowId == windowId && current.packageName?.toString().orEmpty() == packageName) { "window changed during observation" }
                check(display.rotation == rotation) { "display changed during observation" }
            } finally { current.recycle() }
            val result = JSONObject().put("package_name", packageName).put("window_id", windowId)
                .put("width", size.x).put("height", size.y).put("rotation", rotation)
                .put("nodes", nodes).put("truncated", truncated)
            trimToPayloadLimit(result, nodes, PhoneObservations.MAX_PAYLOAD_BYTES)
            return result
        } finally { root.recycle() }
    }

    /**
     * Keeps the serialized observation inside the resident's response frame budget.
     *
     * Trailing nodes are dropped, since depth-first order puts the deepest, least likely to be
     * hit targets last. Sizes are monotonic in the retained count, so binary search avoids
     * re-serializing the whole tree once per removed node on the main thread.
     */
    fun trimToPayloadLimit(result: JSONObject, nodes: JSONArray, limitBytes: Int) {
        if (nodes.length() == 0) return
        if (payloadBytes(result) <= limitBytes) return
        val retained = (0 until nodes.length()).map { nodes.getJSONObject(it) }
        result.put("truncated", true)
        var low = 0
        var high = retained.size
        var keep = 0
        while (low <= high) {
            val middle = (low + high) / 2
            replaceNodes(nodes, retained, middle)
            if (payloadBytes(result) <= limitBytes) { keep = middle; low = middle + 1 } else high = middle - 1
        }
        replaceNodes(nodes, retained, keep)
    }

    private fun replaceNodes(nodes: JSONArray, retained: List<JSONObject>, count: Int) {
        while (nodes.length() > count) nodes.remove(nodes.length() - 1)
        while (nodes.length() < count) nodes.put(retained[nodes.length()])
    }

    private fun payloadBytes(result: JSONObject): Int = result.toString().toByteArray(Charsets.UTF_8).size
}
