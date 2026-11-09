package com.rua.mobile

import android.content.Context
import android.system.Os
import org.json.JSONObject
import java.io.File
import java.util.UUID
import java.util.concurrent.CancellationException

/** Install pinned language tools without replacing Debian or user-installed dependencies. */
internal object RuntimeToolchains {
    private fun declarations(context: Context): List<JSONObject> {
        val json = context.assets.open("runtime-manifest.json").bufferedReader().use { JSONObject(it.readText()) }
        val values = json.getJSONArray("toolchains")
        return (0 until values.length()).map { values.getJSONObject(it) }
    }

    fun ready(context: Context): Boolean = LinuxRuntimeInstaller.ready(context) && declarations(context).all { installed(context, it) }

    /** A compatible older toolchain is usable after an APK update, without a download. */
    fun compatible(context: Context): Boolean = runCatching {
        val root = File(context.filesDir, "runtime/debian")
        val contract = context.assets.open("runtime-manifest.json").bufferedReader().use { JSONObject(it.readText()) }.getJSONObject("compatibility")
        LinuxRuntimeInstaller.ready(context) &&
            File(root, "etc/os-release").readText().lineSequence().any { it == "VERSION_ID=\"${contract.getString("debian")}\"" } &&
            File(root, "usr/bin/python${contract.getString("python")}").canExecute() &&
            declarations(context).all { declaration ->
                val name = declaration.getString("name")
                val command = File(root, "usr/local/bin/$name").canonicalFile
                val tools = File(root, "usr/local/lib/rua/toolchains").canonicalFile
                val relative = command.relativeTo(tools).path
                val digest = relative.substringBefore('/')
                val marker = File(tools, "$digest/.rua-versions.json")
                val version = JSONObject(marker.readText()).getString(name).removePrefix("v").removePrefix("uv ").substringBefore(' ')
                digest.matches(Regex("[a-f0-9]{64}")) && command.canExecute() &&
                    version.substringBefore('.') == contract.getString(name) &&
                    declaration.getJSONObject("commands").keys().asSequence().all {
                        val tool = File(root, "usr/local/bin/$it")
                        tool.canExecute() && tool.canonicalPath.startsWith(File(tools, digest).path + "/")
                    }
            }
    }.getOrDefault(false)

    private fun installed(context: Context, value: JSONObject): Boolean = runCatching {
        val root = File(context.filesDir, "runtime/debian")
        val destination = File(root, "usr/local/lib/rua/toolchains/${value.getString("sha256")}")
        val versions = JSONObject(File(destination, ".rua-versions.json").readText())
        val commands = value.getJSONObject("commands")
        commands.keys().asSequence().all {
            val target = LinuxRuntimeInstaller.guestFile(destination, value.getString("directory") + "/" + commands.getString(it))
            val link = File(root, "usr/local/bin/$it")
            versions.getString(it).isNotBlank() && target.isFile && link.canonicalFile == target.canonicalFile
        }
    }.getOrDefault(false)

    @Synchronized
    fun installCached(context: Context, replaceManaged: Boolean = false, cancelled: () -> Boolean = { false }) {
        check(LinuxRuntimeInstaller.ready(context)) { "Debian 尚未就绪" }
        val root = File(context.filesDir, "runtime/debian")
        val tools = File(root, "usr/local/lib/rua/toolchains").apply { mkdirs() }
        val bin = File(root, "usr/local/bin").apply { mkdirs() }
        val artifacts = LinuxRuntimeInstaller.manifest(context).toolchains
        fun checkCancelled() { if (cancelled()) throw CancellationException("已取消环境准备") }
        for ((index, value) in declarations(context).withIndex()) {
            checkCancelled()
            require(value.getString("architecture") == "arm64")
            val artifact = artifacts[index]
            val destination = File(tools, artifact.sha256)
            val marker = File(destination, ".rua-versions.json")
            val commands = value.getJSONObject("commands")
            val names = commands.keys().asSequence().toList()
            require(names.all { it.matches(Regex("[a-z][a-z0-9]*")) })
            if (installed(context, value)) continue
            val journalFile = File(tools, "${value.getString("name")}.installation.json")
            if (journalFile.isFile) {
                val previous = JSONObject(journalFile.readText())
                val operation = previous.getString("operation_id")
                require(UUID.fromString(operation).toString() == operation)
                val owned = File(tools, ".staging-$operation")
                if (owned.exists()) LinuxRuntimeInstaller.removeStaging(owned)
                NativePayload.syncDirectory(tools)
            }
            val source = File(LinuxRuntimeInstaller.cache(context), artifact.sha256)
            check(LinuxRuntimeInstaller.validCached(source, artifact)) { "缺失或损坏的材料：${artifact.file}" }
            check(tools.usableSpace >= artifact.size * 5) { "存储空间不足，无法安装 ${value.getString("name")}" }
            LinuxRuntimeInstaller.report("正在安装 ${value.getString("name")}")
            if (!destination.exists()) {
                val operation = UUID.randomUUID().toString()
                val staging = File(tools, ".staging-$operation")
                LinuxRuntimeInstaller.atomicText(journalFile, JSONObject().put("operation_id", operation).toString())
                check(staging.mkdir())
                source.inputStream().use { LinuxRuntimeInstaller.extractRootfs(it, staging, cancelled) }
                checkCancelled()
                LinuxRuntimeInstaller.syncTree(staging)
                check(staging.renameTo(destination))
                NativePayload.syncDirectory(tools)
            }
            for (name in names) {
                checkCancelled()
                val target = LinuxRuntimeInstaller.guestFile(destination, value.getString("directory") + "/" + commands.getString(name))
                check(target.isFile && target.canExecute()) { "工具发行物缺少 $name" }
                val link = File(bin, name)
                // Never replace an independently installed user command.
                val managed = link.canonicalPath.startsWith(tools.canonicalPath + "/") &&
                    File(tools, link.canonicalFile.relativeTo(tools.canonicalFile).path.substringBefore('/') + "/.rua-versions.json").isFile
                check(!link.exists() || link.canonicalFile == target.canonicalFile || (replaceManaged && managed)) { "已有 $name，保留原文件" }
                val temporary = File(bin, ".$name-${UUID.randomUUID()}")
                try {
                    Os.symlink(target.relativeTo(bin).path, temporary.path)
                    check(temporary.renameTo(link))
                    NativePayload.syncDirectory(bin)
                } finally { temporary.delete() }
            }
            val probeId = UUID.randomUUID().toString()
            val results = File(context.filesDir, "tmp/$probeId")
            check(results.mkdir())
            try {
                // Versions come from actual guest execution, never inferred from a filename.
                LinuxRuntimeInstaller.guest(context, root, File(context.filesDir, "runtime/install-packages"), cancelled,
                    listOf("/bin/bash", "-c", "set -eu; " + names.joinToString("; ") { "$it --version > /tmp/$probeId/$it" }))
                val versions = JSONObject()
                for (name in names) versions.put(name, File(results, name).readText().trim().also { check(it.isNotEmpty()) })
                val actual = versions.getString(value.getString("name")).removePrefix("v").removePrefix("uv ").substringBefore(' ')
                check(actual == value.getString("version")) { "工具版本不符" }
                checkCancelled()
                LinuxRuntimeInstaller.atomicText(marker, versions.toString())
            } finally { LinuxRuntimeInstaller.removeStaging(results) }
        }
        LinuxRuntimeInstaller.report("Debian、Python 和 Node 工具环境已就绪")
    }
}
