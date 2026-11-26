package com.claktory.app

import android.annotation.SuppressLint
import android.app.Activity
import android.app.AlertDialog
import android.content.ActivityNotFoundException
import android.content.ClipboardManager
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ext.SdkExtensions
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.UUID
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * factory/device's pickAttachments, as on the iPhone (ios/Runner/
 * AppDelegate.swift, Attach): a menu of where to attach from (photos or
 * videos, files, and a pasted image when the clipboard holds one), then the
 * system's own picker: the photo picker, the document picker. Asked with
 * {"camera": true} (the message box's paperclip) on a phone that can take
 * one, Take photo comes first: choosing it closes the menu and fails the
 * pick with take_photo, so the app can explain the camera before asking
 * for it, then call takePhoto ([PhotoCapture]). No storage
 * permission is ever asked for: both hand over only what the person chose.
 * What was chosen is copied into the app's cache ([AttachmentCopies]) and
 * the copies' paths answered, in the order chosen; nothing chosen (or
 * cancelled) answers an empty list, and so does a second pick while one is
 * open. A file over [AttachmentCopies.MAX_SIZE] (the most a Claktory takes)
 * or no room for the copies fails the pick with a code that says which.
 */
class AttachmentPicker(
    private val activity: Activity,
    private val copies: AttachmentCopies = AttachmentCopies(File(activity.cacheDir, AttachmentCopies.FOLDER)),
    private val worker: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "Claktory attachments") },
    private val camera: PhotoCapture? = null,
) : HostLifecycle, AutoCloseable {
    private val main = Handler(Looper.getMainLooper())
    private var result: MethodChannel.Result? = null
    private var menu: AlertDialog? = null

    fun pick(result: MethodChannel.Result, arguments: Any? = null) {
        if (this.result != null || activity.isFinishing) return result.success(emptyList<String>())
        this.result = result
        val photo = (arguments as? Map<*, *>)?.get("camera") == true && camera?.usable() == true
        val choices = buildList {
            if (photo) add(activity.getString(R.string.attach_take_photo) to ::takePhoto)
            add(activity.getString(R.string.attach_photos) to ::photos)
            add(activity.getString(R.string.attach_files) to ::files)
            if (clipboardHasImage()) add(activity.getString(R.string.attach_paste) to ::paste)
        }
        menu = AlertDialog.Builder(activity)
            .setItems(choices.map { it.first }.toTypedArray()) { _, which -> choices[which].second() }
            .setNegativeButton(R.string.attach_cancel) { _, _ -> finish(emptyList()) }
            // Back, or a tap outside: nothing chosen.
            .setOnCancelListener { finish(emptyList()) }
            .show()
    }

    // The photo picker's fields need Android 13, or 11+ with the R extension
    // 2, which photoPickerAvailable() checks; lint can't see through it.
    @SuppressLint("NewApi", "InlinedApi")
    private fun photos() {
        val intent = if (photoPickerAvailable()) {
            Intent(MediaStore.ACTION_PICK_IMAGES)
                .putExtra(MediaStore.EXTRA_PICK_IMAGES_MAX, MediaStore.getPickImagesMaxLimit())
        } else {
            // Before the photo picker: the system's chooser for pictures and
            // videos, still only what's chosen.
            Intent(Intent.ACTION_GET_CONTENT)
                .setType("*/*")
                .putExtra(Intent.EXTRA_MIME_TYPES, arrayOf("image/*", "video/*"))
                .addCategory(Intent.CATEGORY_OPENABLE)
                .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
        }
        start(intent)
    }

    /** Take photo: the app explains the camera, then calls takePhoto. */
    private fun takePhoto() = fail("take_photo", "Take photo was chosen.")

    private fun files() = start(
        Intent(Intent.ACTION_OPEN_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType("*/*")
            .putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true),
    )

    private fun paste() {
        val uri = clipboardImage() ?: return finish(emptyList())
        copy(listOf(uri))
    }

    private fun start(intent: Intent) {
        try {
            @Suppress("DEPRECATION")
            activity.startActivityForResult(intent, REQUEST)
        } catch (error: ActivityNotFoundException) {
            fail("picker_unavailable", "This phone has no picker for that.")
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != REQUEST) return false
        if (resultCode != Activity.RESULT_OK || data == null) {
            finish(emptyList())
        } else {
            copy(chosen(data))
        }
        return true
    }

    private fun copy(uris: List<Uri>) {
        val resolver = activity.contentResolver
        worker.execute {
            val answer = try {
                Result.success(copies.copyAll(uris.map { uri -> Source.of(resolver, uri) }))
            } catch (error: AttachmentCopies.Refused) {
                Result.failure(error)
            } catch (error: Exception) {
                Result.failure(AttachmentCopies.Refused("copy_failed", "A chosen file couldn't be read (${error.javaClass.simpleName})."))
            }
            main.post {
                answer.fold(
                    onSuccess = { finish(it) },
                    onFailure = { error -> (error as AttachmentCopies.Refused).let { fail(it.code, it.message ?: "") } },
                )
            }
        }
    }

    private val clipboard get() = activity.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager

    /**
     * Whether the clipboard holds an image, from its description only:
     * reading the clip itself would tell the person the app pasted, when
     * they only opened the menu.
     */
    private fun clipboardHasImage(): Boolean = clipboard?.primaryClipDescription?.hasMimeType("image/*") == true

    /** The clipboard's image, read when the person chose Paste image. */
    private fun clipboardImage(): Uri? =
        clipboard?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri

    private fun finish(paths: List<String>) {
        menu = null
        result?.success(paths)
        result = null
    }

    private fun fail(code: String, message: String) {
        menu = null
        result?.error(code, message, null)
        result = null
    }

    /** The menu taken away as choosing from it does, without cancelling (tests only). */
    internal fun dismissMenuForTest() {
        menu?.dismiss()
    }

    /** The menu's items as it shows them (tests only). */
    internal fun menuItemsForTest(): List<String> =
        menu?.listView?.adapter?.let { adapter -> (0 until adapter.count).map { adapter.getItem(it).toString() } }.orEmpty()

    /** Chooses the menu's item at [index], as a tap on it does (tests only). */
    internal fun chooseForTest(index: Int) {
        val list = menu?.listView ?: return
        list.performItemClick(list, index, list.adapter.getItemId(index))
    }

    override fun close() {
        menu?.dismiss()
        finish(emptyList())
        worker.shutdown()
    }

    /** What was chosen, in the order chosen. */
    private fun chosen(data: Intent): List<Uri> {
        val clip = data.clipData
        if (clip != null) return (0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri }
        return listOfNotNull(data.data)
    }

    /** Android 13, or 11 and 12 with the photo picker brought by a system update. */
    private fun photoPickerAvailable(): Boolean = when {
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU -> true
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.R -> SdkExtensions.getExtensionVersion(Build.VERSION_CODES.R) >= 2
        else -> false
    }

    /** One chosen file: its name and size as its provider says, and its bytes. */
    class Source(val name: String?, val size: Long?, val mimeType: String?, val open: () -> InputStream) {
        companion object {
            fun of(resolver: ContentResolver, uri: Uri): Source {
                var name: String? = null
                var size: Long? = null
                if (uri.scheme == ContentResolver.SCHEME_CONTENT) {
                    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { row ->
                        if (row.moveToFirst()) {
                            name = row.getColumnIndex(OpenableColumns.DISPLAY_NAME).takeIf { it >= 0 && !row.isNull(it) }?.let(row::getString)
                            size = row.getColumnIndex(OpenableColumns.SIZE).takeIf { it >= 0 && !row.isNull(it) }?.let(row::getLong)
                        }
                    }
                } else {
                    name = uri.lastPathSegment
                }
                // A provider that doesn't know the size yet (MediaStore before
                // it has scanned a new file): the file's own length, if it says.
                if (size == null) {
                    size = runCatching { resolver.openAssetFileDescriptor(uri, "r")?.use { it.length } }
                        .getOrNull()?.takeIf { it >= 0 }
                }
                val type = resolver.getType(uri)
                return Source(name, size, type, { resolver.openInputStream(uri) ?: throw IOException("nothing to read") })
            }
        }
    }

    companion object {
        /** Ours alone among the activity's requests (MainActivity forwards every result). */
        const val REQUEST = 0x4174
    }
}

/**
 * The copies of what the person attached, in [root] (the app's cache):
 * each pick's in a new folder of its own, so names never clash; folders
 * from earlier picks are removed after a day, by which time their files
 * were sent. Pure file work, so it's tested on the JVM.
 */
class AttachmentCopies(
    private val root: File,
    private val maxSize: Long = MAX_SIZE,
    private val extensionOf: (String) -> String? = { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) },
    private val spare: () -> Long = { root.apply { mkdirs() }.usableSpace },
    private val now: () -> Long = System::currentTimeMillis,
) {
    /** Why a pick failed: [code] for the app, the message for a person reading a log. */
    class Refused(val code: String, message: String) : Exception(message)

    /** Copies [sources] into a new folder; their paths, in order. */
    fun copyAll(sources: List<AttachmentPicker.Source>): List<String> {
        prune()
        if (sources.isEmpty()) return emptyList()
        sources.firstOrNull { (it.size ?: 0) > maxSize }?.let { throw tooLarge() }
        val declared = sources.sumOf { it.size ?: 0 }
        if (declared + MARGIN > spare()) throw noRoom()
        val folder = File(root, UUID.randomUUID().toString())
        if (!folder.mkdirs()) throw Refused("copy_failed", "The cache folder couldn't be made.")
        val used = HashSet<String>()
        try {
            return sources.mapIndexed { index, source ->
                val file = File(folder, unique(fileName(source, index, extensionOf), used))
                copy(source, file)
                file.path
            }
        } catch (error: Exception) {
            folder.deleteRecursively()
            throw if (error is Refused) error else Refused("copy_failed", "A chosen file couldn't be read (${error.javaClass.simpleName}).")
        }
    }

    /**
     * Moves [file] (one the app made, such as the camera's photo) into a
     * new folder as [name], kept as a pick's copies are; its path. Over
     * [maxSize] refuses it as too large, and the file stays where it was.
     */
    fun adopt(file: File, name: String): String {
        prune()
        if (file.length() > maxSize) throw tooLarge()
        val folder = File(root, UUID.randomUUID().toString())
        if (!folder.mkdirs()) throw Refused("copy_failed", "The cache folder couldn't be made.")
        val target = File(folder, name)
        try {
            // Both in the app's cache, so a rename; a copy when they aren't on one disk.
            if (!file.renameTo(target)) file.copyTo(target)
        } catch (error: Exception) {
            folder.deleteRecursively()
            throw Refused("copy_failed", "The photo couldn't be kept (${error.javaClass.simpleName}).")
        }
        return target.path
    }

    /** Folders from picks over a day old. */
    fun prune() {
        val cutoff = now() - DAY
        root.listFiles()?.filter { it.isDirectory && it.lastModified() < cutoff }?.forEach { it.deleteRecursively() }
    }

    private fun copy(source: AttachmentPicker.Source, file: File) {
        source.open().use { input ->
            file.outputStream().use { output ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    total += read
                    // What a provider said of its size isn't trusted.
                    if (total > maxSize) throw tooLarge()
                    output.write(buffer, 0, read)
                }
            }
        }
    }

    private fun tooLarge() = Refused("too_large", "A chosen file is larger than a Claktory takes (2 GB).")

    private fun noRoom() = Refused("no_space", "This phone hasn't room to copy the chosen files.")

    companion object {
        const val FOLDER = "Attachments"

        /** The most a Claktory takes in one upload (crates/factoryd/src/uploads.rs, MAX_SIZE). */
        const val MAX_SIZE: Long = 2L * 1024 * 1024 * 1024

        /** Left free beyond the copies, so the phone isn't filled to the brim. */
        const val MARGIN: Long = 256L * 1024 * 1024

        private const val DAY = 24L * 3_600_000

        /**
         * A name to keep the copy under: its provider's, made safe for a
         * file and to show (no control, bidi or format characters, no
         * folders, no leading dots, at most 120 characters, its extension
         * kept), else "Attachment" with one for its type.
         */
        fun fileName(
            source: AttachmentPicker.Source,
            index: Int,
            extensionOf: (String) -> String? = { MimeTypeMap.getSingleton().getExtensionFromMimeType(it) },
        ): String {
            val given = source.name?.filterNot(::invisible)?.substringAfterLast('/')?.substringAfterLast('\\')
                ?.replace(Regex("[\\u0000-\\u001f\\u007f:*?\"<>|]"), "_")
                ?.trim()?.trimStart('.')
            if (!given.isNullOrEmpty()) return shorten(given)
            val extension = source.mimeType?.let(extensionOf)
            val base = if (index == 0) "Attachment" else "Attachment ${index + 1}"
            return if (extension.isNullOrEmpty()) base else "$base.$extension"
        }

        /**
         * Control, bidi and format characters, which could make a name read
         * as another (factoryd's shown_name drops the same): dropped before
         * anything else, since the message box shows the name.
         */
        fun invisible(char: Char): Boolean =
            Character.isISOControl(char) || char in '\u200B'..'\u200F' || char in '\u202A'..'\u202E' ||
                char in '\u2060'..'\u2064' || char in '\u2066'..'\u2069' || char == '\uFEFF'

        private fun shorten(name: String): String {
            if (name.length <= 120) return name
            val dot = name.lastIndexOf('.')
            val extension = if (dot > 0 && name.length - dot <= 16) name.substring(dot) else ""
            return name.take(120 - extension.length) + extension
        }

        /** [name], or "name 2.ext", "name 3.ext"… when one before has it. */
        fun unique(name: String, used: MutableSet<String>): String {
            if (used.add(name.lowercase())) return name
            val dot = name.lastIndexOf('.').takeIf { it > 0 } ?: name.length
            var n = 2
            while (true) {
                val next = name.substring(0, dot) + " $n" + name.substring(dot)
                if (used.add(next.lowercase())) return next
                n++
            }
        }
    }
}
