package com.rua.mobile

import android.content.Context
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileOutputStream
import java.security.MessageDigest
import java.util.zip.ZipInputStream

/** APK-owned immutable resources. User data and the guest never live in this directory. */
internal object NativePayload {
    private const val NETWORK_LOG_DIRECTIVES =
        "iroh::endpoint=debug,iroh::address_lookup=debug,iroh::_events::conn=debug,iroh::_events::path=debug,iroh::_events::qnt=debug,iroh::socket::remote_map::remote_state=debug"

    @Volatile var consoleDirectory: String = ""
        private set
    @Volatile var executable: File? = null
        private set

    fun executable(context: Context): File = File(context.applicationInfo.nativeLibraryDir, "librua.so")

    @Synchronized
    fun prepare(context: Context, onPhase: (String) -> Unit = {}) {
        val binary = executable(context)
        check(binary.canExecute()) { "APK native Rua executable is missing" }
        executable = binary
        val expected = context.assets.open("rua-ui.sha256").bufferedReader().use { it.readText().trim() }
        check(expected.matches(Regex("[a-f0-9]{64}"))) { "Invalid APK resource identity" }
        val root = File(context.filesDir, "app-resources").apply { mkdirs() }
        val destination = File(root, expected)
        if (!destination.isDirectory) {
            val staging = File(root, "$expected.staging")
            // This directory contains only our incomplete APK resource extraction.
            if (staging.exists()) check(staging.deleteRecursively())
            check(staging.mkdirs())
            val digest = MessageDigest.getInstance("SHA-256")
            context.assets.open("rua-ui.zip").use { source ->
                java.security.DigestInputStream(source, digest).use { hashed ->
                    ZipInputStream(hashed).use { zip ->
                        while (true) {
                            val entry = zip.nextEntry ?: break
                            val output = safeResourceFile(staging, entry.name)
                            if (entry.isDirectory) output.mkdirs() else {
                                check(output.parentFile!!.isDirectory || output.parentFile!!.mkdirs())
                                FileOutputStream(output).use { sink -> zip.copyTo(sink); sink.fd.sync() }
                            }
                            zip.closeEntry()
                            onPhase("unpacking")
                        }
                        // Include the central directory in the digest, even if ZipInputStream stops early.
                        val buffer = ByteArray(8192)
                        while (hashed.read(buffer) >= 0) { }
                    }
                }
            }
            val actual = digest.digest().joinToString("") { "%02x".format(it) }
            check(actual == expected) { "APK resource checksum mismatch" }
            check(File(staging, "console/index.html").isFile) { "APK console is missing" }
            syncDirectories(staging)
            check(staging.renameTo(destination)) { "Cannot activate APK resources" }
            syncDirectory(root)
            onPhase("activated")
        }
        check(File(destination, "console/index.html").isFile) { "APK console is incomplete" }
        check(File(destination, "xterm-headless-bridge.mjs").isFile) { "APK terminal bridge is incomplete" }
        consoleDirectory = File(destination, "console").absolutePath
        File(MainActivity.HOME_DIR).mkdirs()
        File(context.filesDir, "tmp").mkdirs()
    }

    internal fun safeResourceFile(root: File, name: String): File {
        require(name.isNotEmpty() && !name.startsWith("/") && !name.contains('\\'))
        require(name.trimEnd('/').split('/').all { it.isNotEmpty() && it != "." && it != ".." })
        val target = File(root, name)
        require(target.canonicalPath.startsWith(root.canonicalPath + File.separator))
        return target
    }

    private fun syncDirectories(root: File) {
        root.walkBottomUp().filter { it.isDirectory }.forEach(::syncDirectory)
    }

    internal fun syncDirectory(directory: File) {
        val fd = Os.open(directory.absolutePath, OsConstants.O_RDONLY, 0)
        try { Os.fsync(fd) } finally { Os.close(fd) }
    }

    fun process(context: Context, args: List<String>): ProcessBuilder {
        val builder = ProcessBuilder(listOf(executable(context).absolutePath) + args)
            .directory(File(MainActivity.HOME_DIR))
            .redirectErrorStream(true)
            .redirectOutput(ProcessBuilder.Redirect.appendTo(File(MainActivity.RUA_LOG_PATH)))
        builder.environment().apply {
            clear()
            put("HOME", MainActivity.HOME_DIR)
            put("TMPDIR", File(context.filesDir, "tmp").absolutePath)
            put("PATH", "/system/bin")
            put("LANG", "C.UTF-8")
            put("TERM", "xterm-256color")
            put("RUA_MOBILE", "1")
            put("RUA_XTERM_BRIDGE", File(File(consoleDirectory).parentFile, "xterm-headless-bridge.mjs").absolutePath)
            put("RUA_ANDROID_NATIVE_DIR", context.applicationInfo.nativeLibraryDir)
            put("RUA_ANDROID_DATA_DIR", context.filesDir.absolutePath)
            put("RUA_PYTHON_INDEX_REGION", pythonIndexRegionFor())
            put("RUA_PHONE_SOCKET", PhoneUseBridge.socketPath(context).absolutePath)
            put("RUA_PHONE_HOST_PID", android.os.Process.myPid().toString())
            put("RUST_LOG", "info,$NETWORK_LOG_DIRECTIVES")
            put("RUA_DEBUG_UNQUIET", NETWORK_LOG_DIRECTIVES)
        }
        return builder
    }
}
