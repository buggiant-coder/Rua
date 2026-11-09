package com.rua.mobile

import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import android.widget.Button
import android.widget.TextView
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment

@RunWith(RobolectricTestRunner::class)
class RuntimeSetupViewTest {
    private fun descendants(view: View): List<View> = listOf(view) + if (view is ViewGroup)
        (0 until view.childCount).flatMap { descendants(view.getChildAt(it)) } else emptyList()

    @Test fun downloadingShowsSourceAndCancellationWithoutAConsole() {
        var actions = 0
        val view = RuntimeSetupView(RuntimeEnvironment.getApplication(), "165 MB", { actions++ }, {}, {})
        view.render("downloading", "正在下载 uv-arm64.tar.gz · 42% · 中科大", true)
        val children = descendants(view)
        assertFalse(children.any { it is WebView })
        assertTrue(children.filterIsInstance<TextView>().any { it.text.contains("42%") })
        assertTrue(children.filterIsInstance<TextView>().any { it.text == "下载来源：中科大" })
        val action = view.findViewById<Button>(R.id.rua_runtime_setup_action)
        assertEquals("取消准备", action.text.toString())
        action.performClick()
        assertEquals(1, actions)
        assertFalse(children.filterIsInstance<Button>().first { it.text == "导入离线材料" }.isEnabled)
    }

    @Test fun permissionsRemainAvailableDuringInstallation() {
        var opened = 0
        val view = RuntimeSetupView(RuntimeEnvironment.getApplication(), "165 MB", {}, {}, { opened++ })
        view.render("downloading", "正在下载 uv-arm64.tar.gz · 42% · 中科大", true)
        val permissions = view.findViewById<Button>(R.id.rua_runtime_setup_permissions)
        assertTrue(permissions.isEnabled)
        permissions.performClick()
        assertEquals(1, opened)
        assertTrue(descendants(view).filterIsInstance<TextView>().any { it.text.contains("安装会在后台继续") })
    }

    @Test fun mirrorSelectionShowsTheCurrentMaterialWithoutOpeningDetails() {
        val view = RuntimeSetupView(RuntimeEnvironment.getApplication(), "165 MB", {}, {}, {})
        for ((file, component) in listOf(
            "Debian" to "基础运行环境",
            "debian-rootfs-arm64.tar.gz" to "基础运行环境",
            "node-v22-linux-arm64.tar.xz" to "Node",
            "uv-arm64.tar.gz" to "Python 包管理工具",
            "libpython3.13-arm64.deb" to "Python 和系统组件",
            "python3.13-arm64.deb" to "Python 和系统组件",
        )) {
            view.render("downloading", "正在检测镜像 · $file", true)
            assertTrue(view.findViewById<TextView>(R.id.rua_runtime_environment_status).text.contains(component))
            assertTrue(descendants(view).filterIsInstance<TextView>().any {
                it.visibility == View.VISIBLE && it.text.contains(file)
            })
        }
    }

    @Test fun pausedAndFailedPreparationKeepRetryAndOfflineImportAvailable() {
        for (phase in listOf("cancelled", "failed", "waiting_network")) {
            var imports = 0
            val view = RuntimeSetupView(RuntimeEnvironment.getApplication(), "165 MB", {}, { imports++ }, {})
            view.render(phase, "准备未完成", false)
            assertEquals("重试", view.findViewById<Button>(R.id.rua_runtime_setup_action).text.toString())
            val offline = descendants(view).filterIsInstance<Button>().first { it.text == "导入离线材料" }
            assertTrue(offline.isEnabled)
            offline.performClick()
            assertEquals(1, imports)
            assertFalse(descendants(view).any { it is WebView })
        }
    }
}
