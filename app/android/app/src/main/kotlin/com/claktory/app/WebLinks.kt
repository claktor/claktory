package com.claktory.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.net.URLDecoder
import java.security.MessageDigest

/**
 * factory/device's webAuth and cancelWebAuth, as on the iPhone
 * (ios/Runner/AppDelegate.swift's WebAuth, ASWebAuthenticationSession):
 * webAuth {url, scheme} opens the service's page and answers the address
 * the service sent the browser back to on [scheme], or null when the
 * person closed the page first; cancelWebAuth closes it, and the waiting
 * call answers null. Pure, over [open] (shows the page) and [toApp]
 * (brings the app back over it), so it's tested on the JVM.
 *
 * On Android the page is a Custom Tab over the app; the service's redirect
 * to the app's scheme reaches [LinkActivity], which hands it
 * here. Only an address on the scheme asked for, while a sign-in waits,
 * and carrying the opened page's `state` when it had one, is taken, once:
 * any app can send the app such a link, so a forged one is ignored and the
 * sign-in goes on waiting for the service's. The service's state and PKCE,
 * checked by the backend, are still what make it trusted. Neither address
 * is ever logged: the redirect holds the authorization code.
 */
class WebAuthCore(
    private val open: (url: String) -> Boolean,
    private val toApp: () -> Unit,
) {
    private var waiting: MethodChannel.Result? = null
    private var scheme = ""

    /** The opened page's `state`, which the redirect must carry back; null if it had none. */
    private var state: String? = null

    /** The page was opened and the app left the screen for it. */
    private var shown = false

    fun start(url: String?, scheme: String?, result: MethodChannel.Result) {
        val web = url?.let { schemeOf(it) }
        if (url == null || scheme.isNullOrEmpty() || (web != "https" && web != "http")) {
            return result.error("arguments", "No address to open.", null)
        }
        cancel()
        waiting = result
        this.scheme = scheme.lowercase()
        state = parameter(url, "state")
        shown = false
        if (!open(url)) finish { it.error("failed", "Couldn't open the sign-in page.", null) }
    }

    /** The service sent the browser back to [address]; true if a sign-in took it. */
    fun redirected(address: String): Boolean {
        if (waiting == null || schemeOf(address) != scheme) return false
        val expected = state
        if (expected != null) {
            val back = parameter(address, "state") ?: return false
            if (!MessageDigest.isEqual(expected.toByteArray(), back.toByteArray())) return false
        }
        finish { it.success(address) }
        return true
    }

    /** Closes the page; the waiting call answers null. */
    fun cancel() {
        if (waiting == null) return
        val over = shown
        finish { it.success(null) }
        if (over) toApp()
    }

    /** The app's screen is going away: the waiting call answers null, and nothing is brought back. */
    fun abandon() = finish { it.success(null) }

    fun paused() {
        if (waiting != null) shown = true
    }

    /** Back in the app with the sign-in still waiting: the person closed the page. */
    fun resumed() {
        if (waiting != null && shown) finish { it.success(null) }
    }

    private fun finish(answer: (MethodChannel.Result) -> Unit) {
        val result = waiting ?: return
        waiting = null
        state = null
        shown = false
        answer(result)
    }

    companion object {
        /** [name]'s value in [address]'s query, or its fragment (where an implicit flow puts it); null if absent. */
        fun parameter(address: String, name: String): String? {
            val rest = address.substringAfter('?', "")
            val parts = rest.substringBefore('#').split('&') + address.substringAfter('#', "").split('&')
            for (part in parts) {
                val key = part.substringBefore('=')
                if (runCatching { URLDecoder.decode(key, "UTF-8") }.getOrNull() != name || !part.contains('=')) continue
                return runCatching { URLDecoder.decode(part.substringAfter('='), "UTF-8") }.getOrNull()
            }
            return null
        }

        fun schemeOf(address: String): String? =
            address.substringBefore(':', "").lowercase().takeIf { it.isNotEmpty() && it.all { c -> c.isLetterOrDigit() || c in "+.-" } }
    }
}

/** [WebAuthCore] in the app: the page in a Custom Tab, or the browser where there's none. */
class WebAuth(private val activity: Activity) : HostLifecycle, AutoCloseable {
    val core = WebAuthCore(
        open = { url ->
            val page = Intent(Intent.ACTION_VIEW, Uri.parse(url))
                .addCategory(Intent.CATEGORY_BROWSABLE)
                // A Custom Tab, without the library: a browser that has them shows one.
                .putExtras(Bundle().apply { putBinder(EXTRA_SESSION, null) })
            try {
                activity.startActivity(page)
                true
            } catch (error: ActivityNotFoundException) {
                false
            }
        },
        toApp = { activity.startActivity(backToApp(activity)) },
    )

    init {
        current = this
    }

    override fun onPause() = core.paused()

    override fun onResume() = core.resumed()

    override fun close() {
        // Not cancel(): bringing the app back while it closes would open it again.
        core.abandon()
        if (current === this) current = null
    }

    companion object {
        const val EXTRA_SESSION = "android.support.customtabs.extra.SESSION"

        /** The app's sign-in, for [LinkActivity]; only ever touched on the main thread. */
        var current: WebAuth? = null

        /** Brings the app's own screen back over the page, closing it. */
        fun backToApp(activity: Activity): Intent =
            Intent(activity, MainActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}

/**
 * Where links reach the app, showing nothing of its own: the address a
 * service's sign-in page sends the browser back to (the app's own scheme,
 * its application id, as the iPhone's is its bundle id) goes to the
 * waiting sign-in; a claktory:// link to [LinksChannel]. Either way the
 * app's one screen comes back over whatever showed, the sign-in page
 * included: the screen is found in its own task, never made twice.
 */
class LinkActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val link = intent?.dataString
        val signedIn = link != null && WebAuth.current?.core?.redirected(link) == true
        val back = WebAuth.backToApp(this)
        if (!signedIn && link != null) back.setAction(Intent.ACTION_VIEW).setData(Uri.parse(link))
        startActivity(back)
        finish()
    }
}

/**
 * factory/links, as on the Mac (app/macos/Runner/AppDelegate.swift's
 * Links): claktory://add?code=… (claktory-dev:// for Claktory Dev,
 * claktory-debug:// for a debug build), or a pairing code itself
 * (factory://?t=…, from a Mac's QR code), opens the app. Links that come
 * before the app's Flutter asks ("listen") wait and are its answer; after
 * that each is sent ("opened") as it comes. A launch from Recents doesn't
 * bring its old link again.
 */
class LinksChannel(messenger: BinaryMessenger, activity: Activity) :
    MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/links")
    private val waiting = mutableListOf<String>()
    private var listening = false
    private val scheme = activity.getString(R.string.link_scheme)

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
            else -> result.notImplemented()
        }
    }

    override fun onNewIntent(intent: Intent): Boolean = take(intent)

    private fun take(intent: Intent): Boolean {
        if (intent.action != Intent.ACTION_VIEW) return false
        val link = intent.dataString ?: return false
        val linkScheme = WebAuthCore.schemeOf(link)
        if (linkScheme != scheme && linkScheme != PAIRING_SCHEME) return false
        heard?.invoke(link)
        if (listening) channel.invokeMethod("opened", link) else waiting += link
        return true
    }

    override fun close() = channel.setMethodCallHandler(null)

    companion object {
        /** A pairing code's own scheme, which every build takes, as the iPhone's do. */
        const val PAIRING_SCHEME = "factory"

        /** Each link taken, for tests. */
        @Volatile
        internal var heard: ((String) -> Unit)? = null
    }
}
