package com.claktory.app

import android.app.Activity
import android.app.Service
import android.content.ComponentName
import android.content.Intent
import android.net.Uri
import android.os.Binder
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import androidx.browser.customtabs.CustomTabsCallback
import androidx.browser.customtabs.CustomTabsClient
import androidx.browser.customtabs.CustomTabsIntent
import androidx.browser.customtabs.CustomTabsServiceConnection
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Opening a website a friend shares over a pipe on this phone (W6). The
 * app forwards the share's port to Claktory's machine through the tunnel,
 * so the page lives only as long as this process can reach the network:
 * Android cuts a cached app off about 8 s after it leaves the screen.
 *
 * So the page opens in a Custom Tab with a session and KEEP_ALIVE: while
 * the tab is on screen the browser holds a binding to [ShareKeepAlive],
 * which keeps this process at the importance of a bound top app, its
 * network and CPU with it (measured on API 36: BTOP, the host reachable
 * for 5 minutes; without KEEP_ALIVE, or in the browser itself, cut within
 * 10 s). Home, or the screen going off, ends that as for the app itself.
 *
 * factory/share-browser:
 * - `open {url}` → "custom_tab" (kept alive), "browser" (no browser here
 *   takes Custom Tabs: opened, but it stops when Claktory is cut off), or
 *   error `bad_url` / `no_browser`. Only a share's own address opens:
 *   http://s-<id>.localhost:<port>/…, never a bare localhost or 127.0.0.1
 *   (Fugo, W6); the app calls it only from the person's tap on Open.
 * factory/share-browser/events: {visible: bool}, whether a tab this app
 *   opened is on screen and holding Claktory up (the app keeps its
 *   connection meanwhile, as for a send in flight).
 */
class ShareBrowser(
    messenger: BinaryMessenger,
    private val activity: Activity,
) : MethodChannel.MethodCallHandler, EventChannel.StreamHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/share-browser")
    private val events = EventChannel(messenger, "factory/share-browser/events")
    private val main = Handler(Looper.getMainLooper())
    private var sink: EventChannel.EventSink? = null
    private var connection: CustomTabsServiceConnection? = null

    init {
        channel.setMethodCallHandler(this)
        events.setStreamHandler(this)
        ShareKeepAlive.listener = { visible -> main.post { sink?.success(mapOf("visible" to visible)) } }
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "open" -> {
                val url = shareAddress(call.argument<String>("url"))
                    ?: return result.error("bad_url", "Only a shared website's own address opens here.", null)
                open(Uri.parse(url), result)
            }
            else -> result.notImplemented()
        }
    }

    private fun open(url: Uri, result: MethodChannel.Result) {
        val app = activity.applicationContext
        val provider = CustomTabsClient.getPackageName(app, null)
        if (provider == null) {
            return try {
                activity.startActivity(Intent(Intent.ACTION_VIEW, url).addCategory(Intent.CATEGORY_BROWSABLE))
                result.success("browser")
            } catch (error: Exception) {
                result.error("no_browser", "No browser on this phone opens it.", null)
            }
        }
        connection?.let { runCatching { app.unbindService(it) } }
        var answered = false
        val bound = object : CustomTabsServiceConnection() {
            override fun onCustomTabsServiceConnected(name: ComponentName, client: CustomTabsClient) {
                if (answered) return
                answered = true
                client.warmup(0)
                val tab = CustomTabsIntent.Builder(client.newSession(CustomTabsCallback()))
                    .setShareState(CustomTabsIntent.SHARE_STATE_OFF)
                    .build()
                tab.intent.setPackage(provider)
                tab.intent.putExtra(KEEP_ALIVE, Intent().setClassName(app.packageName, ShareKeepAlive::class.java.name))
                ShareKeepAlive.expected = true
                try {
                    tab.launchUrl(activity, url)
                    result.success("custom_tab")
                } catch (error: Exception) {
                    ShareKeepAlive.expected = false
                    result.error("no_browser", "No browser on this phone opens it.", null)
                }
            }

            override fun onServiceDisconnected(name: ComponentName?) {}
        }
        connection = bound
        if (!CustomTabsClient.bindCustomTabsService(app, provider, bound)) {
            connection = null
            answered = true
            result.error("no_browser", "No browser on this phone opens it.", null)
        }
    }

    /**
     * Claktory back on screen: no tab of its own is showing, so a binding
     * from now on isn't one until it opens another (Fugo, W6).
     */
    override fun onResume() {
        ShareKeepAlive.expected = false
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        sink = events
        events?.success(mapOf("visible" to ShareKeepAlive.visible))
    }

    override fun onCancel(arguments: Any?) {
        sink = null
    }

    override fun close() {
        channel.setMethodCallHandler(null)
        events.setStreamHandler(null)
        ShareKeepAlive.listener = null
        connection?.let { runCatching { activity.applicationContext.unbindService(it) } }
        connection = null
    }

    companion object {
        /** Chrome's extra: a service of the app's own it binds while the tab is shown. */
        const val KEEP_ALIVE = "android.support.customtabs.extra.KEEP_ALIVE"

        private val shareHost = Regex("^s-[a-z0-9]{1,64}\\.localhost$")

        /**
         * [url] rebuilt from its parts if it's a share's own address
         * (http, s-….localhost, a port, no user), else null: what opens
         * is exactly what was checked.
         */
        fun shareAddress(url: String?): String? {
            val uri = runCatching { java.net.URI(url ?: return null) }.getOrNull() ?: return null
            if (uri.scheme != "http" || uri.rawUserInfo != null) return null
            val host = uri.host ?: return null
            if (!shareHost.matches(host) || uri.port !in 1..65535) return null
            val path = uri.rawPath?.takeIf { it.startsWith("/") } ?: "/"
            val query = uri.rawQuery?.let { "?$it" } ?: ""
            val fragment = uri.rawFragment?.let { "#$it" } ?: ""
            return "http://$host:${uri.port}$path$query$fragment"
        }
    }
}

/**
 * What a Custom Tab [ShareBrowser] opened binds while it's on screen
 * (KEEP_ALIVE): nothing but the binding, which keeps Claktory's process,
 * and so the share's forward, up. Exported so the browser can bind it; a
 * binding this app didn't ask for (no tab opened) isn't reported.
 */
class ShareKeepAlive : Service() {
    override fun onBind(intent: Intent?): IBinder {
        if (expected) report(true)
        return Binder()
    }

    override fun onUnbind(intent: Intent?): Boolean {
        report(false)
        return false
    }

    companion object {
        /** A tab was opened and hasn't been seen closing yet. */
        @Volatile
        var expected = false

        @Volatile
        var visible = false
            private set

        @Volatile
        var listener: ((Boolean) -> Unit)? = null

        internal fun report(shown: Boolean) {
            if (visible == shown) return
            visible = shown
            listener?.invoke(shown)
        }
    }
}
