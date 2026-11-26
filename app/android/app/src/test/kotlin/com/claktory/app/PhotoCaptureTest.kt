package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.text.SimpleDateFormat
import java.util.Locale

/** Take photo's pure parts: the photo's name and the sweep of files left behind. */
class PhotoCaptureTest {
    @get:Rule
    val folder = TemporaryFolder()

    @Test
    fun `a photo is named for when it was taken, as a Mac names a screenshot`() {
        val taken = SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.US).parse("2026-05-11 14:32:05")!!
        assertEquals("Photo 2026-05-11 at 14.32.05.jpg", PhotoCapture.photoName(taken))
    }

    @Test
    fun `camera files over a day old are swept, newer ones kept`() {
        val now = System.currentTimeMillis()
        val old = folder.newFile("old.jpg").apply { setLastModified(now - 25L * 3_600_000) }
        val recent = folder.newFile("recent.jpg").apply { setLastModified(now - 3_600_000) }
        PhotoCapture.sweep(folder.root, now)
        assertFalse(old.exists())
        assertTrue(recent.exists())
    }

    @Test
    fun `every tag of the GPS directory is a location tag`() {
        val tags = PhotoCapture.LOCATION_TAGS
        assertTrue(tags.all { it.startsWith("GPS") })
        assertEquals(tags.size, tags.toSet().size)
        assertTrue(listOf("GPSLatitude", "GPSLongitude", "GPSAltitude", "GPSDestLatitude").all { it in tags })
    }
}
