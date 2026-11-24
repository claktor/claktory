package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.InputStream

class ShareIntakeTest {
    private val own = "com.claktory.app"
    private val intake = ShareIntake(
        providerOf = { authority ->
            when (authority) {
                "media" -> "com.android.providers.media"
                "com.example.files" -> "com.example"
                "com.claktory.app.files" -> own
                else -> null
            }
        },
        ownPackage = own,
    )

    @Test
    fun onlyAnotherAppsContentIsRead() {
        assertTrue(intake.readable("content://media/external/images/media/12"))
        assertTrue(intake.readable("CONTENT://com.example.files/a.pdf"))
        // Another app could point at this app's own files.
        assertFalse(intake.readable("file:///data/data/com.claktory.app/no_backup/dm-keys/x.key"))
        assertFalse(intake.readable("file:///sdcard/Download/a.png"))
        assertFalse(intake.readable("https://example.com/a.png"))
        assertFalse(intake.readable("content:media/1"))
        // This app's own provider, one that doesn't resolve, a user or a port.
        assertFalse(intake.readable("content://com.claktory.app.files/x"))
        assertFalse(intake.readable("content://nobody.home/x"))
        assertFalse(intake.readable("content://someone@media/x"))
        assertFalse(intake.readable("content://media:80/x"))
        assertFalse(intake.readable("content:///x"))
    }

    @Test
    fun textAndSubjectAreCapped() {
        val long = "a".repeat(ShareIntake.MAX_TEXT + 5)
        val plan = intake.plan(long, "s".repeat(ShareIntake.MAX_SUBJECT + 1), emptyList())!!
        assertEquals(ShareIntake.MAX_TEXT, plan.text!!.length)
        assertTrue(plan.textCut)
        assertEquals(ShareIntake.MAX_SUBJECT, plan.subject!!.length)
        assertNull(plan.error)
        // A character is never cut in two.
        val emoji = "a".repeat(ShareIntake.MAX_TEXT - 1) + "😀" + "b"
        val cut = intake.plan(emoji, null, emptyList())!!
        assertEquals(ShareIntake.MAX_TEXT - 1, cut.text!!.length)
        assertTrue(cut.textCut)
        val short = intake.plan("a link", "A page", emptyList())!!
        assertEquals("a link", short.text)
        assertFalse(short.textCut)
        assertEquals("A page", short.subject)
    }

    @Test
    fun filesAreFilteredAndCounted() {
        val mixed = intake.plan(null, null, listOf("content://media/1", "file:///sdcard/a.png", "content://com.example.files/b"))!!
        assertEquals(listOf("content://media/1", "content://com.example.files/b"), mixed.uris)
        assertEquals(ShareIntake.REFUSED, mixed.error)
        val many = intake.plan("with text", null, List(ShareIntake.MAX_FILES + 1) { "content://media/$it" })!!
        assertTrue(many.uris.isEmpty())
        assertEquals(ShareIntake.TOO_MANY, many.error)
        assertEquals("with text", many.text)
        assertEquals(ShareIntake.MAX_FILES, intake.plan(null, null, List(ShareIntake.MAX_FILES) { "content://media/$it" })!!.uris.size)
    }

    @Test
    fun anEmptyShareIsDroppedButAFailedOneComes() {
        assertNull(intake.plan(null, null, emptyList()))
        assertNull(intake.plan("", "", emptyList()))
        // Every file refused: it still comes, with its error, so the app can say so.
        val refused = intake.plan(null, null, listOf("file:///sdcard/a.png"))!!
        assertNull(refused.text)
        assertTrue(refused.uris.isEmpty())
        assertEquals(ShareIntake.REFUSED, refused.error)
    }

    @Test
    fun aStallClosesWhatsRead() {
        var now = 0L
        val guard = StallGuard(30_000) { now }
        var closed = false
        val input = guard.guarded(object : InputStream() {
            override fun read(): Int = 1

            override fun close() {
                closed = true
            }
        })
        now = 29_000
        input.read()
        now = 58_000
        assertFalse(guard.check())
        now = 59_001
        assertTrue(guard.check())
        assertTrue(closed)
        assertTrue(guard.stalled)
    }

    @Test
    fun namesLoseInvisibleCharacters() {
        fun named(name: String) =
            AttachmentCopies.fileName(AttachmentPicker.Source(name, 1, null) { ByteArrayInputStream(ByteArray(0)) }, 0) { null }
        // A right-to-left override makes "invoice\u202Efdp.exe" read as "invoiceexe.pdf".
        assertEquals("invoicefdp.exe", named("invoice\u202Efdp.exe"))
        assertEquals("ab.png", named("a\u200Bb\u2066.png"))
        assertEquals("tab.png", named("t\u0009ab.png"))
        assertEquals("x.pdf", named("\uFEFF.x.pdf"))
    }
}
