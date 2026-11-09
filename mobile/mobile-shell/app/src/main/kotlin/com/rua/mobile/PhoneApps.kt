package com.rua.mobile

import android.content.Context
import android.content.Intent
import org.json.JSONObject

internal object PhoneApps {
    fun list(context: Context): List<JSONObject> {
        val intent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        return context.packageManager.queryIntentActivities(intent, 0)
            .distinctBy { it.activityInfo.packageName }
            .filter { it.activityInfo.packageName != context.packageName && it.activityInfo.packageName != "com.android.settings" && !it.activityInfo.packageName.contains("permissioncontroller") }
            .take(200)
            .map { JSONObject().put("package_name",it.activityInfo.packageName).put("label",it.loadLabel(context.packageManager).toString().take(120)) }
    }
}
