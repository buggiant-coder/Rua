package com.rua.mobile

import android.content.Context
import android.net.ConnectivityManager
import android.system.Os
import android.system.OsConstants
import org.apache.commons.compress.archivers.tar.TarArchiveInputStream
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.security.MessageDigest
import java.util.concurrent.TimeUnit
import java.util.zip.GZIPInputStream
import java.util.zip.ZipInputStream

/** One installation transaction shared by offline import and online preparation. */
internal object LinuxRuntimeInstaller {
    data class Artifact(val file: String, val size: Long, val sha256: String)
    data class Manifest(val id: String, val requiredFreeBytes: Long, val rootfs: Artifact, val packages: List<Artifact>, val toolchains: List<Artifact>) {
        val artifacts get() = listOf(rootfs) + packages + toolchains
    }

    @Volatile var status = "正在检查 Debian 工具环境"
        private set

    fun manifest(context: Context): Manifest {
        val json = context.assets.open("runtime-manifest.json").bufferedReader().use { JSONObject(it.readText()) }
        require(json.getInt("schema") == 1 && json.getString("architecture") == "arm64")
        require(android.os.Build.SUPPORTED_ABIS.contains("arm64-v8a")) { "此环境需要 ARM64 设备" }
        fun artifact(value: JSONObject): Artifact {
            val file = value.getString("file")
            require(file.matches(Regex("[a-zA-Z0-9_.+%~-]+")) && file != "." && file != "..")
            val sha = value.getString("sha256")
            require(sha.matches(Regex("[a-f0-9]{64}")))
            return Artifact(file, value.getLong("size").also { require(it > 0) }, sha)
        }
        val packages = json.getJSONArray("packages")
        val toolchains = json.getJSONArray("toolchains")
        return Manifest(json.getString("id"), json.getLong("requiredFreeBytes"), artifact(json.getJSONObject("rootfs")),
            (0 until packages.length()).map { artifact(packages.getJSONObject(it)) },
            (0 until toolchains.length()).map { artifact(toolchains.getJSONObject(it)) }).also {
            require(it.id.matches(Regex("[a-zA-Z0-9_.-]+")))
            require(it.artifacts.map { a -> a.file }.distinct().size == it.artifacts.size)
        }
    }

    fun ready(context: Context): Boolean = File(context.filesDir, "runtime/debian/.rua-runtime-ready").isFile

    fun describe(context: Context): String {
        val manifest = manifest(context)
        return "Debian ARM64、Python、Node/npm 及其依赖\n下载材料约 ${manifest.artifacts.sumOf { it.size } / 1_000_000} MB\n安装前需 ${manifest.requiredFreeBytes / 1_000_000} MB 可用空间。已有用户数据会保留。"
    }

    /** The ZIP is only a transport envelope: every member must match the APK's trusted manifest. */
    @Synchronized
    fun importOffline(context: Context, input: InputStream, cancelled: () -> Boolean = { false }) {
        val manifest = manifest(context)
        val cache = cache(context).apply { mkdirs() }
        status = "正在校验离线材料"
        try {
            val expected = manifest.artifacts.associateBy { it.file }
            val seen = mutableSetOf<String>()
            ZipInputStream(input).use { zip ->
                while (true) {
                    checkCancelled(cancelled)
                    val entry = zip.nextEntry ?: break
                    val artifact = expected[entry.name] ?: error("材料包含未知文件：${entry.name}")
                    check(!entry.isDirectory && seen.add(entry.name)) { "材料包含重复文件" }
                    verifiedCopy(zip, File(cache, artifact.sha256), artifact, cancelled)
                }
            }
            check(seen == expected.keys) { "离线材料不完整" }
            installCached(context, manifest, cancelled)
            RuntimeToolchains.installCached(context, cancelled = cancelled)
        } catch (error: Exception) {
            status = "环境准备失败：${error.message} · 点击重试"
            throw error
        }
    }

    fun cache(context: Context) = File(context.filesDir, "runtime/downloads")

    internal fun validCached(file: File, artifact: Artifact): Boolean =
        file.isFile && file.length() == artifact.size && file.inputStream().use { hash(it) } == artifact.sha256

    internal fun report(message: String) { status = message }

    internal fun verifiedCopy(input: InputStream, target: File, artifact: Artifact, cancelled: () -> Boolean = { false }) {
        target.parentFile!!.mkdirs()
        val temporary = File.createTempFile("download-", ".part", target.parentFile)
        try {
            val digest = MessageDigest.getInstance("SHA-256")
            var count = 0L
            FileOutputStream(temporary).use { output ->
                val buffer = ByteArray(64 * 1024)
                while (true) {
                    checkCancelled(cancelled)
                    val size = input.read(buffer)
                    if (size < 0) break
                    count += size
                    check(count <= artifact.size) { "材料大小超出清单" }
                    digest.update(buffer, 0, size)
                    output.write(buffer, 0, size)
                }
                output.fd.sync()
            }
            check(count == artifact.size && hex(digest.digest()) == artifact.sha256) { "材料校验失败：${artifact.file}" }
            check(temporary.renameTo(target))
            NativePayload.syncDirectory(target.parentFile!!)
        } finally { temporary.delete() }
    }

    /** Reconcile the commit marker with the durable journal after process death. */
    @Synchronized
    fun recover(context: Context) {
        val runtime = File(context.filesDir, "runtime")
        val journal = File(runtime, "installation.json")
        val previous = journal.takeIf { it.isFile }?.let { JSONObject(it.readText()) }
        // Only this installer's transport temporary files are disposable; verified cache stays.
        val temporaryDownloads = cache(context).listFiles()?.filter { it.isFile && it.name.matches(Regex("download-.*\\.part")) }.orEmpty()
        temporaryDownloads.forEach { check(it.delete()) { "无法清理下载临时文件" } }
        if (temporaryDownloads.isNotEmpty()) NativePayload.syncDirectory(cache(context))
        if (ready(context)) {
            if (previous?.optString("phase") != "ready") {
                cleanupOwnedStaging(runtime, previous)
                atomicText(journal, JSONObject().put("phase", "ready")
                    .put("id", File(runtime, "debian/.rua-runtime-ready").readText().trim()).toString())
            }
            status = "Debian 工具环境已就绪"
            return
        }
        cleanupOwnedStaging(runtime, previous)
        if (previous != null && previous.optString("phase") != "interrupted") {
            atomicText(journal, previous.put("phase", "interrupted").toString())
        }
    }

    private fun cleanupOwnedStaging(runtime: File, previous: JSONObject?) {
        if (previous == null) return
        val name = previous.optString("staging")
        if (name.isEmpty()) return
        val operation = previous.getString("operation_id")
        require(java.util.UUID.fromString(operation).toString() == operation && name == "debian.staging-$operation") { "无法确认临时目录归属，已保留原目录" }
        val owned = File(runtime, name)
        if (owned.exists()) { removeStaging(owned); NativePayload.syncDirectory(runtime) }
    }

    @Synchronized
    fun installCached(
        context: Context,
        manifest: Manifest = manifest(context),
        cancelled: () -> Boolean = { false },
        onPhase: (String) -> Unit = {},
    ) {
        recover(context)
        if (ready(context)) return
        checkCancelled(cancelled)
        val runtime = File(context.filesDir, "runtime").apply { mkdirs() }
        check(runtime.usableSpace >= manifest.requiredFreeBytes) { "存储空间不足：需要 ${manifest.requiredFreeBytes / 1_000_000} MB 可用空间" }
        (listOf(manifest.rootfs) + manifest.packages).forEach { artifact ->
            checkCancelled(cancelled)
            check(validCached(File(cache(context), artifact.sha256), artifact)) { "缺失或损坏的材料：${artifact.file}" }
        }
        val destination = File(runtime, "debian")
        check(!destination.exists()) { "已有未确认环境，保留原目录；请先完成环境恢复" }
        val operation = java.util.UUID.randomUUID().toString()
        val staging = File(runtime, "debian.staging-$operation")
        val journal = JSONObject().put("id", manifest.id).put("operation_id", operation).put("staging", staging.name)
        fun phase(name: String, message: String) {
            checkCancelled(cancelled)
            status = message
            atomicText(File(runtime, "installation.json"), journal.put("phase", name).toString())
            if (name != "unpacking") onPhase(name)
            checkCancelled(cancelled)
        }
        try {
            phase("unpacking", "正在安装 Debian")
            check(staging.mkdirs())
            File(cache(context), manifest.rootfs.sha256).inputStream().use { source -> extractRootfs(source, staging, cancelled) { onPhase("unpacking") } }
            for (dir in listOf("tmp", "guest-home", "home")) File(context.filesDir, dir).mkdirs()
            File(staging, "run/rua-install").mkdirs()
            val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            val dns = cm.activeNetwork?.let { cm.getLinkProperties(it) }?.dnsServers.orEmpty().mapNotNull { it.hostAddress }.ifEmpty { DnsSyncManager.DEFAULT_DNS }
            atomicText(File(staging, "etc/resolv.conf"), dns.joinToString("") { "nameserver $it\n" })
            atomicText(File(staging, "usr/sbin/policy-rc.d"), "#!/bin/sh\nexit 101\n")
            Os.chmod(File(staging, "usr/sbin/policy-rc.d").path, 0x1ed)
            val packages = File(runtime, "install-packages").apply { mkdirs() }
            for (artifact in manifest.packages) {
                checkCancelled(cancelled)
                File(cache(context), artifact.sha256).inputStream().use { verifiedCopy(it, File(packages, artifact.file), artifact) }
            }
            check(manifest.packages.isNotEmpty()) { "清单缺少 Python 依赖" }
            phase("configuring", "正在配置 Python")
            guest(context, staging, packages, cancelled, listOf("/bin/bash", "-c", "dpkg --force-depends --unpack /run/rua-install/*.deb && dpkg --configure --pending"))
            phase("probing", "正在验证工具环境")
            guest(context, staging, packages, cancelled, listOf("/bin/bash", "-c", "set -eu; test -x /usr/bin/prlimit; test -z \"$(dpkg --audit)\"; python3 -c 'import ssl,sqlite3,venv; print(\"RUA_DEBIAN_PYTHON_READY\")'; python3 -m venv /tmp/rua-install-venv; /tmp/rua-install-venv/bin/python -c 'print(\"RUA_VENV_READY\")'; rm -rf /tmp/rua-install-venv"))
            atomicText(File(staging, ".rua-runtime-ready"), manifest.id + "\n")
            syncTree(staging)
            phase("before_activation", "正在激活工具环境")
            check(staging.renameTo(destination)) { "无法激活 Debian" }
            NativePayload.syncDirectory(runtime)
            // The marker/rename is the commit point. A kill before the final journal write is recovered.
            onPhase("after_activation")
            atomicText(File(runtime, "installation.json"), JSONObject().put("phase", "ready").put("id", manifest.id).toString())
            status = "Debian 工具环境已就绪"
        } catch (error: Exception) {
            val message = if (generateSequence<Throwable>(error) { it.cause }.any {
                (it is android.system.ErrnoException && it.errno == OsConstants.ENOSPC) || it.message?.contains("No space left", true) == true
            }) "存储空间不足，请释放空间后重试" else error.message
            status = "环境准备失败：$message · 点击重试"
            // A full disk may prevent even the failure journal; the prior durable stage still owns cleanup.
            runCatching { atomicText(File(runtime, "installation.json"), journal.put("phase", "failed").put("message", message).toString()) }
            throw IllegalStateException(message, error)
        }
    }

    private fun isSymlink(file: File): Boolean =
        Os.lstat(file.path).st_mode and OsConstants.S_IFMT == OsConstants.S_IFLNK

    internal fun removeStaging(file: File) {
        if (!isSymlink(file) && file.isDirectory) {
            file.listFiles()?.forEach(::removeStaging)
        }
        check(file.delete()) { "无法清理未完成的安装" }
    }

    internal fun syncTree(file: File) {
        if (isSymlink(file) || !file.isDirectory) return
        file.listFiles()?.forEach(::syncTree)
        NativePayload.syncDirectory(file)
    }

    internal fun extractRootfs(input: InputStream, root: File, cancelled: () -> Boolean = { false }, onExtractedFile: () -> Unit = {}) {
        val hardlinks = mutableListOf<Pair<File, File>>()
        TarArchiveInputStream(GZIPInputStream(input)).use { tar ->
            while (true) {
                checkCancelled(cancelled)
                val entry = tar.nextEntry ?: break
                val name = entry.name.removePrefix("./").trimEnd('/')
                if (name.isEmpty() || name == ".") continue
                val target = guestFile(root, name)
                check(target.parentFile!!.isDirectory || target.parentFile!!.mkdirs())
                when {
                    entry.isDirectory -> check(target.isDirectory || target.mkdirs())
                    entry.isSymbolicLink -> {
                        val link = if (entry.linkName.startsWith('/')) entry.linkName.trimStart('/')
                            else name.substringBeforeLast('/', "").let { if (it.isEmpty()) entry.linkName else "$it/${entry.linkName}" }
                        val destination = guestFile(root, link)
                        // Relative links work both in the guest and for native capability inventory checks.
                        Os.symlink(destination.relativeTo(target.parentFile!!).path, target.path)
                    }
                    entry.isLink -> hardlinks.add(target to guestFile(root, entry.linkName.removePrefix("./")))
                    entry.isFile -> {
                        val temp = File.createTempFile("extract-", ".part", target.parentFile)
                        try {
                            FileOutputStream(temp).use { tar.copyTo(it); it.fd.sync() }
                            Os.chmod(temp.path, entry.mode and 0x1ff or 0x180)
                            check(temp.renameTo(target))
                            onExtractedFile()
                        } finally { temp.delete() }
                    }
                    else -> error("不支持的归档类型：$name")
                }
            }
        }
        for ((target, source) in hardlinks) {
            check(source.isFile && source.canonicalPath.startsWith(root.canonicalPath + "/"))
            val temporary = File.createTempFile("link-", ".part", target.parentFile)
            try {
                FileOutputStream(temporary).use { output -> source.inputStream().use { it.copyTo(output) }; output.fd.sync() }
                Os.chmod(temporary.path, Os.stat(source.path).st_mode and 0x1ff)
                check(temporary.renameTo(target))
            } finally { temporary.delete() }
        }
    }

    internal fun guestFile(root: File, name: String): File {
        require(!name.startsWith('/') && !name.contains('\\')) { "归档路径越界" }
        val parts = mutableListOf<String>()
        for (part in name.split('/')) {
            when (part) {
                "", "." -> Unit
                ".." -> { require(parts.isNotEmpty()) { "归档路径越界" }; parts.removeAt(parts.lastIndex) }
                else -> parts.add(part)
            }
        }
        val target = File(root, parts.joinToString("/"))
        require(target.canonicalPath.startsWith(root.canonicalPath + "/")) { "归档链接越界" }
        return target
    }

    internal fun atomicText(target: File, value: String) {
        target.parentFile!!.mkdirs()
        val temporary = File.createTempFile("write-", ".tmp", target.parentFile)
        try {
            FileOutputStream(temporary).use { it.write(value.toByteArray()); it.fd.sync() }
            check(temporary.renameTo(target))
            NativePayload.syncDirectory(target.parentFile!!)
        } finally { temporary.delete() }
    }

    private fun checkCancelled(cancelled: () -> Boolean) {
        if (cancelled()) throw java.util.concurrent.CancellationException("已取消环境准备")
    }

    internal fun guest(context: Context, root: File, packages: File, cancelled: () -> Boolean, command: List<String>,
                       extraBinds: List<Pair<String, String>> = emptyList(), timeoutMinutes: Long = 5) {
        val native = File(context.applicationInfo.nativeLibraryDir)
        val log = File(context.filesDir, "runtime/install.log")
        val args = listOf(File(native, "libproot.so").path, "-0", "--link2symlink", "--kill-on-exit",
            "-r", root.path, "-b", "/dev", "-b", "/proc", "-b", "/sys", "-b", "${packages.path}:/run/rua-install",
            "-b", "${File(context.filesDir, "tmp").path}:/tmp",
            "-b", "${File(context.filesDir, "guest-home").path}:/root") +
            extraBinds.flatMap { (host, guest) -> listOf("-b", "$host:$guest") } + listOf("-w", "/root", "/usr/bin/env", "-i",
            "HOME=/root", "PATH=/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", "LANG=C.UTF-8", "DEBIAN_FRONTEND=noninteractive") + command
        val builder = ProcessBuilder(listOf(File(native, "librua-process-group.so").path) + args).redirectErrorStream(true).redirectOutput(ProcessBuilder.Redirect.appendTo(log))
        builder.environment().apply {
            clear()
            put("LD_LIBRARY_PATH", native.path)
            put("PROOT_LOADER", File(native, "libproot-loader.so").path)
            put("PROOT_TMP_DIR", File(context.filesDir, "tmp").path)
            put("PROOT_NO_SECCOMP", "1")
        }
        val child = builder.start()
        try {
            val deadline = android.os.SystemClock.elapsedRealtime() + TimeUnit.MINUTES.toMillis(timeoutMinutes)
            while (!child.waitFor(100, TimeUnit.MILLISECONDS)) {
                checkCancelled(cancelled)
                check(android.os.SystemClock.elapsedRealtime() < deadline) { "安装探测超时" }
            }
            checkCancelled(cancelled)
            check(child.exitValue() == 0) { "安装探测失败，详见环境安装日志" }
        } finally {
            if (child.isAlive) {
                // The APK-native owner kills and reaps the isolated group on TERM.
                child.destroy()
                check(child.waitFor(5, TimeUnit.SECONDS)) { "安装进程清理未完成" }
            }
        }
    }

    private fun hash(input: InputStream): String {
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(64 * 1024)
        while (true) { val size = input.read(buffer); if (size < 0) break; digest.update(buffer, 0, size) }
        return hex(digest.digest())
    }
    private fun hex(bytes: ByteArray) = bytes.joinToString("") { "%02x".format(it) }
}
