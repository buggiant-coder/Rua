package com.rua.mobile

import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileInputStream
import java.io.FileOutputStream
import java.util.concurrent.CancellationException

/** Filesystem operations for isolated environment update candidates. */
internal object RuntimeTree {
    fun sync(file: File) {
        when (Os.lstat(file.path).st_mode and OsConstants.S_IFMT) {
            OsConstants.S_IFDIR -> {
                file.listFiles().orEmpty().forEach(::sync)
                NativePayload.syncDirectory(file)
            }
            OsConstants.S_IFREG -> FileInputStream(file).use { it.fd.sync() }
        }
    }

    fun copy(source: File, target: File, cancelled: () -> Boolean, root: File, hiddenMounts: Set<String>) {
        if (cancelled()) throw CancellationException("已取消环境更新")
        val mode = Os.lstat(source.path).st_mode
        when (mode and OsConstants.S_IFMT) {
            OsConstants.S_IFLNK -> Os.symlink(Os.readlink(source.path), target.path)
            OsConstants.S_IFDIR -> {
                check(target.mkdir())
                // PRoot creates mode-000 placeholders for binds. Their contents are
                // always hidden by the canonical host bind; preserve every other directory.
                val relative = source.relativeTo(root).path
                val hidden = relative in hiddenMounts
                if (!hidden) {
                    val children = checkNotNull(source.listFiles()) { "无法读取已有 Debian 目录：$relative，原目录保留" }
                    children.forEach { copy(it, File(target, it.name), cancelled, root, hiddenMounts) }
                }
                if (!hidden) Os.chmod(target.path, mode and 0x1ff)
                NativePayload.syncDirectory(target)
            }
            OsConstants.S_IFREG -> {
                val temporary = File.createTempFile("copy-", ".part", target.parentFile)
                try {
                    FileOutputStream(temporary).use { output -> source.inputStream().use { it.copyTo(output) }; output.fd.sync() }
                    Os.chmod(temporary.path, mode and 0x1ff)
                    check(temporary.renameTo(target))
                } finally { temporary.delete() }
            }
            else -> error("已有 Debian 含无法复制的特殊文件，原目录已保留")
        }
    }

}
