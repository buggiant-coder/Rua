package com.rua.mobile

import android.app.UiAutomation
import android.net.Uri
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

@RunWith(AndroidJUnit4::class)
class NativeStorageDeviceTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val automation get() = instrumentation.getUiAutomation(UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES)
    private fun shell(command: String): String = android.os.ParcelFileDescriptor.AutoCloseInputStream(
        automation.executeShellCommand(command)).bufferedReader().use { it.readText().trim() }
    private fun await(message: String, check: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + 20_000
        while (SystemClock.elapsedRealtime() < end) {
            if (check()) return
            SystemClock.sleep(100)
        }
        error(message)
    }
    private fun web(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) web(view.getChildAt(index))?.let { return it }
        return null
    }

    @Test fun privateHomeIsNotPublishedAsDocumentsProvider() {
        val context = instrumentation.targetContext
        assertNull(context.packageManager.resolveContentProvider("${context.packageName}.documents", 0))
    }

    @Test fun downloadsExportRemainsReadableWithoutHomeProvider() {
        val context = instrumentation.targetContext
        val name = "rua-native-export-${System.nanoTime()}.txt"
        val contents = "RUA_DOWNLOAD_EXPORT_OK"
        val path = MobileFileBridge(context).saveBlob(name,
            android.util.Base64.encodeToString(contents.toByteArray(), android.util.Base64.NO_WRAP), "text/plain")
        assertFalse(path, path.startsWith("ERROR:"))
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            val resolver = context.contentResolver
            val collection = android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI
            val column = android.provider.MediaStore.MediaColumns.DISPLAY_NAME
            try {
                val document = resolver.query(collection, arrayOf(android.provider.MediaStore.MediaColumns._ID),
                    "$column = ?", arrayOf(name), null)!!.use { cursor ->
                    assertTrue("Export missing from MediaStore", cursor.moveToFirst())
                    android.content.ContentUris.withAppendedId(collection, cursor.getLong(0))
                }
                assertEquals(contents, resolver.openInputStream(document)!!.bufferedReader().use { it.readText() })
            } finally { resolver.delete(collection, "$column = ?", arrayOf(name)) }
        } else {
            val file = java.io.File(path)
            try {
                assertEquals(contents, file.readText())
            } finally { file.delete() }
        }
    }

    @Test fun realFilePickerReturnsCancellationToWebView() {
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            var browser: WebView? = null
            await("Authenticated WebView did not load") {
                scenario.onActivity { browser = web(it.window.decorView) }
                browser != null
            }
            val callback = AtomicBoolean(false)
            val result = AtomicReference<Array<Uri>?>()
            scenario.onActivity {
                assertTrue(browser!!.webChromeClient!!.onShowFileChooser(browser, {
                    result.set(it); callback.set(true)
                }, null))
            }
            await("System document picker was not launched") {
                shell("dumpsys activity activities").lineSequence().any { it.contains("ResumedActivity") && it.contains("documentsui") }
            }
            shell("input keyevent KEYCODE_BACK")
            await("File picker cancellation not returned to WebView") { callback.get() }
            assertNull(result.get())
        }
    }

    @Test fun realFilePickerReturnsReadableSelectedDocument() {
        val resolver = instrumentation.targetContext.contentResolver
        val name = "rua-native-picker-${System.nanoTime()}.txt"
        val values = android.content.ContentValues().apply {
            put(android.provider.MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(android.provider.MediaStore.MediaColumns.MIME_TYPE, "text/plain")
            put(android.provider.MediaStore.MediaColumns.RELATIVE_PATH, "Download")
            put(android.provider.MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val document = requireNotNull(resolver.insert(android.provider.MediaStore.Downloads.EXTERNAL_CONTENT_URI, values))
        try {
            android.os.ParcelFileDescriptor.AutoCloseOutputStream(resolver.openFileDescriptor(document, "w")!!).use {
                it.write("RUA_SELECTED_DOCUMENT".toByteArray()); it.flush(); it.fd.sync()
            }
            values.clear(); values.put(android.provider.MediaStore.MediaColumns.IS_PENDING, 0)
            resolver.update(document, values, null, null)
            ActivityScenario.launch(MainActivity::class.java).use { scenario ->
                var browser: WebView? = null
                await("WebView did not load") {
                    scenario.onActivity { browser = web(it.window.decorView) }
                    browser != null
                }
                val result = AtomicReference<Array<Uri>?>()
                scenario.onActivity {
                    assertTrue(browser!!.webChromeClient!!.onShowFileChooser(browser, { result.set(it) }, null))
                }
                await("Published document not visible in system picker") {
                    val root = automation.rootInActiveWindow ?: return@await false
                    val nodes = root.findAccessibilityNodeInfosByText(name)
                    var node = nodes.firstOrNull() ?: return@await false
                    while (!node.isClickable) node = node.parent ?: return@await false
                    node.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK)
                }
                await("Selected URI not returned to WebView") { result.get()?.size == 1 }
                assertEquals("RUA_SELECTED_DOCUMENT", resolver.openInputStream(result.get()!!.single())!!.bufferedReader().use { it.readText() })
            }
        } finally { resolver.delete(document, null, null) }
    }

    // Android kills the app when this permission changes; the host runner changes it
    // between invocations, then checks the restarted APK under the real app UID.
    @Test fun storageBridgeReflectsPermissionAfterProcessRecreation() {
        val expected = InstrumentationRegistry.getArguments().getString("storageGranted") == "true"
        val context = instrumentation.targetContext
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            var permitted = !expected
            scenario.onActivity { permitted = MobileStorageBridge(it).hasAllFilesAccess() }
            assertEquals(expected, permitted)
            assertTrue(java.io.File(context.filesDir, "runtime/debian/.rua-runtime-ready").isFile)
            if (expected) {
                val shared = java.io.File(android.os.Environment.getExternalStorageDirectory(), "Download/rua-native-storage-${System.nanoTime()}.txt")
                try {
                    LinuxRuntimeInstaller.atomicText(shared, "RUA_SHARED_STORAGE_OK")
                    assertEquals("RUA_SHARED_STORAGE_OK", shared.readText())
                } finally { shared.delete() }
            }
            await("Storage permission must not disable the resident") {
                val token = PhoneUseBridge.awaitAuthToken(100) ?: return@await false
                runCatching { NativeAgentDeviceTest().api(token, "GET", "/platform/foreground-operations").has("revision") }.getOrDefault(false)
            }
        }
    }
}
