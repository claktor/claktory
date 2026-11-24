package com.claktory.app

import java.io.File

/**
 * factory/device's shareFiles, checked before anything shows (#418): the
 * files the app wrote to share, each with its type, and the type the
 * share says it is. Only regular files inside the app's share folder
 * ([root], cache/claktory-share) go: anything else, a path that only
 * looks inside (.., a link out), a folder or a missing file refuses the
 * whole share. Pure, so it's tested on the JVM.
 */
object FileShare {
    /** At most this many files in one share, as a share into Claktory takes. */
    const val MAX_FILES = 20

    /** The folder under the cache the app writes its shares to, and the provider serves. */
    const val FOLDER = "claktory-share"

    data class Item(val file: File, val mime: String)

    /** [type] is what ACTION_SEND or ACTION_SEND_MULTIPLE says the files are. */
    data class Plan(val items: List<Item>, val type: String)

    /**
     * The share, or null when it can't go: no files, more than
     * [MAX_FILES], types that don't match the files one for one, or a file
     * that isn't one of the app's own in [root]. An empty type is guessed
     * from the name ([guess]), else it's any file's.
     */
    fun plan(paths: List<*>?, mimes: List<*>?, root: File, guess: (String) -> String?): Plan? {
        if (paths.isNullOrEmpty() || paths.size > MAX_FILES) return null
        if (mimes == null || mimes.size != paths.size) return null
        val folder = runCatching { root.canonicalFile }.getOrNull() ?: return null
        val items = paths.indices.map { index ->
            val path = paths[index] as? String ?: return null
            val given = mimes[index] as? String ?: return null
            val file = File(path)
            if (!file.isAbsolute) return null
            val real = runCatching { file.canonicalFile }.getOrNull() ?: return null
            // Inside the folder, not the folder itself, and a file.
            if (!real.path.startsWith(folder.path + File.separator) || !real.isFile) return null
            Item(real, given.trim().lowercase().takeIf(::wellFormed) ?: guessed(real.name, guess))
        }
        return Plan(items, common(items.map { it.mime }))
    }

    /** How long a share's folder is kept: the app picked may read its files later, through its grant. */
    const val KEEP_MS = 24 * 60 * 60 * 1000L

    /**
     * Deletes the share folders in [root] last changed more than [KEEP_MS]
     * before [now], but never one holding any of [keep] (the files of the
     * share being made, however deep). At each
     * share, as the app does at launch, so a phone that never relaunches it
     * doesn't keep files for days. Answers how many went.
     */
    fun sweep(root: File, now: Long, keep: Set<File> = emptySet()): Int {
        val kept = keep.mapNotNull { runCatching { it.canonicalFile }.getOrNull() }.toSet()
        return (root.listFiles() ?: return 0).count { folder ->
            val real = runCatching { folder.canonicalFile }.getOrNull()
            real != null &&
                kept.none { it == real || it.path.startsWith(real.path + File.separator) } &&
                now - folder.lastModified() > KEEP_MS &&
                folder.deleteRecursively()
        }
    }

    private fun guessed(name: String, guess: (String) -> String?): String {
        val extension = name.substringAfterLast('.', "").lowercase()
        return extension.takeIf { it.isNotEmpty() }?.let(guess)?.lowercase()?.takeIf(::wellFormed) ?: ANY
    }

    /** type/subtype, nothing more (no parameters, no wildcard type). */
    private fun wellFormed(mime: String): Boolean = MIME.matches(mime) && !mime.startsWith("*/")

    /** One type for them all: theirs if they share it, the kind with any subtype if they share the kind (images), else any. */
    fun common(mimes: List<String>): String {
        val distinct = mimes.toSet()
        if (distinct.size == 1) return distinct.single()
        val kinds = distinct.map { it.substringBefore('/') }.toSet()
        return if (kinds.size == 1) "${kinds.single()}/*" else ANY
    }

    const val ANY = "*/*"
    private val MIME = Regex("""[a-z0-9][a-z0-9!#$&^_.+-]*/[a-z0-9*][a-z0-9!#$&^_.+*-]*""")
}
