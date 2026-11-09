package com.rua.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.util.Log
import org.json.JSONObject
import java.io.File
import java.io.InputStream
import java.security.MessageDigest
import java.util.concurrent.CancellationException
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** One service-owned preparation operation; Activities only observe or request user actions. */
internal object RuntimePreparation {
    private val busy = AtomicBoolean(false)
    private val cancelled = AtomicBoolean(false)
    private val worker = Executors.newSingleThreadScheduledExecutor()
    @Volatile private var stopped = true
    @Volatile private var userCancelled = false
    @Volatile private var attempts = 0
    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    val running get() = busy.get()

    private fun stateFile(context: Context) = File(context.filesDir, "runtime/preparation.json")
    fun phaseName(context: Context): String = runCatching {
        JSONObject(stateFile(context).readText()).optString("phase", "pending")
    }.getOrDefault("pending")

    @Synchronized private fun phase(context: Context, phase: String, message: String) {
        if (userCancelled && phase != "cancelled" && phase != "ready") return
        LinuxRuntimeInstaller.atomicText(stateFile(context), JSONObject().put("phase", phase).put("message", message).toString())
        LinuxRuntimeInstaller.report(message)
    }

    @Synchronized fun start(context: Context) {
        if (!stopped) return
        stopped = false
        val app = context.applicationContext
        userCancelled = stateFile(app).takeIf { it.isFile }?.let {
            runCatching { JSONObject(it.readText()).optString("phase") == "cancelled" }.getOrDefault(false)
        } ?: false
        attempts = 0
        val manager = app.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) {
                if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) && attempts < 3) launch(app)
            }
        }
        networkCallback = callback
        manager.registerNetworkCallback(android.net.NetworkRequest.Builder().addCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET).build(), callback)
        launch(app)
    }

    @Synchronized fun stop(context: Context) {
        stopped = true
        cancelled.set(true)
        val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        networkCallback?.let { runCatching { manager.unregisterNetworkCallback(it) } }
        networkCallback = null
    }

    @Synchronized fun cancel(context: Context) {
        userCancelled = true
        cancelled.set(true)
        phase(context, "cancelled", "环境准备已取消 · 点击重试或导入")
    }

    @Synchronized fun retry(context: Context) {
        if (busy.get()) return
        userCancelled = false
        attempts = 0
        phase(context, "pending", "正在准备 Debian、Python 和 Node")
        launch(context.applicationContext)
    }

    @Synchronized fun update(context: Context) {
        if (stopped || !busy.compareAndSet(false, true)) return
        userCancelled = false
        cancelled.set(false)
        val app = context.applicationContext
        phase(app, "updating", "正在更新工具环境，原环境会保留到验证完成")
        worker.execute {
            try {
                val isCancelled = { cancelled.get() || stopped || userCancelled }
                RuntimeMirrorDownloader(isCancelled, LinuxRuntimeInstaller::report).use { downloader ->
                    RuntimeUpdate.install(app, isCancelled, download = { candidate ->
                        download(candidate, downloader, force = true)
                        downloadToolchains(candidate, downloader)
                    })
                }
                phase(app, "ready", "工具环境更新完成，已有依赖和工作已保留")
            } catch (error: Exception) {
                Log.w("RuaRuntime", "Environment update failed", error)
                phase(app, if (userCancelled) "cancelled" else "update_failed", "更新未完成：${error.message} · 点击重试")
            } finally { busy.set(false) }
        }
    }

    private fun connected(context: Context): Boolean {
        val manager = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
        return manager.activeNetwork?.let { manager.getNetworkCapabilities(it) }
            ?.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) == true
    }

    @Synchronized private fun launch(context: Context) {
        if (stopped || (userCancelled && !RuntimeToolchains.compatible(context)) || attempts >= 3 || !busy.compareAndSet(false, true)) return
        cancelled.set(false)
        worker.execute {
            var retry = false
            try {
                RuntimeUpdate.recover(context)
                LinuxRuntimeInstaller.recover(context)
                if (RuntimeToolchains.compatible(context)) {
                    phase(context, "ready", "Debian 工具环境已就绪")
                    return@execute
                }
                if (!connected(context)) {
                    phase(context, "waiting_network", "等待网络连接 · 可点击导入离线材料")
                    return@execute
                }
                attempts++
                phase(context, "downloading", LinuxRuntimeInstaller.describe(context) + "\n正在自动检测镜像")
                RuntimeMirrorDownloader({ cancelled.get() || stopped || userCancelled }, LinuxRuntimeInstaller::report).use { downloader ->
                    val isCancelled = { cancelled.get() || stopped || userCancelled }
                    val install: (Context) -> Unit = { target ->
                        download(target, downloader)
                        downloader.checkCancelled()
                        LinuxRuntimeInstaller.installCached(target, cancelled = isCancelled)
                        downloadToolchains(target, downloader)
                        RuntimeToolchains.installCached(target, cancelled = isCancelled)
                    }
                    install(context)
                }
                phase(context, "ready", "Debian 工具环境已就绪")
            } catch (error: Exception) {
                if (cancelled.get() || stopped || error is CancellationException) {
                    phase(context, if (userCancelled) "cancelled" else "interrupted", if (userCancelled) "环境准备已取消 · 点击重试或导入" else "环境准备已中断，下次打开继续")
                } else {
                    Log.w("RuaRuntime", "Environment preparation failed: ${error.message}")
                    phase(context, if (connected(context)) "failed" else "waiting_network", "环境准备未完成：${error.message} · 点击重试或导入")
                    retry = attempts < 3
                }
            } finally {
                busy.set(false)
                if (cancelled.get() && !userCancelled && !stopped) retry = true
                if (retry && !stopped && !userCancelled) worker.schedule({ launch(context) }, 15, TimeUnit.SECONDS)
            }
        }
    }

    fun importOffline(context: Context, input: InputStream) {
        synchronized(this) {
            check(busy.compareAndSet(false, true)) { "请先取消正在进行的环境准备" }
            cancelled.set(false)
            userCancelled = false
        }
        try {
            val isCancelled = { cancelled.get() || userCancelled }
            LinuxRuntimeInstaller.importOffline(context, input, isCancelled)
            phase(context, "ready", "Debian 工具环境已就绪")
        } catch (error: Exception) {
            if (userCancelled) phase(context, "cancelled", "环境准备已取消 · 点击重试或导入")
            else phase(context, "failed", "离线导入失败：${error.message} · 点击重试或导入")
            throw error
        } finally { busy.set(false) }
    }

    private fun download(context: Context, downloader: RuntimeMirrorDownloader, force: Boolean = false) {
        if (!force && LinuxRuntimeInstaller.ready(context)) return
        val manifest = LinuxRuntimeInstaller.manifest(context)
        val json = context.assets.open("runtime-manifest.json").bufferedReader().use { JSONObject(it.readText()) }
        val root = json.getJSONObject("rootfs")
        val cache = LinuxRuntimeInstaller.cache(context)
        if (!LinuxRuntimeInstaller.validCached(File(cache, manifest.rootfs.sha256), manifest.rootfs)) {
            val registries = listOf(
                Triple("Docker 官方", "https://registry-1.docker.io/v2/library/debian/", "https://auth.docker.io/token?service=registry.docker.io&scope=repository%3Alibrary%2Fdebian%3Apull"),
                Triple("DaoCloud", "https://m.daocloud.io/v2/docker.io/library/debian/", "https://m.daocloud.io/auth/token?service=m.daocloud.io&scope=repository%3Adocker.io%2Flibrary%2Fdebian%3Apull"),
            )
            val sources = downloader.prepareSources(registries.map { (name, base, tokenUrl) ->
                {
                    val token = JSONObject(String(downloader.metadata(RuntimeMirrorDownloader.Source(name, tokenUrl)), Charsets.UTF_8)).getString("token")
                    val headers = mapOf("Authorization" to "Bearer $token", "Accept" to "application/vnd.oci.image.manifest.v1+json")
                    val metadata = downloader.metadata(RuntimeMirrorDownloader.Source(name, base + "manifests/" + root.getString("ociManifest"), headers))
                    val digest = MessageDigest.getInstance("SHA-256").digest(metadata).joinToString("") { "%02x".format(it) }
                    check("sha256:$digest" == root.getString("ociManifest")) { "Debian 镜像身份不符" }
                    val layers = JSONObject(String(metadata, Charsets.UTF_8)).getJSONArray("layers")
                    check((0 until layers.length()).any { layers.getJSONObject(it).getString("digest") == "sha256:${manifest.rootfs.sha256}" })
                    RuntimeMirrorDownloader.Source(name, base + "blobs/sha256:" + manifest.rootfs.sha256, headers)
                }
            })
            downloader.download(manifest.rootfs, File(cache, manifest.rootfs.sha256), sources)
        }
        val packages = json.getJSONArray("packages")
        for (index in 0 until packages.length()) {
            val value = packages.getJSONObject(index)
            check(value.getString("architecture") in listOf("all", "arm64")) { "依赖架构不符" }
            val artifact = manifest.packages[index]
            val path = value.getString("path")
            check(!path.contains("..") && path.startsWith("pool/"))
            val archive = if (path.startsWith("pool/updates/")) "debian-security" else "debian"
            val sources = listOf(
                RuntimeMirrorDownloader.Source("Debian 官方", "https://deb.debian.org/$archive/$path"),
                RuntimeMirrorDownloader.Source("清华", "https://mirrors.tuna.tsinghua.edu.cn/$archive/$path"),
                RuntimeMirrorDownloader.Source("中科大", "https://mirrors.ustc.edu.cn/$archive/$path"),
                RuntimeMirrorDownloader.Source("华为云", "https://repo.huaweicloud.com/$archive/$path"),
                // Frozen archive fallback survives removal of an older version from rolling mirrors.
                RuntimeMirrorDownloader.Source("Debian Snapshot", "https://snapshot.debian.org/archive/$archive/20261104T000000Z/$path"),
            )
            downloader.download(artifact, File(cache, artifact.sha256), sources)
        }
    }
    private fun downloadToolchains(context: Context, downloader: RuntimeMirrorDownloader) {
        val manifest = LinuxRuntimeInstaller.manifest(context)
        val values = context.assets.open("runtime-manifest.json").bufferedReader().use { JSONObject(it.readText()) }.getJSONArray("toolchains")
        for (index in 0 until values.length()) {
            val value = values.getJSONObject(index)
            val sources = value.getJSONArray("sources")
            val artifact = manifest.toolchains[index]
            downloader.download(artifact, File(LinuxRuntimeInstaller.cache(context), artifact.sha256),
                (0 until sources.length()).map {
                    val source = sources.getJSONObject(it)
                    RuntimeMirrorDownloader.Source(source.getString("name"), source.getString("url"))
                })
        }
    }

}
