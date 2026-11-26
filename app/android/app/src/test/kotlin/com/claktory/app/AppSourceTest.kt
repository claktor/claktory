package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** factory/device's appSource: Google Play, Claktory's website, or nothing (Settings, ST1). */
class AppSourceTest {
    @Test
    fun playIsGooglePlay() {
        assertEquals("google_play", AppSource.classify("com.claktory.app", debuggable = false, installer = "com.android.vending"))
    }

    @Test
    fun theReleaseInstalledAnyOtherWayIsTheWebsites() {
        for (installer in listOf("com.google.android.packageinstaller", "com.android.packageinstaller", "com.android.shell", "com.sec.android.app.samsungapps", null)) {
            assertEquals("website", AppSource.classify("com.claktory.app", debuggable = false, installer = installer))
        }
    }

    @Test
    fun claktoryDevAndDebuggableBuildsHaveNone() {
        assertNull(AppSource.classify("com.claktory.app.dev", debuggable = false, installer = "com.android.vending"))
        assertNull(AppSource.classify("com.claktory.app.dev", debuggable = true, installer = null))
        assertNull(AppSource.classify("com.claktory.app", debuggable = true, installer = "com.android.vending"))
        // A copy under another name isn't Claktory's release.
        assertNull(AppSource.classify("com.example.claktory", debuggable = false, installer = "com.android.vending"))
    }
}
