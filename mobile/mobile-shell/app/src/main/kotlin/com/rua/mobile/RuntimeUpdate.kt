package com.rua.mobile

import android.content.Context
import android.content.ContextWrapper
import android.system.Os
import org.json.JSONObject
import java.io.Closeable
import java.io.File
import java.util.UUID
import java.util.concurrent.CancellationException
import java.util.concurrent.TimeUnit

/** The stable admission lock covers snapshot, validation, commit, and recovery. */
internal object RuntimeUpdate {
    private fun runtime(context: Context) = File(context.filesDir, "runtime")

    private class Lease(context: Context) : Closeable {
        private val child = ProcessBuilder(File(context.applicationInfo.nativeLibraryDir, "librua-runtime-gate.so").path,
            "--exclusive", File(runtime(context), "use.lock").path, File(runtime(context), "update.pending").path)
            .redirectErrorStream(true).start()
        init {
            val result = child.inputStream.bufferedReader().readLine()
            if (result != "LOCKED") {
                child.destroy()
                child.waitFor(5, TimeUnit.SECONDS)
                error("工具环境正在使用中，请先结束工具、MCP、ACP、插件和终端，再重试更新")
            }
        }
        override fun close() {
            child.outputStream.close()
            check(child.waitFor(5, TimeUnit.SECONDS) && child.exitValue() == 0) { "更新使用锁未正常释放" }
        }
    }

    @Synchronized fun recover(context: Context) {
        val journalFile = File(runtime(context), "update.json")
        if (!File(runtime(context), "update.pending").exists()) {
            if (!journalFile.isFile) return
            val journal = JSONObject(journalFile.readText())
            if (journal.optString("phase") in listOf("ready", "rolled_back") &&
                !File(runtime(context), "update-${journal.getString("operation_id")}").exists()) return
        }
        Lease(context).use { recoverLocked(context) }
    }

    private fun recoverLocked(context: Context) {
        val runtime = runtime(context)
        val journalFile = File(runtime, "update.json")
        val pending = File(runtime, "update.pending")
        if (!journalFile.isFile) { check(!pending.exists()) { "更新记录缺失，保留环境等待恢复" }; return }
        val journal = JSONObject(journalFile.readText())
        val id = journal.getString("operation_id")
        require(UUID.fromString(id).toString() == id)
        val staging = File(runtime, "update-$id")
        val root = File(runtime, "debian")
        val home = File(context.filesDir, "guest-home")
        fun owned(file: File) = File(file, ".rua-update-id").takeIf { it.isFile }?.readText() == id
        val committed = owned(root) && owned(home)
        if (!committed) {
            // Each rename can be interrupted. Restore only this transaction's backups.
            for ((active, backup) in listOf(root to File(staging, "previous-root"), home to File(staging, "previous-home"))) {
                if (backup.exists()) {
                    if (active.exists()) {
                        check(owned(active)) { "更新路径存在未知环境，保留所有材料" }
                        LinuxRuntimeInstaller.removeStaging(active)
                    }
                    check(backup.renameTo(active))
                    NativePayload.syncDirectory(active.parentFile!!)
                } else check(active.isDirectory && !owned(active)) { "更新恢复缺少原环境，保留所有材料" }
            }
        }
        LinuxRuntimeInstaller.atomicText(journalFile, journal.put("phase", if (committed) "ready" else "rolled_back").toString())
        if (pending.exists()) { check(pending.delete()); NativePayload.syncDirectory(runtime) }
        if (staging.exists()) { LinuxRuntimeInstaller.removeStaging(staging); NativePayload.syncDirectory(runtime) }
    }

    @Synchronized fun install(
        context: Context,
        cancelled: () -> Boolean = { false },
        onPhase: (String) -> Unit = {},
        download: (Context) -> Unit,
    ) {
        Lease(context).use {
            recoverLocked(context)
            check(RuntimeToolchains.compatible(context)) { "现有环境不在兼容范围，原环境已保留" }
            val runtime = runtime(context)
            val id = UUID.randomUUID().toString()
            val staging = File(runtime, "update-$id")
            val journalFile = File(runtime, "update.json")
            val journal = JSONObject().put("operation_id", id)
            fun phase(name: String) {
                if (cancelled()) throw CancellationException("已取消环境更新")
                LinuxRuntimeInstaller.atomicText(journalFile, journal.put("phase", name).toString())
                onPhase(name)
            }
            phase("preparing")
            LinuxRuntimeInstaller.atomicText(File(runtime, "update.pending"), id)
            try {
                check(staging.mkdir())
                val candidate = object : ContextWrapper(context) { override fun getFilesDir(): File = staging }
                for (dir in listOf("runtime", "tmp")) check(File(staging, dir).mkdir())
                Os.symlink(LinuxRuntimeInstaller.cache(context).path, LinuxRuntimeInstaller.cache(candidate).path)
                check(staging.usableSpace >= LinuxRuntimeInstaller.manifest(context).requiredFreeBytes) { "更新空间不足，原环境已保留" }
                LinuxRuntimeInstaller.report("正在保留已有依赖并准备更新")
                val appFiles = context.applicationContext.filesDir
                val hidden = setOf("dev", "proc", "sys", "tmp", "sdcard", "storage", "root", "run/rua-cache",
                    MainActivity.HOME_DIR.removePrefix("/"), File(appFiles, "home").path.removePrefix("/"),
                    File(appFiles, "app-resources").path.removePrefix("/"))
                RuntimeTree.copy(File(runtime, "debian"), File(staging, "runtime/debian"), cancelled, File(runtime, "debian"), hidden)
                RuntimeTree.copy(File(context.filesDir, "guest-home"), File(staging, "guest-home"), cancelled, File(context.filesDir, "guest-home"), emptySet())
                phase("copied")
                download(candidate)
                val manifest = LinuxRuntimeInstaller.manifest(candidate)
                manifest.artifacts.forEach { artifact ->
                    if (cancelled()) throw CancellationException("已取消环境更新")
                    check(LinuxRuntimeInstaller.validCached(File(LinuxRuntimeInstaller.cache(candidate), artifact.sha256), artifact)) { "更新材料校验失败：${artifact.file}" }
                }
                val inputs = File(staging, "update-inputs").apply { mkdirs() }
                for (name in listOf("update-runtime.py", "runtime-manifest.json"))
                    LinuxRuntimeInstaller.atomicText(File(inputs, name), context.assets.open(name).bufferedReader().use { it.readText() })
                val root = File(staging, "runtime/debian")
                phase("updating")
                LinuxRuntimeInstaller.guest(candidate, root, inputs, cancelled,
                    listOf("/usr/bin/python3", "/run/rua-install/update-runtime.py"),
                    extraBinds = listOf(LinuxRuntimeInstaller.cache(context).path to "/run/rua-cache"), timeoutMinutes = 20)
                RuntimeToolchains.installCached(candidate, replaceManaged = true, cancelled = cancelled)
                check(RuntimeToolchains.ready(candidate) && RuntimeToolchains.compatible(candidate)) { "更新后的工具版本不兼容" }
                LinuxRuntimeInstaller.atomicText(File(root, ".rua-runtime-ready"), manifest.id + "\n")
                LinuxRuntimeInstaller.atomicText(File(root, ".rua-update-id"), id)
                LinuxRuntimeInstaller.atomicText(File(staging, "guest-home/.rua-update-id"), id)
                val cacheMount = File(root, "run/rua-cache")
                if (cacheMount.exists() && Os.lstat(cacheMount.path).st_mode and 0x1ff == 0) {
                    check(cacheMount.delete()) { "更新材料挂载点含未知数据" }
                    NativePayload.syncDirectory(cacheMount.parentFile!!)
                }
                RuntimeTree.sync(root)
                RuntimeTree.sync(File(staging, "guest-home"))
                phase("verified")
                fun move(from: File, to: File) {
                    check(from.renameTo(to)) { "无法激活环境更新，原环境保留" }
                    NativePayload.syncDirectory(from.parentFile!!)
                    NativePayload.syncDirectory(to.parentFile!!)
                }
                move(File(runtime, "debian"), File(staging, "previous-root"))
                phase("root_saved")
                move(File(context.filesDir, "guest-home"), File(staging, "previous-home"))
                phase("home_saved")
                move(File(staging, "guest-home"), File(context.filesDir, "guest-home"))
                phase("home_committed")
                move(root, File(runtime, "debian"))
                onPhase("activated")
                recoverLocked(context)
                LinuxRuntimeInstaller.report("工具环境更新完成，已有依赖和工作已保留")
            } catch (error: Exception) {
                val log = File(staging, "runtime/install.log")
                if (log.isFile) runCatching { LinuxRuntimeInstaller.atomicText(File(runtime, "update.log"), log.readText().takeLast(256 * 1024)) }
                recoverLocked(context)
                throw error
            }
        }
    }
}
