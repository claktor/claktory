package com.claktory.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.DocumentsContract
import io.flutter.plugin.common.MethodChannel
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * factory/device's saveText {name, text}: the system's save dialog
 * (ACTION_CREATE_DOCUMENT, text/plain, [name] suggested), then [text] as
 * UTF-8 into the document the person made. Answers true once written,
 * false when they cancelled (and for a second save while one is open),
 * and the error "failed" when it couldn't be written; a document made but
 * not written is deleted rather than left empty or half written.
 *
 * The text may be the vault's recovery key: it's held only in memory while
 * the dialog is open, never in a file of the app's or a log. If Android
 * ends the process while the dialog is open, the text is gone, so the
 * document the dialog made is deleted when its answer comes back.
 */
class TextSave(
    private val activity: Activity,
    private val worker: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "Claktory save") },
) : HostLifecycle, AutoCloseable {
    private val main = Handler(Looper.getMainLooper())
    private var result: MethodChannel.Result? = null
    private var text: String? = null

    fun save(arguments: Any?, result: MethodChannel.Result) {
        val map = arguments as? Map<*, *>
        val name = (map?.get("name") as? String)?.takeIf { it.isNotBlank() }
        val text = map?.get("text") as? String
        if (name == null || text == null) return result.error(FAILED, "saveText needs a name and a text.", null)
        if (this.result != null || activity.isFinishing) return result.success(false)
        val intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
            .addCategory(Intent.CATEGORY_OPENABLE)
            .setType(MIME)
            .putExtra(Intent.EXTRA_TITLE, name)
        this.result = result
        this.text = text
        try {
            @Suppress("DEPRECATION")
            activity.startActivityForResult(intent, REQUEST)
        } catch (error: ActivityNotFoundException) {
            answer(Result.failure(Failed("This phone has no way to save a file.")))
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != REQUEST) return false
        val made = data?.data.takeIf { resultCode == Activity.RESULT_OK }
        val text = this.text
        this.text = null
        when {
            made == null && resultCode != Activity.RESULT_OK -> answer(Result.success(false))
            made == null -> answer(Result.failure(Failed("The save dialog gave no file.")))
            !TextSaves.writable(made.scheme, made.authority, activity.packageName) ->
                answer(Result.failure(Failed("The save dialog gave a place Claktory can't save to.")))
            text == null -> worker.execute { delete(made) }
            else -> worker.execute {
                val written = try {
                    val out = activity.contentResolver.openOutputStream(made, "wt")
                        ?: throw java.io.IOException("no stream")
                    out.use { it.write(text.toByteArray(Charsets.UTF_8)) }
                    Result.success(true)
                } catch (error: Exception) {
                    delete(made)
                    Result.failure(Failed("The file couldn't be written (${error.javaClass.simpleName})."))
                }
                main.post { answer(written) }
            }
        }
        return true
    }

    private fun delete(uri: Uri) {
        runCatching {
            if (DocumentsContract.isDocumentUri(activity, uri)) DocumentsContract.deleteDocument(activity.contentResolver, uri)
            else activity.contentResolver.delete(uri, null, null)
        }
    }

    private fun answer(answer: Result<Boolean>) {
        val pending = result ?: return
        result = null
        text = null
        answer.fold(
            onSuccess = { pending.success(it) },
            onFailure = { pending.error(FAILED, it.message, null) },
        )
    }

    override fun close() {
        answer(Result.success(false))
        worker.shutdown()
    }

    private class Failed(message: String) : Exception(message)

    companion object {
        /** Ours alone among the activity's requests (MainActivity forwards every result). */
        const val REQUEST = 0x4179
        const val MIME = "text/plain"
        const val FAILED = "failed"
    }
}

/** What saveText writes to: pure, so it's tested on the JVM. */
object TextSaves {
    /**
     * Only a document another app's provider made: content://, and never
     * one of the app's own providers, which the app could write to itself
     * (its cache, say) when a save dialog handed one back. A user id before
     * the authority (content://0@…) names the same provider.
     */
    fun writable(scheme: String?, authority: String?, packageName: String): Boolean {
        if (!scheme.equals("content", ignoreCase = true)) return false
        val provider = authority?.substringAfterLast('@')?.lowercase()?.takeIf { it.isNotEmpty() } ?: return false
        val own = packageName.lowercase()
        return provider != own && !provider.startsWith("$own.")
    }
}
