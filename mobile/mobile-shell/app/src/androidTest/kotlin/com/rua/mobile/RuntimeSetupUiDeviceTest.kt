package com.rua.mobile

import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.Button
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** The harness supplies either an isolated unprepared guest or a verified ready guest. */
@RunWith(AndroidJUnit4::class)
class RuntimeSetupUiDeviceTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private fun web(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (index in 0 until view.childCount) web(view.getChildAt(index))?.let { return it }
        return null
    }
    private fun await(message: String, check: () -> Boolean) {
        val end = SystemClock.elapsedRealtime() + 30_000
        while (SystemClock.elapsedRealtime() < end) { if (check()) return; SystemClock.sleep(100) }
        error(message)
    }
    private fun evaluate(scenario: ActivityScenario<MainActivity>, script: String): String {
        var result = "null"
        val latch = CountDownLatch(1)
        scenario.onActivity {
            val browser = web(it.window.decorView)
            if (browser == null) latch.countDown() else browser.evaluateJavascript(script) { value -> result = value; latch.countDown() }
        }
        assertTrue(latch.await(3, TimeUnit.SECONDS))
        return result
    }
    private fun nativeText(text: String) = instrumentation.uiAutomation.rootInActiveWindow
        ?.findAccessibilityNodeInfosByText(text).orEmpty()

    @Test fun unpreparedEnvironmentOffersPermissionsWithoutOpeningTheConsole() {
        assertFalse(RuntimeToolchains.compatible(instrumentation.targetContext))
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            await("Full-screen preparation missing") {
                var visible = false
                scenario.onActivity {
                    assertNull(web(it.window.decorView))
                    visible = it.findViewById<View>(R.id.rua_runtime_setup)?.isShown == true
                }
                visible
            }
            scenario.onActivity { it.findViewById<Button>(R.id.rua_runtime_setup_permissions).performClick() }
            await("Permission guide missing") { nativeText("权限设置").isNotEmpty() && nativeText("无障碍服务").isNotEmpty() }
            scenario.recreate()
            await("Preparation must survive permission-guide Activity recreation") {
                var visible = false
                scenario.onActivity {
                    assertNull(web(it.window.decorView))
                    visible = it.findViewById<View>(R.id.rua_runtime_setup)?.isShown == true
                }
                visible
            }
        }
    }

    @Test fun readyConsoleHasNoStatusBarAndMoreOpensNativePanels() {
        assertTrue(RuntimeToolchains.compatible(instrumentation.targetContext))
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            await("Chat menu missing") { evaluate(scenario, "!!document.querySelector('button[aria-label=\"更多\"],button[aria-label=\"More\"]')") == "true" }
            scenario.onActivity {
                assertNull(it.findViewById<View>(R.id.rua_runtime_environment_status))
                assertNull(it.findViewById<View>(R.id.rua_runtime_setup))
                val params = web(it.window.decorView)!!.layoutParams
                if (params is ViewGroup.MarginLayoutParams) assertEquals(0, params.topMargin)
            }
            fun openMenuItem(zh: String, en: String) {
                evaluate(scenario, "document.querySelector('button[aria-label=\"更多\"],button[aria-label=\"More\"]').click(); true")
                await("Native menu item missing: $zh") {
                    evaluate(scenario, "Array.from(document.querySelectorAll('[role=\"menuitem\"]')).some(e => e.textContent.includes('$zh') || e.textContent.includes('$en'))") == "true"
                }
                evaluate(scenario, "Array.from(document.querySelectorAll('[role=\"menuitem\"]')).find(e => e.textContent.includes('$zh') || e.textContent.includes('$en')).click(); true")
            }
            openMenuItem("工具环境管理", "Runtime environment")
            await("Environment panel missing") { nativeText("工具环境管理").isNotEmpty() }
            val close = nativeText("关闭").first { it.className == "android.widget.Button" }
            assertTrue(close.performAction(android.view.accessibility.AccessibilityNodeInfo.ACTION_CLICK))
            openMenuItem("权限设置", "App permissions")
            await("Permission panel missing") { nativeText("无障碍服务").isNotEmpty() }
        }
    }
}
