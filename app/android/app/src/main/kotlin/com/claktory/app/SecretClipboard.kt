package com.claktory.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.PersistableBundle
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.security.MessageDigest

/**
 * What a clipboard holds, as far as [SecretClipboardCore] needs to know
 * without reading another app's clip: its description's label and, from
 * Android 8, the time the system took it.
 */
class ClipStamp(val label: String?, val timestamp: Long?)

/** The phone's clipboard for [SecretClipboardCore]: Android's, or a fake in tests. */
interface SecretClipboardPort {
    /** Puts [text] under [label], marked sensitive. */
    fun write(label: String, text: String)

    /** What holds it now; null when it's empty or Android won't say (not in front, from Android 10). */
    fun stamp(): ClipStamp?

    /** Its text, or null: asked only when the label is this app's own. */
    fun text(): String?

    fun clear()
}

/**
 * factory/secret-clipboard's logic (app/lib/people/secret_clipboard.dart):
 * [copy] puts a secret on the clipboard and answers a token for that
 * write; [clear] clears the clipboard only while it still holds that
 * write, never what the person copied since. From Android 8 the write is
 * known by the time the system took it; before, by this app's label and
 * the text's digest. When Android won't say what's there (the app isn't
 * in front, from Android 10), nothing is cleared and the token stays for
 * the next clear.
 */
class SecretClipboardCore(private val port: SecretClipboardPort) {
    private class Write(val timestamp: Long?, val digest: String)

    private var next = 0
    private val writes = HashMap<Int, Write>()

    fun copy(text: String): Int {
        port.write(LABEL, text)
        val token = ++next
        writes[token] = Write(port.stamp()?.timestamp, digest(text))
        return token
    }

    /** True when the clipboard held the write [token] made, and was cleared. */
    fun clear(token: Int): Boolean {
        val write = writes[token] ?: return false
        // Android won't say (not in front): the token waits for a clear that can check.
        val now = port.stamp() ?: return false
        writes.remove(token)
        if (now.label != LABEL) return false
        val same = if (write.timestamp != null && now.timestamp != null) {
            now.timestamp == write.timestamp
        } else {
            port.text()?.let { digest(it) } == write.digest
        }
        if (same) port.clear()
        return same
    }

    private fun digest(text: String) =
        MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }

    companion object {
        /** This app's label: Flutter's own copies have another. */
        const val LABEL = "Claktory code"

        /** ClipDescription.EXTRA_IS_SENSITIVE's name, which Android 13 reads and keyboards before it too. */
        const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"
    }
}

/** Android's clipboard, behind [SecretClipboardPort]. */
class AndroidClipboard(context: Context) : SecretClipboardPort {
    private val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

    override fun write(label: String, text: String) {
        val clip = ClipData.newPlainText(label, text)
        clip.description.extras = PersistableBundle().apply {
            putBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE, true)
        }
        clipboard.setPrimaryClip(clip)
    }

    override fun stamp(): ClipStamp? {
        val description = clipboard.primaryClipDescription ?: return null
        val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) description.timestamp else null
        return ClipStamp(description.label?.toString(), timestamp)
    }

    override fun text(): String? = clipboard.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.text?.toString()

    override fun clear() {
        // Android 9 can clear it; before, an empty text replaces it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) clipboard.clearPrimaryClip()
        else clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
    }
}

/**
 * factory/secret-clipboard: Copy code's secret on the clipboard, marked
 * sensitive (Android 13 hides it in its preview; keyboards keep it out of
 * their history), cleared later only if it's still there. Android has no
 * expiry of its own, so `seconds` is the app's to keep: it calls `clear`.
 */
class SecretClipboardChannel(
    messenger: BinaryMessenger,
    port: SecretClipboardPort,
) : MethodChannel.MethodCallHandler, AutoCloseable {
    constructor(messenger: BinaryMessenger, context: Context) : this(messenger, AndroidClipboard(context))

    private val channel = MethodChannel(messenger, "factory/secret-clipboard")
    val core = SecretClipboardCore(port)

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        try {
            when (call.method) {
                "copy" -> {
                    val text = call.argument<String>("text") ?: return result.error("failed", "There's no text to copy.", null)
                    result.success(core.copy(text))
                }
                "clear" -> {
                    val token = call.argument<Number>("token")?.toInt()
                        ?: return result.error("failed", "There's no token.", null)
                    core.clear(token)
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        } catch (error: Exception) {
            // The kind only: never the text.
            result.error("failed", error.javaClass.simpleName, null)
        }
    }

    override fun close() = channel.setMethodCallHandler(null)
}
