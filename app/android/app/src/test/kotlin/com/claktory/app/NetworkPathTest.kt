package com.claktory.app

import android.net.NetworkCapabilities.TRANSPORT_BLUETOOTH
import android.net.NetworkCapabilities.TRANSPORT_CELLULAR
import android.net.NetworkCapabilities.TRANSPORT_ETHERNET
import android.net.NetworkCapabilities.TRANSPORT_VPN
import android.net.NetworkCapabilities.TRANSPORT_WIFI
import org.junit.Assert.assertEquals
import org.junit.Test

/** factory/network's events are the iPhone's (app/lib/network.dart reads them). */
class NetworkPathTest {
    private fun event(
        transports: Set<Int>?,
        internet: Boolean = true,
        metered: Boolean = false,
        dataSaver: Boolean = false,
        network: Long? = null,
        blocked: Boolean = false,
    ) = NetworkPath.describe(transports, internet, metered, dataSaver, network, blocked)

    @Test
    fun noNetworkIsUnsatisfiedWithNoInterfaces() {
        assertEquals(
            mapOf(
                "satisfied" to false,
                "interfaces" to emptyList<String>(),
                "expensive" to false,
                "constrained" to false,
                "blocked" to false,
            ),
            event(null),
        )
    }

    @Test
    fun wifiAndCellularAreNamedAsOnTheIPhone() {
        assertEquals(listOf("wifi"), event(setOf(TRANSPORT_WIFI))["interfaces"])
        assertEquals(listOf("cellular"), event(setOf(TRANSPORT_CELLULAR))["interfaces"])
        assertEquals(listOf("wired"), event(setOf(TRANSPORT_ETHERNET))["interfaces"])
    }

    @Test
    fun aVpnPutsThePhysicalInterfaceFirst() {
        // Wi-Fi to cellular under a VPN is still a move of the primary.
        assertEquals(listOf("wifi", "other"), event(setOf(TRANSPORT_VPN, TRANSPORT_WIFI))["interfaces"])
        assertEquals(listOf("cellular", "other"), event(setOf(TRANSPORT_VPN, TRANSPORT_CELLULAR))["interfaces"])
        assertEquals(listOf("other"), event(setOf(TRANSPORT_BLUETOOTH))["interfaces"])
    }

    @Test
    fun meteredIsExpensiveAndDataSaverIsConstrained() {
        val event = event(setOf(TRANSPORT_CELLULAR), metered = true, dataSaver = true)
        assertEquals(true, event["expensive"])
        assertEquals(true, event["constrained"])
    }

    @Test
    fun aNetworkWithoutInternetIsntSatisfied() {
        assertEquals(false, event(setOf(TRANSPORT_WIFI), internet = false)["satisfied"])
    }

    @Test
    fun theLogLineReadsAsDartsSummary() {
        assertEquals("down ", NetworkPath.summary(event(null)))
        assertEquals("up wifi,wired", NetworkPath.summary(event(setOf(TRANSPORT_WIFI, TRANSPORT_ETHERNET))))
        assertEquals(
            "up cellular expensive constrained",
            NetworkPath.summary(event(setOf(TRANSPORT_CELLULAR), metered = true, dataSaver = true)),
        )
    }

    @Test
    fun aBlockIsSaidApartFromTheNetwork() {
        // Kept off a network that's up (W6: Claktory hidden under a Custom Tab, the screen off).
        val kept = event(setOf(TRANSPORT_WIFI), network = 7L, blocked = true)
        assertEquals(true, kept["blocked"])
        assertEquals(true, kept["satisfied"])
        assertEquals("7", kept["network"])
        assertEquals("up wifi blocked", NetworkPath.summary(kept))
        // Let through again: the same network, only blocked differs.
        val through = event(setOf(TRANSPORT_WIFI), network = 7L)
        assertEquals(kept - "blocked", through - "blocked")
        assertEquals(false, through["blocked"])
        assertEquals(false, event(null)["blocked"])
    }

    @Test
    fun theNetworkIsNamedOnlyWhileThereIsOne() {
        assertEquals("432902426637", event(setOf(TRANSPORT_WIFI), network = 432902426637L)["network"])
        assertEquals(false, event(setOf(TRANSPORT_WIFI)).containsKey("network"))
        assertEquals(false, NetworkPath.describe(null, false, false, false, network = 1L).containsKey("network"))
    }
}
