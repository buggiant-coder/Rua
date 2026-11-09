package com.rua.mobile

import android.content.pm.ActivityInfo
import android.os.SystemClock
import android.view.View
import android.view.ViewGroup
import android.view.inputmethod.InputMethodManager
import android.webkit.WebView
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class NativeTerminalWebViewDeviceTest {
    private fun web(view: View): WebView? {
        if (view is WebView) return view
        if (view is ViewGroup) for (i in 0 until view.childCount) web(view.getChildAt(i))?.let { return it }
        return null
    }
    private fun evaluate(scenario: ActivityScenario<MainActivity>, script: String): String {
        var result = "null"
        val signal = CountDownLatch(1)
        scenario.onActivity { activity ->
            val browser = web(activity.window.decorView)
            if (browser == null) signal.countDown() else browser.evaluateJavascript(script) { result = it; signal.countDown() }
        }
        assertTrue("WebView script did not finish", signal.await(3, TimeUnit.SECONDS))
        return result
    }
    private fun await(message: String, check: () -> Boolean) {
        val deadline = SystemClock.elapsedRealtime() + 30_000
        while (SystemClock.elapsedRealtime() < deadline) { if (check()) return; SystemClock.sleep(100) }
        error(message)
    }
    @Test fun normalTerminalPageRetainsShellAcrossRouteImeFocusAndRotation() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val fixture = NativeAgentDeviceTest()
        ActivityScenario.launch(MainActivity::class.java).use { scenario ->
            var token = ""
            fun api(method: String, path: String, body: JSONObject? = null) = fixture.api(token, method, "/terminal$path" + (if (method == "GET" && !path.contains('?')) "?execution_device=local" else ""), body?.toString())
            await("Terminal API did not become ready") {
                token = PhoneUseBridge.awaitAuthToken(100) ?: token
                runCatching { api("GET", "/profiles"); true }.getOrDefault(false)
            }
            val existing = api("GET", "/sessions").getJSONArray("sessions")
            val before = (0 until existing.length()).map { existing.getJSONObject(it).getString("session_id") }.toSet()
            await("Console did not load") { evaluate(scenario, "document.readyState === 'complete' && !!localStorage.getItem('rua_auth_token')") == "true" }
            evaluate(scenario, "localStorage.setItem('rua-terminal-diagnostics','1'); history.pushState({},'', '/terminal'); window.dispatchEvent(new PopStateEvent('popstate')); true")
            await("Terminal page did not mount") { evaluate(scenario, "!!document.querySelector('button[aria-label=\"New terminal\"]:not(:disabled),button[aria-label=\"新建终端\"]:not(:disabled)')") == "true" }
            evaluate(scenario, "document.querySelector('button[aria-label=\"New terminal\"],button[aria-label=\"新建终端\"]').click(); true")
            var id = ""
            await("Normal terminal button did not create a session") {
                val sessions = api("GET", "/sessions").getJSONArray("sessions")
                id = (0 until sessions.length()).map { sessions.getJSONObject(it).getString("session_id") }.firstOrNull { it !in before }.orEmpty()
                id.isNotEmpty()
            }
            var viewer = ""
            try {
                await("Web terminal did not become live") { evaluate(scenario, "(window.__ruaTerminalDiagnostics || []).some(e => e.session_id === ${JSONObject.quote(id)} && e.event === 'attach_complete') && !!document.querySelector('.xterm-helper-textarea')") == "true" }
                val sessions = api("GET", "/sessions").getJSONArray("sessions")
                val session = (0 until sessions.length()).map { sessions.getJSONObject(it) }.single { it.getString("session_id") == id }
                assertEquals("proot", session.getString("profile_id"))
                viewer = session.getJSONObject("input_lease_holder").getString("browser_id")
                val marker = File(session.getString("cwd"), "native-web-terminal-pid").also { it.delete() }
                scenario.onActivity { activity -> web(activity.window.decorView)!!.requestFocus() }
                evaluate(scenario, "document.querySelector('.xterm-helper-textarea').focus(); true")
                evaluate(scenario, "(() => {const original=window.fetch; window.nativeTerminalInputProbe={sent:[],active:0,max:0}; window.fetch=async(...args)=>{const p=window.nativeTerminalInputProbe; const input=String(args[0]).includes('/terminal/sessions/')&&String(args[0]).endsWith('/input'); if(input){p.sent.push(JSON.parse(args[1].body).data);p.active++;p.max=Math.max(p.max,p.active);}try{return await original(...args);}finally{if(input)p.active--;}};return true;})()")
                InstrumentationRegistry.getInstrumentation().waitForIdleSync()
                InstrumentationRegistry.getInstrumentation().sendStringSync("echo $$ > native-web-terminal-pid")
                InstrumentationRegistry.getInstrumentation().sendKeyDownUpSync(android.view.KeyEvent.KEYCODE_ENTER)
                try {
                    await("WebView terminal input did not reach Debian") { marker.isFile && marker.readText().trim().toIntOrNull() != null }
                } catch (error: Exception) {
                    val screen = api("POST", "/sessions/$id/attach", JSONObject().put("browser_viewer_id", viewer).put("scrollback_lines", 2000)).getJSONObject("snapshot").getString("data")
                    val diagnostics = evaluate(scenario, "JSON.stringify({input:window.nativeTerminalInputProbe,focus:document.activeElement?.className,events:(window.__ruaTerminalDiagnostics||[]).slice(-12)})")
                    throw AssertionError("${error.message}; screen=${screen.takeLast(2000)}; diagnostics=$diagnostics", error)
                }
                assertEquals("Input requests overlapped", "1", evaluate(scenario, "window.nativeTerminalInputProbe.max"))
                val pid = marker.readText().trim()
                val attaches = evaluate(scenario, "(window.__ruaTerminalDiagnostics || []).filter(e=>e.event==='attach_start').length")
                evaluate(scenario, "history.pushState({},'', '/'); window.dispatchEvent(new PopStateEvent('popstate')); true")
                SystemClock.sleep(300)
                evaluate(scenario, "history.pushState({},'', '/terminal'); window.dispatchEvent(new PopStateEvent('popstate')); true")
                evaluate(scenario, "(() => {const t=document.querySelector('.xterm-helper-textarea'); t.focus(); t.dispatchEvent(new CompositionEvent('compositionstart',{bubbles:true,data:''})); t.dispatchEvent(new CompositionEvent('compositionend',{bubbles:true,data:''})); return true;})()")
                scenario.onActivity { activity ->
                    web(activity.window.decorView)?.let { browser ->
                        browser.requestFocus()
                        activity.getSystemService(InputMethodManager::class.java).showSoftInput(browser, InputMethodManager.SHOW_IMPLICIT)
                    }
                    activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_LANDSCAPE
                }
                SystemClock.sleep(700)
                scenario.onActivity { activity ->
                    activity.getSystemService(InputMethodManager::class.java).hideSoftInputFromWindow(activity.window.decorView.windowToken, 0)
                    activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_PORTRAIT
                }
                SystemClock.sleep(700)
                assertEquals("Route/IME/rotation recreated the viewer", attaches, evaluate(scenario, "(window.__ruaTerminalDiagnostics || []).filter(e=>e.event==='attach_start').length"))
                assertEquals("Route/IME/rotation triggered recovery replay", "0", evaluate(scenario, "(window.__ruaTerminalDiagnostics || []).filter(e=>e.event==='recovery_start').length"))
                marker.delete()
                api("POST", "/sessions/$id/input", JSONObject().put("browser_viewer_id", viewer).put("data", "printf '%s' $$ > native-web-terminal-pid\n"))
                await("Terminal stopped after visibility changes") { marker.isFile }
                assertEquals(pid, marker.readText().trim())
            } finally {
                if (viewer.isNotEmpty()) api("DELETE", "/sessions/$id?browser_viewer_id=$viewer")
            }
        }
    }
}
