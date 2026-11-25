package com.rua.mobile

import android.accessibilityservice.AccessibilityService
import android.graphics.Bitmap
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.view.Display
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.util.concurrent.CompletableFuture

internal object PhoneScreenshot {
    fun sameTarget(before: JSONObject, after: JSONObject): Boolean {
        for (field in listOf("package_name","window_id","rotation","width","height")) if(before.get(field) != after.get(field)) return false
        val prior=org.json.JSONArray(before.getJSONArray("nodes").toString())
        for(index in 0 until prior.length()) prior.getJSONObject(index).remove("token")
        return prior.toString() == after.getJSONArray("nodes").toString()
    }
    fun capture(service: MobileAccessibilityService, invocation: PhoneInvocation, requestId: String, maxNodes: Int, maxDepth: Int, generation: String): CompletableFuture<JSONObject> {
        val owner = invocation.owner
        val operation = PhoneDeviceControl.begin(invocation, requestId, generation, "screenshot")
        val handler = Handler(Looper.getMainLooper())
        handler.post {
            try {
                check(Build.VERSION.SDK_INT >= 30) { "Accessibility screenshots require Android 11 or newer" }
                check(!operation.cancelled.get()) { "Phone Use cancelled" }
                val before = service.observe(owner,maxNodes,maxDepth)
                service.controller.state.dispatch(operation) { service.takeScreenshot(Display.DEFAULT_DISPLAY, service.mainExecutor, object : AccessibilityService.TakeScreenshotCallback {
                    override fun onSuccess(result: AccessibilityService.ScreenshotResult) {
                        val buffer = result.hardwareBuffer
                        var hardware: Bitmap? = null
                        var bitmap: Bitmap? = null
                        var scaled: Bitmap? = null
                        try {
                            check(!operation.cancelled.get()) { "Phone Use cancelled" }
                            val after = PhoneTreeCapture.capture(service,maxNodes,maxDepth)
                            check(sameTarget(before,after)) { "UI changed during screenshot" }
                            hardware = Bitmap.wrapHardwareBuffer(buffer,result.colorSpace) ?: error("Screenshot buffer unavailable")
                            bitmap = hardware.copy(Bitmap.Config.ARGB_8888,false)
                            val factor = minOf(1.0, 1280.0 / maxOf(bitmap.width,bitmap.height))
                            scaled = Bitmap.createScaledBitmap(bitmap,(bitmap.width*factor).toInt().coerceAtLeast(1),(bitmap.height*factor).toInt().coerceAtLeast(1),true)
                            val bytes = ByteArrayOutputStream()
                            scaled.compress(Bitmap.CompressFormat.JPEG,65,bytes)
                            check(bytes.size() <= 768 * 1024) { "Screenshot exceeds size limit" }
                            // The base64 image and the tree share one media response frame.
                            PhoneTreeCapture.trimToPayloadLimit(before, before.getJSONArray("nodes"), PhoneObservations.MAX_SCREENSHOT_PAYLOAD_BYTES)
                            before.put("screenshot",JSONObject().put("mime_type","image/jpeg").put("width",scaled.width).put("height",scaled.height)
                                .put("data",android.util.Base64.encodeToString(bytes.toByteArray(),android.util.Base64.NO_WRAP)))
                            operation.result.complete(before)
                        } catch(e: Exception) {
                            invalidateIfCurrent(service, operation)
                            operation.result.completeExceptionally(e)
                            PhoneDeviceControl.fail(operation)
                        } finally {
                            if(scaled !== bitmap) scaled?.recycle()
                            bitmap?.recycle(); hardware?.recycle(); buffer.close()
                            service.controller.state.finish(operation)
                        }
                    }
                    override fun onFailure(errorCode: Int) {
                        invalidateIfCurrent(service, operation)
                        operation.result.completeExceptionally(IllegalStateException("Screenshot unavailable: $errorCode"))
                        PhoneDeviceControl.fail(operation)
                        service.controller.state.finish(operation)
                    }
                }) }
            } catch(e: Exception) { operation.result.completeExceptionally(e); PhoneDeviceControl.fail(operation); service.controller.state.finish(operation) }
        }
        handler.postDelayed({ if(!operation.result.isDone) { invalidateIfCurrent(service, operation); PhoneDeviceControl.fail(operation); service.controller.state.ifCurrent(operation) { service.controller.state.cancel(invocation,requestId) } }; service.controller.state.finish(operation) },3000)
        return operation.result
    }

    fun invalidateIfCurrent(service: MobileAccessibilityService, operation: PhoneOperation) {
        service.controller.state.ifCurrent(operation) { service.observations.invalidateOwner(operation.owner) }
    }
}
