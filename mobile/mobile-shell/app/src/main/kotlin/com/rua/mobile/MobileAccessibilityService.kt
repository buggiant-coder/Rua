package com.rua.mobile

import android.accessibilityservice.AccessibilityService
import android.content.Intent
import android.util.Log
import android.view.accessibility.AccessibilityEvent







class MobileAccessibilityService : AccessibilityService() {
    internal val observations = PhoneObservations()
    internal val controller by lazy { PhoneController(this) }

    internal fun observe(owner: String, maxNodes: Int, maxDepth: Int): org.json.JSONObject {
        check(instance === this) { "accessibility service is not connected" }
        observations.invalidateOwner(owner)
        return observations.remember(owner, PhoneTreeCapture.capture(this, maxNodes, maxDepth))
    }

    companion object {
        private const val TAG = "MobileA11y"

        @Volatile
        var instance: MobileAccessibilityService? = null
            private set
    }

    override fun onServiceConnected() {
        super.onServiceConnected()
        instance = this
        controller.attach()
        Log.i(TAG, "AccessibilityService connected")
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        if (event?.eventType == AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED ||
            event?.eventType == AccessibilityEvent.TYPE_WINDOWS_CHANGED) {
            @Suppress("DEPRECATION")
            val root=rootInActiveWindow
            if(root==null) observations.clear()
            else try { observations.invalidateOtherWindows(root.packageName?.toString().orEmpty(),root.windowId) }
            finally { root.recycle() }
        }
    }

    override fun onInterrupt() {
        controller.state.cancelCurrent()
        observations.clear()
        Log.i(TAG, "AccessibilityService interrupted")
    }

    override fun onUnbind(intent: Intent?): Boolean {
        controller.close()
        observations.clear()
        instance = null
        Log.i(TAG, "AccessibilityService unbound")
        return super.onUnbind(intent)
    }

    override fun onConfigurationChanged(newConfig: android.content.res.Configuration) {
        observations.clear()
        super.onConfigurationChanged(newConfig)
    }

    override fun onDestroy() {
        controller.close()
        observations.clear()
        if (instance === this) instance = null
        super.onDestroy()
    }
}
