package com.rua.mobile

import android.webkit.JavascriptInterface

/** Only opens native panels; web content cannot directly install or grant permissions. */
class MobileRuntimeBridge(private val environment: () -> Unit, private val permissions: () -> Unit) {
    @JavascriptInterface fun openEnvironment() = environment()
    @JavascriptInterface fun openPermissions() = permissions()

    companion object { const val JS_NAME = "MobileRuntime" }
}
