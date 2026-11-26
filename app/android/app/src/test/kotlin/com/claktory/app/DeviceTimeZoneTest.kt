package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.TimeZone

/** factory/device's timeZone: the IANA name, never an offset (#435). */
class DeviceTimeZoneTest {
    @Test
    fun answersTheZonesName() {
        val before = TimeZone.getDefault()
        try {
            TimeZone.setDefault(TimeZone.getTimeZone("Europe/London"))
            assertEquals("Europe/London", DeviceChannel.timeZone())
            TimeZone.setDefault(TimeZone.getTimeZone("America/Argentina/Buenos_Aires"))
            assertEquals("America/Argentina/Buenos_Aires", DeviceChannel.timeZone())
        } finally {
            TimeZone.setDefault(before)
        }
    }
}
