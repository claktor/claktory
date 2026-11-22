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
    private fun event(transports: Set<Int>?, internet: Boolean = true, metered: Boolean = false, dataSaver: Boolean = false) =
        NetworkPath.describe(transports, internet, metered, dataSaver)

    @Test
    fun noNetworkIsUnsatisfiedWithNoInterfaces() {
        assertEquals(
            mapOf("satisfied" to false, "interfaces" to emptyList<String>(), "expensive" to false, "constrained" to false),
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
}
