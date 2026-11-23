package com.claktory.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.security.MessageDigest

/**
 * What a clipboard holds, as far as [SecretClipboardCore] needs to know
 * without reading another app's clip: its description's label, from
 * Android 8 the time the system took it, and the expiry this app put in
 * its extras (null when there's none).
 */
class ClipStamp(val label: String?, val timestamp: Long?, val expiresAt: Long? = null)

/** The phone's clipboard for [SecretClipboardCore]: Android's, or a fake in tests. */
interface SecretClipboardPort {
    /** Puts [text] under [label], marked sensitive, with [expiresAt] (ms since the epoch) in its extras when given. */
    fun write(label: String, text: String, expiresAt: Long?)

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
 *
 * A token lives only as long as the app's process, so the copy also
 * carries its expiry in the clip itself: [sweep], at launch and whenever
 * the app comes back, clears this app's code once that time has passed,
 * even if the app was stopped or left in between. The clip keeps the
 * expiry; the app keeps nothing. A sweep that finds the code still
 * unexpired answers when it expires, so the app (and the host) can sweep
 * again then, in a process that never made the copy.
 */
class SecretClipboardCore(
    private val port: SecretClipboardPort,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private class Write(val timestamp: Long?, val digest: String)

    private var next = 0
    private val writes = HashMap<Int, Write>()

    /** [seconds]: how long the code lasts; past it, [sweep] clears it. Null or not above 0: no expiry. */
    fun copy(text: String, seconds: Int? = null): Int {
        port.write(LABEL, text, seconds?.takeIf { it > 0 }?.let { clock() + it * 1000L })
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

    /**
     * Clears the clipboard when it holds this app's code past its expiry.
     * Reads only the clip's description, never its text, so Android shows
     * no "pasted from" notice. Answers when this app's code still there
     * expires (ms since the epoch), for a sweep then; null when there's
     * nothing of ours left: cleared now, none there, one with no expiry,
     * or Android won't say what's there (not in front). An expiry further
     * ahead than [MAX_AHEAD_MS] isn't one this app set (any app can label
     * a clip so): it's left alone and answers null, so nothing waits on it.
     */
    fun sweep(): Long? {
        val now = port.stamp() ?: return null
        if (now.label != LABEL) return null
        val expiresAt = now.expiresAt ?: return null
        val time = clock()
        // Compared without subtracting: a forged Long.MIN_VALUE would overflow.
        if (expiresAt > time + MAX_AHEAD_MS) return null
        if (expiresAt > time) return expiresAt
        port.clear()
        return null
    }

    private fun digest(text: String) =
        MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8)).joinToString("") { "%02x".format(it) }

    companion object {
        /** This app's label: Flutter's own copies have another. */
        const val LABEL = "Claktory code"

        /** ClipDescription.EXTRA_IS_SENSITIVE's name, which Android 13 reads and keyboards before it too. */
        const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"

        /** When the code expires, in ms since the epoch: in the clip's extras, for [sweep]. */
        const val EXTRA_EXPIRES_MS = "com.claktory.EXPIRES_MS"

        /** An hour: twice the longest a code lasts (add-a-device's 30 minutes). */
        const val MAX_AHEAD_MS = 3_600_000L
    }
}

/** Android's clipboard, behind [SecretClipboardPort]. */
class AndroidClipboard(context: Context) : SecretClipboardPort {
    private val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

    override fun write(label: String, text: String, expiresAt: Long?) {
        val clip = ClipData.newPlainText(label, text)
        clip.description.extras = PersistableBundle().apply {
            putBoolean(SecretClipboardCore.EXTRA_IS_SENSITIVE, true)
            if (expiresAt != null) putLong(SecretClipboardCore.EXTRA_EXPIRES_MS, expiresAt)
        }
        clipboard.setPrimaryClip(clip)
    }

    override fun stamp(): ClipStamp? {
        val description = clipboard.primaryClipDescription ?: return null
        val timestamp = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) description.timestamp else null
        val extras = description.extras
        val expiresAt = extras?.takeIf { it.containsKey(SecretClipboardCore.EXTRA_EXPIRES_MS) }
            ?.getLong(SecretClipboardCore.EXTRA_EXPIRES_MS)
        return ClipStamp(description.label?.toString(), timestamp, expiresAt)
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
 * expiry of its own: the app calls `clear` when `seconds` have passed,
 * and `sweep` at launch and whenever it comes back, which clears a code
 * whose `seconds` passed while the app was stopped or away (the expiry
 * rides in the clip's extras). `sweep` answers when a code still there
 * expires, or null. The host sweeps too, each time the app's window gets
 * focus (the first moment from Android 10 that the clip can be seen), and
 * whenever a sweep or a copy leaves a code with an expiry ahead, it
 * sweeps again at that moment (one at a time, the latest replacing the
 * last), so a code expiring while the app stays in front goes then.
 */
class SecretClipboardChannel(
    messenger: BinaryMessenger,
    port: SecretClipboardPort,
) : MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    constructor(messenger: BinaryMessenger, context: Context) : this(messenger, AndroidClipboard(context))

    private val channel = MethodChannel(messenger, "factory/secret-clipboard")
    val core = SecretClipboardCore(port)
    private val main = Handler(Looper.getMainLooper())
    private val due = Runnable {
        scheduled = null
        sweep()
    }

    /** When the host sweeps next, as a delay in ms from when it was set; null when nothing waits. For tests. */
    @Volatile
    var scheduled: Long? = null
        private set

    /** Sweeps now, and again when what's left of ours expires. */
    fun sweep(): Long? {
        val expiresAt = core.sweep()
        main.removeCallbacks(due)
        scheduled = null
        if (expiresAt != null) {
            // A little after, so the clock has passed it; a slept phone runs it later, and focus sweeps anyway.
            // Never more than core's limit ahead, whatever the clip says.
            val delay = (expiresAt - System.currentTimeMillis()).coerceIn(0, SecretClipboardCore.MAX_AHEAD_MS) + 100
            scheduled = delay
            main.postDelayed(due, delay)
        }
        return expiresAt
    }

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        try {
            when (call.method) {
                "copy" -> {
                    val text = call.argument<String>("text") ?: return result.error("failed", "There's no text to copy.", null)
                    val token = core.copy(text, call.argument<Number>("seconds")?.toInt())
                    sweep()
                    result.success(token)
                }
                "clear" -> {
                    val token = call.argument<Number>("token")?.toInt()
                        ?: return result.error("failed", "There's no token.", null)
                    core.clear(token)
                    result.success(null)
                }
                "sweep" -> result.success(sweep())
                else -> result.notImplemented()
            }
        } catch (error: Exception) {
            // The kind only: never the text.
            result.error("failed", error.javaClass.simpleName, null)
        }
    }

    override fun onFocus(focused: Boolean) {
        if (!focused) return
        try {
            sweep()
        } catch (_: Exception) {
            // The next focus, or the app's own sweep, tries again.
        }
    }

    override fun close() {
        main.removeCallbacks(due)
        scheduled = null
        channel.setMethodCallHandler(null)
    }
}
