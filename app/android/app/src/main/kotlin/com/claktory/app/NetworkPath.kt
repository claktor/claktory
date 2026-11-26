package com.claktory.app

import android.annotation.SuppressLint
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel

/**
 * The phone's network, for the app to reconnect the moment it changes
 * (app/lib/network.dart): Wi-Fi to cellular, a network coming back. iroh
 * can't see these on Android itself (netwatch's Android monitor does
 * nothing), so this channel is how the tunnel learns of them.
 *
 * factory/network (events), as on the iPhone and the Mac (NetworkPath.swift):
 * while listened to, the network now, then one event whenever it changes,
 * on the main thread: {satisfied, interfaces: ["wifi" | "cellular" |
 * "wired" | "other", …] (the one used first, first), expensive, constrained},
 * and on Android only, while there's a network, network: its handle (a
 * decimal string), so the app can tell a new network from the same one;
 * and blocked: whether Android keeps this app off that network now (a
 * background limit, Doze, Data Saver; from Android 10, false before), so
 * the app can redial the moment it's let through even while its screen is
 * hidden (a shared website open in a Custom Tab, W6), and not take a
 * refused dial for a dead Claktory.
 */
class NetworkPath(messenger: BinaryMessenger, context: Context) : EventChannel.StreamHandler, AutoCloseable {
    private val channel = EventChannel(messenger, "factory/network")
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var sink: EventChannel.EventSink? = null
    // Counts listenings, so a stopped callback's late events are dropped.
    private var generation = 0
    private var heard = false
    // Whether Android keeps the app off the default network now (Android 10+).
    @Volatile
    private var blocked = false
    // The last event sent: the same again says nothing new, and isn't sent.
    private var last: Map<String, Any>? = null

    init {
        channel.setStreamHandler(this)
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
        stop()
        sink = events
        val listening = ++generation
        heard = false
        blocked = false
        last = null
        // The default-network callback reports the network now as soon as
        // it's registered, then each change. Not getActiveNetwork() first:
        // while Doze or a background limit still blocks the app it answers
        // null, which read as "down" and made the callback's "up" a move.
        // Only when no callback comes (there's no network) is it "down".
        val watching = object : ConnectivityManager.NetworkCallback() {
            // Before Android 8 only this is sure to come for a new default
            // network; from 8 the capabilities follow it at once as well.
            override fun onAvailable(network: Network) =
                send(listening, network, connectivity.getNetworkCapabilities(network))

            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) =
                send(listening, network, capabilities)

            override fun onLost(network: Network) = send(listening, network, null)

            // Android 10+: told once after registering, then at each change.
            override fun onBlockedStatusChanged(network: Network, blocked: Boolean) {
                this@NetworkPath.blocked = blocked
                send(listening, network, connectivity.getNetworkCapabilities(network))
            }
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            connectivity.registerDefaultNetworkCallback(watching, main)
        } else {
            // Android 7 has only this one (its callbacks come on Android's
            // own thread); send() posts each event to the main thread anyway.
            connectivity.registerDefaultNetworkCallback(watching)
        }
        callback = watching
        main.postDelayed({
            if (generation == listening && !heard) {
                val now = connectivity.activeNetwork
                send(listening, now, now?.let { connectivity.getNetworkCapabilities(it) })
            }
        }, NO_NETWORK_AFTER_MS)
    }

    override fun onCancel(arguments: Any?) = stop()

    override fun close() {
        stop()
        channel.setStreamHandler(null)
    }

    private fun stop() {
        generation++
        callback?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        callback = null
        sink = null
    }

    /** One event of the listening numbered [listening]; dropped once it's stopped. */
    private fun send(listening: Int, network: Network?, capabilities: NetworkCapabilities?) {
        val event = describe(
            capabilities?.let(::transportsOf),
            internet = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true,
            metered = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true,
            dataSaver = connectivity.restrictBackgroundStatus ==
                ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED,
            network = network?.takeIf { capabilities != null }?.networkHandle,
            blocked = blocked,
        )
        main.post {
            if (generation != listening) return@post
            heard = heard || network != null
            // Before Android 8 a new network comes as onAvailable and then
            // its capabilities, and a loss can come twice: one event each.
            if (event == last) return@post
            last = event
            // What the app was told, for reading a reconnect afterwards: no
            // address, only the kind of network and Android's number for it.
            Log.i(TAG, "network: ${summary(event)}${network?.let { " net=${it.networkHandle}" } ?: ""}")
            sink?.success(event)
        }
    }

    companion object {
        private const val TAG = "Claktory"

        /** How long to wait for the callback before saying there's no network. */
        const val NO_NETWORK_AFTER_MS = 300L

        /** An event as one line: "up wifi,wired", "down". */
        fun summary(event: Map<String, Any>): String =
            (if (event["satisfied"] == true) "up " else "down ") +
                (event["interfaces"] as List<*>).joinToString(",") +
                (if (event["expensive"] == true) " expensive" else "") +
                (if (event["constrained"] == true) " constrained" else "") +
                (if (event["blocked"] == true) " blocked" else "")

        /**
         * The default network's transports, as NetworkCapabilities says them.
         * Wi-Fi Aware (8) and LoWPAN (8.1) are numbers compiled in; an older
         * Android just never reports them.
         */
        @SuppressLint("InlinedApi")
        private fun transportsOf(capabilities: NetworkCapabilities): Set<Int> =
            listOf(
                NetworkCapabilities.TRANSPORT_WIFI,
                NetworkCapabilities.TRANSPORT_CELLULAR,
                NetworkCapabilities.TRANSPORT_ETHERNET,
                NetworkCapabilities.TRANSPORT_VPN,
                NetworkCapabilities.TRANSPORT_BLUETOOTH,
                NetworkCapabilities.TRANSPORT_WIFI_AWARE,
                NetworkCapabilities.TRANSPORT_LOWPAN,
            ).filter(capabilities::hasTransport).toSet()

        /**
         * The event for a default network with [transports] (null: none).
         * The physical interface comes first, so a VPN over Wi-Fi moving to
         * cellular is still a move; a VPN, Bluetooth or the like is "other".
         */
        fun describe(
            transports: Set<Int>?,
            internet: Boolean,
            metered: Boolean,
            dataSaver: Boolean,
            network: Long? = null,
            blocked: Boolean = false,
        ): Map<String, Any> {
            if (transports == null) {
                return mapOf(
                    "satisfied" to false,
                    "interfaces" to emptyList<String>(),
                    "expensive" to false,
                    "constrained" to false,
                    "blocked" to false,
                )
            }
            val interfaces = buildList {
                if (NetworkCapabilities.TRANSPORT_WIFI in transports) add("wifi")
                if (NetworkCapabilities.TRANSPORT_CELLULAR in transports) add("cellular")
                if (NetworkCapabilities.TRANSPORT_ETHERNET in transports) add("wired")
                if (transports.any { it !in PHYSICAL }) add("other")
            }
            return buildMap {
                put("satisfied", internet)
                put("interfaces", interfaces)
                put("expensive", metered)
                put("constrained", dataSaver)
                // Up but kept off it by Android: satisfied stays what the
                // network is, so a block lifting isn't read as a move.
                put("blocked", blocked)
                // Which network it is (Android's handle for it, as a decimal
                // string): network.dart counts a move only when it changes, so
                // Wi-Fi coming and going on one network (the emulator's) or a
                // down-and-up of the same network isn't one. Only when up.
                if (network != null) put("network", network.toString())
            }
        }

        private val PHYSICAL = setOf(
            NetworkCapabilities.TRANSPORT_WIFI,
            NetworkCapabilities.TRANSPORT_CELLULAR,
            NetworkCapabilities.TRANSPORT_ETHERNET,
        )
    }
}
