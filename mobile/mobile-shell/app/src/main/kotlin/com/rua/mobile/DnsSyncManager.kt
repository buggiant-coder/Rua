package com.rua.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import java.io.File
import java.net.Inet4Address











class DnsSyncManager(context: Context) {

    companion object {
        const val TAG = "DnsSyncManager"

        val DEFAULT_DNS = listOf("223.5.5.5", "119.29.29.29", "114.114.114.114")

        const val DEBOUNCE_MS = 200L
    }

    private val appContext = context.applicationContext
    private val resolvConf = File(context.filesDir, "runtime/debian/etc/resolv.conf")
    private val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE)
        as ConnectivityManager

    private var registered = false


    private val workThread = HandlerThread("DnsSyncWorker").also { it.start() }
    private val workHandler = Handler(workThread.looper)
    private val debouncedUpdate = Runnable { updateResolvConf() }

    private fun scheduleUpdate() {
        workHandler.removeCallbacks(debouncedUpdate)
        workHandler.postDelayed(debouncedUpdate, DEBOUNCE_MS)
    }

    private val callback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            Log.i(TAG, "network available: $network")
            scheduleUpdate()
        }
        override fun onLost(network: Network) {
            Log.i(TAG, "network lost: $network")
            scheduleUpdate()
        }
        override fun onLinkPropertiesChanged(network: Network, linkProps: LinkProperties) {
            scheduleUpdate()
        }
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {

            scheduleUpdate()
        }
    }


    fun start() {

        updateResolvConf()

        if (registered) return
        try {
            val req = NetworkRequest.Builder()
                .addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)

                .build()
            cm.registerNetworkCallback(req, callback)
            registered = true
            Log.i(TAG, "NetworkCallback registered")
        } catch (ex: Throwable) {
            Log.e(TAG, "registerNetworkCallback failed", ex)
        }
    }

    fun stop() {
        if (registered) try { cm.unregisterNetworkCallback(callback) } catch (_: Throwable) { }
        registered = false
        workHandler.removeCallbacks(debouncedUpdate)
        workThread.quitSafely()
    }


    fun sync() = updateResolvConf()

    private fun updateResolvConf() {
        if (!LinuxRuntimeInstaller.ready(appContext)) return
        try {
            val dns = chooseDnsServers()
            val content = buildString {
                dns.forEach { appendLine("nameserver $it") }

                appendLine("options single-request-reopen timeout:1 attempts:1 no-aaaa")
            }
            LinuxRuntimeInstaller.atomicText(resolvConf, content)
            Log.i(TAG, "resolv.conf updated: $dns")
        } catch (ex: Throwable) {
            Log.e(TAG, "updateResolvConf failed", ex)
        }
    }

    private fun chooseDnsServers(): List<String> {





        val net = cm.activeNetwork ?: return DEFAULT_DNS
        val link = cm.getLinkProperties(net) ?: return DEFAULT_DNS


        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && link.isPrivateDnsActive) {
            Log.i(TAG, "private DNS active (${link.privateDnsServerName}), fallback to default")
            return DEFAULT_DNS
        }


        val dns = link.dnsServers
            .filterIsInstance<Inet4Address>()
            .mapNotNull { it.hostAddress }
            .filter { it.isNotBlank() }

        return if (dns.isNotEmpty()) dns else DEFAULT_DNS
    }

    @Suppress("DEPRECATION")
    private fun allNetworksOrEmpty(): List<Network> = try {
        cm.allNetworks.toList()
    } catch (_: Throwable) { emptyList() }


}
