package com.claktory.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TextSavesTest {
    private val app = "com.claktory.app.dev"

    @Test
    fun aDocumentAnotherAppsProviderMade() {
        assertTrue(TextSaves.writable("content", "com.android.externalstorage.documents", app))
        assertTrue(TextSaves.writable("content", "com.google.android.apps.docs.storage", app))
        assertTrue(TextSaves.writable("CONTENT", "media", app))
        assertTrue("another user's provider is still another app's", TextSaves.writable("content", "10@com.android.providers.downloads.documents", app))
        assertTrue("a name that only starts like the app's", TextSaves.writable("content", "com.claktory.app.devices", app))
    }

    @Test
    fun neverTheAppsOwnProvidersOrAFile() {
        for ((scheme, authority) in listOf(
            "content" to "com.claktory.app.dev",
            "content" to "com.claktory.app.dev.camera",
            "content" to "com.claktory.app.dev.share",
            "content" to "Com.Claktory.App.Dev.Camera",
            "content" to "0@com.claktory.app.dev.camera",
            "content" to "evil@0@com.claktory.app.dev.share",
            "content" to "",
            "content" to null,
            "file" to "",
            "file" to null,
            "https" to "claktory.com",
            null to "com.android.externalstorage.documents",
        )) {
            assertFalse("$scheme://$authority", TextSaves.writable(scheme, authority, app))
        }
    }
}
