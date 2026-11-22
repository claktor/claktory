package com.claktory.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
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
 * "wired" | "other", …] (the one used first, first), expensive, constrained}.
 */
class NetworkPath(messenger: BinaryMessenger, context: Context) : EventChannel.StreamHandler, AutoCloseable {
    private val channel = EventChannel(messenger, "factory/network")
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var callback: ConnectivityManager.NetworkCallback? = null
    private var sink: EventChannel.EventSink? = null

    init {
        channel.setStreamHandler(this)
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
        stop()
        sink = events
        // The network now, then each change.
        send(connectivity.activeNetwork)
        val watching = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, capabilities: NetworkCapabilities) =
                send(capabilities)

            override fun onLost(network: Network) = send(null as NetworkCapabilities?)
        }
        connectivity.registerDefaultNetworkCallback(watching, main)
        callback = watching
    }

    override fun onCancel(arguments: Any?) = stop()

    override fun close() {
        stop()
        channel.setStreamHandler(null)
    }

    private fun stop() {
        callback?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        callback = null
        sink = null
    }

    private fun send(network: Network?) =
        send(network?.let { connectivity.getNetworkCapabilities(it) })

    private fun send(capabilities: NetworkCapabilities?) {
        val event = describe(
            capabilities?.let(::transportsOf),
            internet = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true,
            metered = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_METERED) != true,
            dataSaver = connectivity.restrictBackgroundStatus ==
                ConnectivityManager.RESTRICT_BACKGROUND_STATUS_ENABLED,
        )
        main.post { sink?.success(event) }
    }

    companion object {
        /** The default network's transports, as NetworkCapabilities says them. */
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
        fun describe(transports: Set<Int>?, internet: Boolean, metered: Boolean, dataSaver: Boolean): Map<String, Any> {
            if (transports == null) {
                return mapOf("satisfied" to false, "interfaces" to emptyList<String>(), "expensive" to false, "constrained" to false)
            }
            val interfaces = buildList {
                if (NetworkCapabilities.TRANSPORT_WIFI in transports) add("wifi")
                if (NetworkCapabilities.TRANSPORT_CELLULAR in transports) add("cellular")
                if (NetworkCapabilities.TRANSPORT_ETHERNET in transports) add("wired")
                if (transports.any { it !in PHYSICAL }) add("other")
            }
            return mapOf(
                "satisfied" to internet,
                "interfaces" to interfaces,
                "expensive" to metered,
                "constrained" to dataSaver,
            )
        }

        private val PHYSICAL = setOf(
            NetworkCapabilities.TRANSPORT_WIFI,
            NetworkCapabilities.TRANSPORT_CELLULAR,
            NetworkCapabilities.TRANSPORT_ETHERNET,
        )
    }
}
