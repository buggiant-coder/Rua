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
        while(result.toString().toByteArray(Charsets.UTF_8).size > 60*1024 && nodes.length()>0) {
            nodes.remove(nodes.length()-1); result.put("truncated",true)
        }
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
        const val MAX_NODES = 40
        const val MAX_DEPTH = 12
        const val MAX_TEXT = 160
    }
}

internal object PhoneTreeCapture {
    @Suppress("DEPRECATION")
    fun capture(service: AccessibilityService, maxNodes: Int, maxDepth: Int): JSONObject {
        require(maxNodes in 1..PhoneObservations.MAX_NODES) { "invalid node limit" }
        require(maxDepth in 1..PhoneObservations.MAX_DEPTH) { "invalid depth limit" }
        val root = service.rootInActiveWindow ?: throw IllegalStateException("no accessible foreground window")
        try {
            val display = (service.getSystemService(android.content.Context.WINDOW_SERVICE) as WindowManager).defaultDisplay
            val size = android.graphics.Point()
            display.getRealSize(size)
            val rotation = display.rotation
            val windowId = root.windowId
            val packageName = root.packageName?.toString().orEmpty()
            val nodes = JSONArray()
            var truncated = false
            fun visit(node: AccessibilityNodeInfo, path: List<Int>, depth: Int) {
                if (nodes.length() >= maxNodes) { truncated = true; return }
                val bounds = Rect()
                node.getBoundsInScreen(bounds)
                if (!node.isVisibleToUser || bounds.isEmpty) return
                val text = if (node.isPassword) "" else node.text?.toString().orEmpty().take(PhoneObservations.MAX_TEXT)
                val description = if (node.isPassword) "" else node.contentDescription?.toString().orEmpty().take(PhoneObservations.MAX_TEXT)
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
                if (depth >= maxDepth) { if (node.childCount > 0) truncated = true; return }
                for (index in 0 until node.childCount) {
                    if (nodes.length() >= maxNodes) { truncated = true; break }
                    val child = node.getChild(index) ?: continue
                    try { visit(child, path + index, depth + 1) } finally { child.recycle() }
                }
            }
            visit(root, emptyList(), 0)
            val current = service.rootInActiveWindow ?: throw IllegalStateException("window changed during observation")
            try {
                check(current.windowId == windowId && current.packageName?.toString().orEmpty() == packageName) { "window changed during observation" }
                check(display.rotation == rotation) { "display changed during observation" }
            } finally { current.recycle() }
            val result = JSONObject().put("package_name", packageName).put("window_id", windowId)
                .put("width", size.x).put("height", size.y).put("rotation", rotation)
                .put("nodes", nodes).put("truncated", truncated)
            while (result.toString().toByteArray(Charsets.UTF_8).size > 56 * 1024 && nodes.length() > 0) {
                nodes.remove(nodes.length() - 1)
                result.put("truncated", true)
            }
            return result
        } finally { root.recycle() }
    }
}
