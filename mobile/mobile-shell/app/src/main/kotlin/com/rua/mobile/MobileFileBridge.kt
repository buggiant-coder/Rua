package com.rua.mobile

import android.content.ContentValues
import android.content.Context
import android.media.MediaScannerConnection
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.util.Base64
import android.util.Log
import android.webkit.JavascriptInterface
import java.io.File























data class MobileSaveBlobRequest(
    val requestId: String,
    val name: String,
    val mime: String,
    val bytes: ByteArray,
)

class MobileFileBridge(
    private val ctx: Context,
    private val saveWithPicker: ((MobileSaveBlobRequest) -> String)? = null,
) {

    @JavascriptInterface
    fun saveBlobWithPicker(requestId: String, name: String, base64: String, mime: String): String {
        return try {
            val safeName = sanitize(name).ifEmpty { "download.bin" }
            val resolvedMime = mime.ifEmpty { "application/octet-stream" }
            val bytes = Base64.decode(base64, Base64.DEFAULT)
            val picker = saveWithPicker ?: return "ERROR:picker_unavailable"
            picker(MobileSaveBlobRequest(requestId, safeName, resolvedMime, bytes))
        } catch (e: Throwable) {
            Log.e(TAG, "saveBlobWithPicker('$name') failed", e)
            "ERROR:${e.javaClass.simpleName}:${e.message ?: ""}"
        }
    }

    @JavascriptInterface
    fun saveBlob(name: String, base64: String, mime: String): String {
        return try {
            val safeName = sanitize(name).ifEmpty { "download.bin" }
            val resolvedMime = mime.ifEmpty { "application/octet-stream" }
            val bytes = Base64.decode(base64, Base64.DEFAULT)
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                saveViaMediaStore(safeName, bytes, resolvedMime)
            } else {
                saveViaLegacy(safeName, bytes)
            }
        } catch (e: Throwable) {
            Log.e(TAG, "saveBlob('$name') failed", e)
            "ERROR:${e.javaClass.simpleName}:${e.message ?: ""}"
        }
    }

    private fun saveViaMediaStore(name: String, bytes: ByteArray, mime: String): String {
        val resolver = ctx.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, mime)
            put(
                MediaStore.Downloads.RELATIVE_PATH,
                Environment.DIRECTORY_DOWNLOADS + "/rua",
            )
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: return "ERROR:insert_returned_null"
        try {
            resolver.openOutputStream(uri).use { os ->
                if (os == null) {
                    resolver.delete(uri, null, null)
                    return "ERROR:openOutputStream_null"
                }
                os.write(bytes)
                os.flush()
            }
        } catch (e: Throwable) {
            resolver.delete(uri, null, null)
            throw e
        }
        Log.i(TAG, "saved via MediaStore: $name (${bytes.size} bytes) -> $uri")
        return "/sdcard/Download/rua/$name"
    }

    private fun saveViaLegacy(name: String, bytes: ByteArray): String {
        val baseDir = ctx.getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: return "ERROR:no_external_files_dir"
        val dir = File(baseDir, "rua")
        if (!dir.exists() && !dir.mkdirs()) return "ERROR:mkdirs_failed"
        val target = uniqueFile(dir, name)
        target.writeBytes(bytes)
        MediaScannerConnection.scanFile(ctx, arrayOf(target.absolutePath), null, null)
        Log.i(TAG, "saved via legacy: ${target.absolutePath} (${bytes.size} bytes)")
        return target.absolutePath
    }

    private fun uniqueFile(dir: File, name: String): File {
        val first = File(dir, name)
        if (!first.exists()) return first
        val dot = name.lastIndexOf('.')
        val stem = if (dot > 0) name.substring(0, dot) else name
        val ext = if (dot > 0) name.substring(dot) else ""
        var i = 1
        while (true) {
            val candidate = File(dir, "${stem}_$i$ext")
            if (!candidate.exists()) return candidate
            i++
            if (i > 9999) return candidate
        }
    }

    private fun sanitize(name: String): String {


        val cleaned = name.replace(Regex("[\\\\/\\u0000-\\u001f]"), "_").trim()
        return if (cleaned.length > 200) cleaned.takeLast(200) else cleaned
    }

    companion object {
        const val TAG = "MobileFileBridge"
        const val JS_NAME = "MobileNative"
    }
}
