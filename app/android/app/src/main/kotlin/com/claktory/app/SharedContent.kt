package com.claktory.app

import android.app.Activity
import android.content.ClipData
import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.Closeable
import java.io.File
import java.io.FilterInputStream
import java.io.InputStream
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.TimeoutException

/**
 * What another app shared, parsed and bounded before anything is read: the
 * text and subject, capped, and the files' addresses, only those this app
 * may read. Pure, so it's tested on the JVM. Any app on the phone can
 * share to Claktory, so nothing in it is trusted (Fugo, #366 A6).
 */
class ShareIntake(
    /** The package that provides [authority], or null when none does. */
    private val providerOf: (authority: String) -> String?,
    private val ownPackage: String,
) {
    class Plan(
        val text: String?,
        val textCut: Boolean,
        val subject: String?,
        /** The files to copy, as content:// addresses this app may read. */
        val uris: List<String>,
        /** too_many when there were more than [MAX_FILES] (none taken); refused when some were left out. */
        val error: String?,
    )

    /**
     * The share's plan, or null when there's nothing in it at all. A share
     * with no text and no file it can take still comes, with its error, so
     * the app can say so rather than show nothing.
     */
    fun plan(text: String?, subject: String?, uris: List<String>, leftOut: String? = null): Plan? {
        val (kept, cut) = cap(text, MAX_TEXT)
        val title = cap(subject, MAX_SUBJECT).first
        var error: String? = null
        val files = when {
            uris.size > MAX_FILES -> {
                error = TOO_MANY
                emptyList()
            }
            else -> uris.filter { readable(it) }.also { if (it.size < uris.size) error = REFUSED }
        }
        // What ShareActivity left out before the share reached the app.
        when (leftOut) {
            TOO_MANY -> error = TOO_MANY
            REFUSED -> if (error == null) error = REFUSED
        }
        if (kept == null && files.isEmpty() && error == null) return null
        return Plan(kept, cut, title, files, error)
    }

    /**
     * Only content:// from another app's provider: never file:// or any other
     * scheme (another app could point at this app's own files), never this
     * app's own provider, nor one that doesn't resolve, nor an authority with
     * a user or a port.
     */
    fun readable(uri: String): Boolean {
        val scheme = uri.substringBefore(':', "")
        if (!scheme.equals(ContentResolver.SCHEME_CONTENT, ignoreCase = true)) return false
        val rest = uri.substring(scheme.length + 1)
        if (!rest.startsWith("//")) return false
        val authority = rest.substring(2).substringBefore('/').substringBefore('?').substringBefore('#')
        if (authority.isEmpty() || '@' in authority || ':' in authority) return false
        val owner = providerOf(authority) ?: return false
        return owner != ownPackage
    }

    companion object {
        const val MAX_TEXT = 100_000
        const val MAX_SUBJECT = 1_000
        const val MAX_FILES = 20
        const val TOO_MANY = "too_many"
        const val REFUSED = "refused"

        /** [value] at most [max] characters, never cutting a character in two; empty is none. */
        fun cap(value: String?, max: Int): Pair<String?, Boolean> {
            if (value.isNullOrEmpty()) return null to false
            if (value.length <= max) return value to false
            var end = max
            if (Character.isHighSurrogate(value[end - 1])) end--
            return value.substring(0, end) to true
        }
    }
}

/**
 * Gives up on a provider that stops sending: once [stallMs] pass with no
 * bytes (or no answer to opening or asking about a file), [check] closes
 * what's being read, which ends a read blocked on it, and says so.
 */
class StallGuard(private val stallMs: Long, private val now: () -> Long = System::currentTimeMillis) {
    @Volatile
    private var last = now()

    @Volatile
    private var reading: Closeable? = null

    @Volatile
    var stalled = false
        private set

    fun progress() {
        last = now()
    }

    fun watch(stream: Closeable) {
        reading = stream
        progress()
    }

    /** True once it has stalled; then what's being read is closed. */
    fun check(): Boolean {
        if (!stalled && now() - last > stallMs) {
            stalled = true
            runCatching { reading?.close() }
        }
        return stalled
    }

    /** [input], telling this guard of every byte read. */
    fun guarded(input: InputStream): InputStream {
        watch(input)
        return object : FilterInputStream(input) {
            override fun read(): Int = super.read().also { progress() }

            override fun read(buffer: ByteArray, offset: Int, length: Int): Int =
                super.read(buffer, offset, length).also { progress() }
        }
    }
}

/**
 * factory/shared: what other apps share to Claktory (ACTION_SEND and
 * ACTION_SEND_MULTIPLE, through [ShareActivity]) goes to the app, which
 * lets the person choose an agent or chat and fills its message box as a
 * draft. Nothing here ever sends.
 *
 * - `listen` answers the shares that came before the app asked (one that
 *   launched it); after that each comes as `received`, as it's ready.
 * - A share is a map: {id (positive, rising), text, text_cut, subject,
 *   files (paths of copies, as pickAttachments answers), error}. The files
 *   are copied into the app's cache first ([AttachmentCopies], pruned after
 *   a day), so the other app's grant needn't outlive the moment. error is
 *   too_many, refused, too_large, no_space or copy_failed; a share with an
 *   error still comes, even with nothing else in it.
 * - `discard {id}` removes a share's copies, when the person didn't use it.
 *
 * Shares wait only in memory, never on the device: if the app's process
 * ends first, they're gone (and their copies go at the day's pruning).
 */
class SharedChannel(
    messenger: BinaryMessenger,
    private val activity: Activity,
    private val copies: AttachmentCopies = AttachmentCopies(File(activity.cacheDir, AttachmentCopies.FOLDER)),
    private val root: File = File(activity.cacheDir, AttachmentCopies.FOLDER),
) : MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/shared")
    private val main = Handler(Looper.getMainLooper())

    /** One share at a time, in the order they came. */
    private val worker: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "Claktory shares") }

    /** Where each share's files are read: a thread it can be abandoned on, if a provider hangs past closing. */
    private val readers: ExecutorService = Executors.newCachedThreadPool { Thread(it, "Claktory share reader").apply { isDaemon = true } }

    private val intake = ShareIntake(
        providerOf = { authority ->
            runCatching { activity.packageManager.resolveContentProvider(authority, 0)?.packageName }.getOrNull()
        },
        ownPackage = activity.packageName,
    )
    private val waiting = mutableListOf<Map<String, Any?>>()
    private val folders = HashMap<Int, File>()
    private var listening = false
    private var closed = false

    init {
        channel.setMethodCallHandler(this)
        val launch = activity.intent
        if (launch != null && launch.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0) take(launch)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "listen" -> {
                listening = true
                result.success(waiting.toList())
                waiting.clear()
            }
            "discard" -> {
                val id = call.argument<Number>("id")?.toInt()
                val folder = id?.let { folders.remove(it) }
                if (folder != null) worker.execute { if (folder.parentFile?.canonicalPath == root.canonicalPath) folder.deleteRecursively() }
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    override fun onNewIntent(intent: Intent): Boolean = take(intent)

    private fun take(intent: Intent): Boolean {
        val shared = try {
            Shared.read(intent)
        } catch (error: Exception) {
            // A malformed or hostile parcel: the share is dropped, the app goes on.
            null
        } ?: return false
        val token = intent.getStringExtra(ShareActivity.EXTRA_TOKEN)
        if (token != null && !taken.add(token)) return true
        val leftOut = runCatching { intent.getStringExtra(ShareActivity.EXTRA_LEFT_OUT) }.getOrNull()
        val plan = intake.plan(shared.text, shared.subject, shared.uris.map { it.toString() }, leftOut) ?: return true
        val id = ++next
        val resolver = activity.contentResolver
        val uris = shared.uris.filter { it.toString() in plan.uris }
        worker.execute {
            if (holdCopyMs > 0) Thread.sleep(holdCopyMs)
            var error = plan.error
            var files = emptyList<String>()
            if (uris.isNotEmpty()) {
                try {
                    files = copy(resolver, uris)
                } catch (refused: AttachmentCopies.Refused) {
                    error = refused.code
                } catch (failed: Exception) {
                    error = COPY_FAILED
                }
            }
            val share = mapOf(
                "id" to id,
                "text" to plan.text,
                "text_cut" to plan.textCut,
                "subject" to plan.subject,
                "files" to files,
                "error" to error,
            )
            main.post { deliver(id, files, share) }
        }
        return true
    }

    /** [uris] copied, giving up on a provider that stalls ([stallMs] with nothing). */
    private fun copy(resolver: ContentResolver, uris: List<Uri>): List<String> {
        val guard = StallGuard(stallMs)
        val reading = readers.submit<List<String>> {
            val sources = uris.map { uri ->
                val source = AttachmentPicker.Source.of(resolver, uri)
                guard.progress()
                AttachmentPicker.Source(source.name, source.size, source.mimeType) { guard.guarded(source.open()) }
            }
            copies.copyAll(sources)
        }
        while (true) {
            try {
                return reading.get(1, TimeUnit.SECONDS)
            } catch (waited: TimeoutException) {
                if (!guard.check()) continue
                // Closed: a read blocked on it ends now. If even that hangs, the reader is left behind.
                try {
                    return reading.get(5, TimeUnit.SECONDS)
                } catch (still: Exception) {
                    reading.cancel(true)
                    throw AttachmentCopies.Refused(COPY_FAILED, "A shared file stopped coming.")
                }
            } catch (failed: java.util.concurrent.ExecutionException) {
                throw failed.cause ?: failed
            }
        }
    }

    private fun deliver(id: Int, files: List<String>, share: Map<String, Any?>) {
        if (closed) return
        files.firstOrNull()?.let { folders[id] = File(it).parentFile ?: return@let }
        heard?.invoke(share)
        if (listening) channel.invokeMethod("received", share) else waiting += share
    }

    override fun close() {
        closed = true
        channel.setMethodCallHandler(null)
        worker.shutdown()
        readers.shutdown()
    }

    /** What an intent shared, read with typed getters only. */
    class Shared(val text: String?, val subject: String?, val uris: List<Uri>) {
        companion object {
            fun read(intent: Intent): Shared? {
                val multiple = when (intent.action) {
                    Intent.ACTION_SEND -> false
                    Intent.ACTION_SEND_MULTIPLE -> true
                    else -> return null
                }
                val extras = intent.extras ?: Bundle.EMPTY
                val text = extras.getCharSequence(Intent.EXTRA_TEXT)?.toString()
                    ?: if (multiple) extras.getCharSequenceArrayList(Intent.EXTRA_TEXT)?.joinToString("\n\n") { it.toString() } else null
                val subject = (extras.getCharSequence(Intent.EXTRA_SUBJECT) ?: extras.getCharSequence(Intent.EXTRA_TITLE))?.toString()
                val uris = if (multiple) streams(intent) else listOfNotNull(stream(intent))
                return Shared(text, subject, uris)
            }

            private fun stream(intent: Intent): Uri? =
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.extras?.get(Intent.EXTRA_STREAM) as? Uri
                }

            /** Only the addresses: anything else in the list is dropped, and a list that can't be read is none. */
            private fun streams(intent: Intent): List<Uri> = try {
                val list: List<*>? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.extras?.get(Intent.EXTRA_STREAM) as? ArrayList<*>
                }
                list?.filterIsInstance<Uri>() ?: emptyList()
            } catch (error: Exception) {
                emptyList()
            }
        }
    }

    companion object {
        const val COPY_FAILED = "copy_failed"

        /** How long a provider may send nothing before its share's files fail. */
        @Volatile
        internal var stallMs = 30_000L

        /** Shares already taken, by [ShareActivity]'s token: the screen made again doesn't take its share twice. */
        private val taken = java.util.Collections.synchronizedSet(HashSet<String>())

        /** Ids rise for the process's life. */
        private var next = 0

        /** How long each share waits before its files are read: tests only, to outlive ShareActivity's own grant. */
        @Volatile
        internal var holdCopyMs = 0L

        /** Each share delivered, for tests. */
        @Volatile
        internal var heard: ((Map<String, Any?>) -> Unit)? = null
    }
}

/**
 * Where other apps' shares come in. It shows nothing: it reads the share
 * with typed getters and hands the app's one screen a fresh, explicit
 * intent holding only the text, the subject and the files' addresses, with
 * a read grant for those alone. The incoming intent is never forwarded, so
 * nothing else in it (extras, nested intents) reaches the app.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val incoming = intent
        if (incoming != null) {
            // The grants are checked as the files are left out, but a share
            // that still can't be passed on (a grant that ended between the
            // two, say) comes without its files, and never ends the app.
            pass(incoming) { forward(this, it, ::readable) } || pass(incoming) { forward(this, it) { false } }
        }
        finish()
    }

    /** [forward] made and started: false when either failed. */
    private fun pass(incoming: Intent, forward: (Intent) -> Intent?): Boolean = try {
        forward(incoming)?.let(::startActivity)
        true
    } catch (error: Exception) {
        false
    }

    /**
     * Whether this app may read [uri] now, as passing it on needs: a grant
     * (the sharing app's, usually), or a provider anyone may read. A file
     * the sharing app didn't let Claktory read can't be passed on: the
     * system refuses the whole forward, which took the app down (Sale's
     * defect, #193).
     */
    private fun readable(uri: Uri): Boolean = try {
        val read = Intent.FLAG_GRANT_READ_URI_PERMISSION
        val pid = android.os.Process.myPid()
        val uid = android.os.Process.myUid()
        when {
            checkUriPermission(uri, pid, uid, read) == android.content.pm.PackageManager.PERMISSION_GRANTED -> true
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM ->
                checkContentUriPermissionFull(uri, pid, uid, read) == android.content.pm.PackageManager.PERMISSION_GRANTED
            else -> {
                val provider = uri.authority?.let { packageManager.resolveContentProvider(it, 0) }
                provider != null && provider.exported && provider.readPermission == null &&
                    provider.pathPermissions.isNullOrEmpty() &&
                    (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q || !provider.forceUriPermissions)
            }
        }
    } catch (error: Exception) {
        false
    }

    companion object {
        const val EXTRA_TOKEN = "com.claktory.SHARE_TOKEN"

        /**
         * Left out by [ShareActivity]: too_many when there were more files
         * than the app takes (none passed on), refused when some couldn't
         * be read. The app says so as for its own refusals.
         */
        const val EXTRA_LEFT_OUT = "com.claktory.SHARE_LEFT_OUT"

        /**
         * The app's screen's intent for [incoming], with only the files
         * [readable] lets through: a file the sharing app gave no access to
         * can't be passed on, and would stop the whole share.
         */
        fun forward(context: Context, incoming: Intent, readable: (Uri) -> Boolean): Intent? {
            val shared = SharedChannel.Shared.read(incoming) ?: return null
            var leftOut: String? = null
            val uris = when {
                shared.uris.size > ShareIntake.MAX_FILES -> {
                    leftOut = ShareIntake.TOO_MANY
                    emptyList()
                }
                else -> shared.uris.filter(readable).also { if (it.size < shared.uris.size) leftOut = ShareIntake.REFUSED }
            }
            val out = Intent(context, MainActivity::class.java)
                .setAction(incoming.action)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
                .putExtra(EXTRA_TOKEN, java.util.UUID.randomUUID().toString())
            shared.text?.let { out.putExtra(Intent.EXTRA_TEXT, it) }
            shared.subject?.let { out.putExtra(Intent.EXTRA_SUBJECT, it) }
            leftOut?.let { out.putExtra(EXTRA_LEFT_OUT, it) }
            if (uris.isNotEmpty()) {
                if (incoming.action == Intent.ACTION_SEND) {
                    out.putExtra(Intent.EXTRA_STREAM, uris.first())
                } else {
                    out.putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(uris))
                }
                val clip = ClipData.newRawUri("", uris.first())
                uris.drop(1).forEach { clip.addItem(ClipData.Item(it)) }
                out.clipData = clip
                // Read only: nothing writable or persistable is passed on, whatever came in.
                out.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            }
            return out
        }
    }
}
