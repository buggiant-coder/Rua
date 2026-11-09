package com.rua.mobile

import android.app.Activity
import android.webkit.JavascriptInterface

class MobileStorageBridge(private val activity: Activity) {
    @JavascriptInterface
    fun hasAllFilesAccess(): Boolean = PermissionGuide.hasAllFilesAccess(activity)

    @JavascriptInterface
    fun requestAllFilesAccess() {
        activity.runOnUiThread { PermissionGuide.jumpToAllFilesAccess(activity) }
    }

    companion object {
        const val JS_NAME = "MobileStorage"
    }
}
