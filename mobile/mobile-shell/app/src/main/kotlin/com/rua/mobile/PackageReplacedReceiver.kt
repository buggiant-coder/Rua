package com.rua.mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

class PackageReplacedReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        val result = MobilePackageInstallCleanup.cleanupCurrentPackageInstallIfNeeded(context)
        if (result != null) {
            Log.i(MainActivity.TAG, "package replaced; cleaned packaged resident server count=${result.killed}")
        }
    }
}
