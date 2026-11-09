package com.rua.mobile

import android.accessibilityservice.AccessibilityServiceInfo
import android.annotation.SuppressLint
import android.app.Activity
import android.app.AppOpsManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.os.PowerManager
import android.os.Process
import android.provider.Settings
import android.util.Log
import android.view.accessibility.AccessibilityManager







object PermissionGuide {
    private const val TAG = "PermissionGuide"


    data class Item(
        val name: String,
        val description: String,
        val isGranted: (Context) -> Boolean,
        val jumpTo: (Activity) -> Unit,
    )


    fun standardItems(): List<Item> = listOf(
        Item(
            name = "忽略电池优化",
            description = "防止后台被系统省电策略冻结",
            isGranted = ::isBatteryWhitelisted,
            jumpTo = ::jumpToBatteryWhitelist,
        ),
        Item(
            name = "所有文件访问",
            description = "让 AI 读写手机存储里的文件",
            isGranted = ::hasAllFilesAccess,
            jumpTo = ::jumpToAllFilesAccess,
        ),
        Item(
            name = "无障碍服务",
            description = "让 AI 看屏幕、模拟点击和输入",
            isGranted = ::hasAccessibilityService,
            jumpTo = ::jumpToAccessibilitySettings,
        ),
        Item(
            name = "悬浮窗",
            description = "AI 操作时显示状态浮窗",
            isGranted = ::hasOverlayPermission,
            jumpTo = ::jumpToOverlayPermission,
        ),
        Item(
            name = "应用使用情况",
            description = "知道当前是哪个 app, 适配 AI 行为",
            isGranted = ::hasUsageStatsPermission,
            jumpTo = ::jumpToUsageStatsSettings,
        ),
        Item(
            name = "通知",
            description = "AI 完成任务时给你弹通知",
            isGranted = ::hasNotificationPermission,
            jumpTo = ::jumpToNotificationSettings,
        ),
    )



    fun isBatteryWhitelisted(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return true
        val pm = context.getSystemService(Context.POWER_SERVICE) as PowerManager
        return pm.isIgnoringBatteryOptimizations(context.packageName)
    }

    fun hasAllFilesAccess(context: Context): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            Environment.isExternalStorageManager()
        } else {
            context.checkSelfPermission("android.permission.WRITE_EXTERNAL_STORAGE") ==
                PackageManager.PERMISSION_GRANTED
        }
    }





    fun hasAccessibilityService(context: Context): Boolean {
        return try {
            val am = context.getSystemService(Context.ACCESSIBILITY_SERVICE) as AccessibilityManager
            am.getEnabledAccessibilityServiceList(AccessibilityServiceInfo.FEEDBACK_ALL_MASK)
                .any { it.resolveInfo?.serviceInfo?.packageName == context.packageName }
        } catch (_: Throwable) { false }
    }

    fun hasOverlayPermission(context: Context): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return true
        return Settings.canDrawOverlays(context)
    }

    fun hasUsageStatsPermission(context: Context): Boolean {
        return try {
            val ops = context.getSystemService(Context.APP_OPS_SERVICE) as AppOpsManager
            val mode = ops.checkOpNoThrow(
                AppOpsManager.OPSTR_GET_USAGE_STATS, Process.myUid(), context.packageName
            )
            mode == AppOpsManager.MODE_ALLOWED
        } catch (_: Throwable) { false }
    }

    fun hasNotificationPermission(context: Context): Boolean {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.checkSelfPermission("android.permission.POST_NOTIFICATIONS") ==
                PackageManager.PERMISSION_GRANTED
        } else true
    }



    @SuppressLint("BatteryLife")
    fun jumpToBatteryWhitelist(activity: Activity) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
        try {
            activity.startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                data = Uri.parse("package:${activity.packageName}")
            })
        } catch (_: Throwable) {
            try { activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)) }
            catch (_: Throwable) { openAppDetails(activity) }
        }
    }

    fun jumpToAllFilesAccess(activity: Activity) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) {
            openAppDetails(activity)
            return
        }
        try {
            activity.startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
                data = Uri.parse("package:${activity.packageName}")
            })
        } catch (_: Throwable) {
            try { activity.startActivity(Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)) }
            catch (_: Throwable) { openAppDetails(activity) }
        }
    }

    fun jumpToAccessibilitySettings(activity: Activity) {
        try { activity.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) }
        catch (ex: Throwable) { Log.e(TAG, "open a11y settings failed", ex); openAppDetails(activity) }
    }

    fun jumpToOverlayPermission(activity: Activity) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
        try {
            activity.startActivity(Intent(Settings.ACTION_MANAGE_OVERLAY_PERMISSION).apply {
                data = Uri.parse("package:${activity.packageName}")
            })
        } catch (_: Throwable) { openAppDetails(activity) }
    }

    fun jumpToUsageStatsSettings(activity: Activity) {
        try { activity.startActivity(Intent(Settings.ACTION_USAGE_ACCESS_SETTINGS)) }
        catch (ex: Throwable) { Log.e(TAG, "open usage stats settings failed", ex); openAppDetails(activity) }
    }

    fun jumpToNotificationSettings(activity: Activity) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {

            try {
                activity.requestPermissions(
                    arrayOf("android.permission.POST_NOTIFICATIONS"),
                    REQ_NOTIFICATIONS
                )
            } catch (_: Throwable) { openAppDetails(activity) }
        } else {
            try {
                activity.startActivity(Intent("android.settings.APP_NOTIFICATION_SETTINGS").apply {
                    putExtra("android.provider.extra.APP_PACKAGE", activity.packageName)
                })
            } catch (_: Throwable) { openAppDetails(activity) }
        }
    }


    fun openAppDetails(activity: Activity) {
        try {
            activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                data = Uri.parse("package:${activity.packageName}")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            })
        } catch (ex: Throwable) {
            Log.e(TAG, "open app details failed", ex)
        }
    }

    const val REQ_NOTIFICATIONS = 1001



    fun manualSteps(): String {
        val brand = Build.MANUFACTURER.lowercase()
        val launchManager = when {
            "huawei" in brand || "honor" in brand ->
                "1) 设置 → 应用和服务 → 应用启动管理\n" +
                "2) 找到\"手Rua\" → 关闭\"自动管理\"\n" +
                "3) 在弹窗里全部勾选: 自启动 / 关联启动 / 后台活动"
            "xiaomi" in brand || "redmi" in brand ->
                "1) 安全中心 → 应用管理 → 权限 → 自启动管理 → 手Rua → 开启\n" +
                "2) 设置 → 应用 → 手Rua → 省电策略 → 无限制"
            "oppo" in brand || "realme" in brand || "oneplus" in brand ->
                "1) 设置 → 电池 → 后台耗电管理 → 手Rua → 允许\n" +
                "2) 电池 → 应用速冻 → 手Rua → 关闭"
            "vivo" in brand || "iqoo" in brand ->
                "1) i 管家 → 应用管理 → 权限管理 → 自启动管理 → 手Rua → 开启\n" +
                "2) i 管家 → 后台高耗电 → 手Rua → 允许"
            "samsung" in brand ->
                "设置 → 设备维护 → 电池 → 从不休眠应用 → 添加手Rua"
            else ->
                "设置 → 电池 → 找到\"手Rua\" → 设为不限制后台运行"
        }
        val recentLock = "在最近应用 (多任务) 界面长按手Rua 卡片 → 选\"加锁\"或下拉锁定, 防止滑动清掉"
        return "【后台保活 - 厂商启动管理】\n$launchManager\n\n【最近应用加锁】\n$recentLock"
    }
}
