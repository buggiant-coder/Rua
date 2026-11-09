package com.rua.mobile

import android.webkit.WebView
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.util.ReflectionHelpers

@RunWith(RobolectricTestRunner::class)
class MainActivityBackTest {
    @Test
    fun backDispatchesDismissWithoutTraversingWebHistory() {
        val activity = Robolectric.buildActivity(MainActivity::class.java).get()
        val webView = WebView(activity)
        ReflectionHelpers.setField(activity, "webView", webView)
        webView.loadUrl("http://localhost/chat")
        webView.loadUrl("http://localhost/settings")
        val currentUrl = webView.url

        activity.onBackPressed()

        assertEquals("window.dispatchEvent(new Event('rua:back'))", shadowOf(webView).lastEvaluatedJavascript)
        assertEquals(currentUrl, webView.url)
        assertFalse(activity.isFinishing)
    }

    @Test
    fun backWithoutConsoleDoesNotFinishTheActivity() {
        val activity = Robolectric.buildActivity(MainActivity::class.java).get()

        activity.onBackPressed()

        assertFalse(activity.isFinishing)
    }
}
