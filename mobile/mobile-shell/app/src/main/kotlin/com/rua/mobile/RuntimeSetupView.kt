package com.rua.mobile

import android.content.Context
import android.content.res.ColorStateList
import android.content.res.Configuration
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.FrameLayout
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.ScrollView
import android.widget.TextView

/** The first-run surface owns the entire content area until every required tool is usable. */
internal class RuntimeSetupView(
    context: Context,
    materials: String,
    onAction: () -> Unit,
    onImport: () -> Unit,
    onPermissions: () -> Unit,
) : ScrollView(context) {
    private val night = resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK == Configuration.UI_MODE_NIGHT_YES
    private val ink = Color.parseColor(if (night) "#E9ECEF" else "#0C0F12")
    private val muted = Color.parseColor(if (night) "#B8BCC2" else "#6C757D")
    private val accent = Color.parseColor(if (night) "#74C0FC" else "#228BE6")
    private fun dp(value: Int) = (value * resources.displayMetrics.density).toInt()
    private fun label(value: String, size: Float, color: Int = ink) = TextView(context).apply {
        text = value
        textSize = size
        setTextColor(color)
        setLineSpacing(dp(4).toFloat(), 1f)
    }
    private fun styleButton(button: Button, primary: Boolean = false, outlined: Boolean = false) {
        button.apply {
            isAllCaps = false
            minHeight = dp(48)
            elevation = 0f
            stateListAnimator = null
            backgroundTintList = null
            setTextColor(if (primary) Color.parseColor(if (night) "#1F1F1F" else "#FFFFFF") else muted)
            val shape = GradientDrawable().apply {
                cornerRadius = dp(12).toFloat()
                setColor(if (primary) accent else Color.TRANSPARENT)
                if (outlined) setStroke(dp(1), Color.parseColor(if (night) "#495057" else "#DEE2E6"))
            }
            background = RippleDrawable(ColorStateList.valueOf(Color.parseColor("#301479B8")), shape, null)
        }
    }
    private val title = label("正在准备 Rua", 30f).apply { setTypeface(typeface, Typeface.BOLD) }
    private val description = label("首次启动需要准备运行环境，可能需要几分钟。\n这段时间可以先设置权限，安装会在后台继续。", 15f, muted)
    private val stage = label("正在检查运行环境", 17f).apply {
        id = R.id.rua_runtime_environment_status
        setTypeface(typeface, Typeface.BOLD)
        accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
    }
    private val source = label("自动检测可用线路，选择响应最快的来源", 13f, muted)
    private val progress = ProgressBar(context, null, android.R.attr.progressBarStyleHorizontal).apply {
        max = 100
        progressTintList = ColorStateList.valueOf(accent)
        indeterminateTintList = ColorStateList.valueOf(accent)
    }
    private val detail = label("", 12f, muted).apply { visibility = View.GONE }
    private val action = Button(context).apply {
        id = R.id.rua_runtime_setup_action
        styleButton(this, outlined = true)
        setOnClickListener { onAction() }
    }
    private val offline = Button(context).apply {
        text = "导入离线材料"
        styleButton(this, outlined = true)
        setOnClickListener { onImport() }
    }

    init {
        id = R.id.rua_runtime_setup
        isFillViewport = true
        setBackgroundColor(Color.parseColor(if (night) "#1F1F1F" else "#FFFFFF"))
        val frame = FrameLayout(context).apply { setPadding(dp(24), dp(32), dp(24), dp(32)) }
        addView(frame, LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.MATCH_PARENT))
        val content = LinearLayout(context).apply { orientation = LinearLayout.VERTICAL }
        frame.addView(content, FrameLayout.LayoutParams(
            minOf(resources.displayMetrics.widthPixels - dp(48), dp(440)),
            ViewGroup.LayoutParams.WRAP_CONTENT, Gravity.CENTER,
        ))
        fun add(view: View, top: Int = 0) {
            content.addView(view, LinearLayout.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT).apply { topMargin = dp(top) })
        }
        add(label("手Rua  /  RUA", 14f, accent).apply { letterSpacing = 0.12f; setTypeface(typeface, Typeface.BOLD) })
        add(title, 28)
        add(description, 12)
        val card = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(24), dp(24), dp(24), dp(24))
            background = GradientDrawable().apply {
                setColor(Color.parseColor(if (night) "#242424" else "#FFFFFF"))
                cornerRadius = dp(20).toFloat()
                setStroke(dp(1), Color.parseColor(if (night) "#495057" else "#DEE2E6"))
            }
        }
        card.addView(stage)
        card.addView(progress, LinearLayout.LayoutParams(LayoutParams.MATCH_PARENT, dp(6)).apply { topMargin = dp(22); bottomMargin = dp(16) })
        card.addView(source)
        card.addView(label(materials, 12f, muted), LinearLayout.LayoutParams(LayoutParams.MATCH_PARENT, LayoutParams.WRAP_CONTENT).apply { topMargin = dp(24) })
        add(card, 30)
        add(Button(context).apply {
            id = R.id.rua_runtime_setup_permissions
            text = "设置所需权限"
            styleButton(this, primary = true)
            setOnClickListener { onPermissions() }
        }, 24)
        add(label("通知、文件访问、无障碍与后台运行等", 12f, muted), 8)
        add(action, 20)
        add(offline, 10)
        add(Button(context).apply {
            text = "查看详情"
            styleButton(this)
            setOnClickListener {
                detail.visibility = if (detail.visibility == View.VISIBLE) View.GONE else View.VISIBLE
                text = if (detail.visibility == View.VISIBLE) "收起详情" else "查看详情"
            }
        }, 4)
        add(detail, 8)
    }

    fun render(phase: String, message: String, running: Boolean) {
        val paused = phase in listOf("cancelled", "interrupted")
        title.text = when {
            paused -> "准备已暂停"
            phase == "failed" -> "准备暂未完成"
            phase == "waiting_network" -> "等待网络连接"
            else -> "正在准备 Rua"
        }
        description.text = when {
            paused -> "已下载的内容会保留。点击重试继续准备。"
            phase == "failed" -> "请检查网络或查看详情，然后重试。已完成的下载无需重复。"
            phase == "waiting_network" -> "联网后会自动继续，也可以导入离线材料。"
            else -> "首次启动需要准备运行环境，可能需要几分钟。\n这段时间可以先设置权限，安装会在后台继续。"
        }
        val downloading = running && message.startsWith("正在下载 ")
        val selecting = running && message.startsWith("正在检测镜像")
        val component = when {
            message.contains("node-v") -> "Node 运行环境"
            message.contains("uv-") -> "Python 包管理工具"
            message.contains("rootfs") || message.endsWith(" · Debian") -> "基础运行环境"
            else -> "Python 和系统组件"
        }
        val percent = if (downloading) Regex("· (\\d+)% ·").find(message)?.groupValues?.get(1)?.toIntOrNull() else null
        stage.text = when {
            downloading -> "正在下载 $component" + (percent?.let { "  $it%" } ?: "")
            selecting -> "正在选择下载线路 · $component"
            else -> message.substringBefore(" · ").lineSequence().first().take(100)
        }
        source.text = when {
            downloading -> "下载来源：" + message.substringAfterLast(" · ")
            selecting -> "正在检测：" + message.substringAfter(" · ", "可用镜像")
            else -> "自动检测可用线路，下载内容将经过完整校验"
        }
        progress.visibility = if (running) View.VISIBLE else View.GONE
        progress.isIndeterminate = percent == null
        if (percent != null) progress.progress = percent
        action.text = if (running) "取消准备" else "重试"
        action.isEnabled = !(running && paused)
        offline.isEnabled = !running
        offline.alpha = if (running) 0.45f else 1f
        detail.text = message
    }
}
