package com.claktory.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** A clipboard as Android keeps it: a label, the time it took the clip (null before Android 8), the text. */
class FakeClipboard(private val timestamps: Boolean = true) : SecretClipboardPort {
    var label: String? = null
    var text: String? = null
    var timestamp = 0L
    var sensitiveWrites = 0
    var expiresAt: Long? = null
    var inFront = true
    var textReads = 0

    override fun write(label: String, text: String, expiresAt: Long?) {
        sensitiveWrites++
        put(label, text)
        this.expiresAt = expiresAt
    }

    /** Another app's copy, or the app's own through Flutter: no expiry. */
    fun put(label: String?, text: String?) {
        this.label = label
        this.text = text
        expiresAt = null
        timestamp++
    }

    override fun stamp(): ClipStamp? {
        if (!inFront || text == null) return null
        return ClipStamp(label, if (timestamps) timestamp else null, expiresAt)
    }

    override fun text(): String? {
        textReads++
        return text
    }

    override fun clear() {
        label = null
        text = null
    }
}

class SecretClipboardCoreTest {
    private val code = "claktory://add?code=SECRET-HANDOFF"

    @Test
    fun clearRemovesTheCodeWhileItsStillThere() {
        for (timestamps in listOf(true, false)) {
            val clipboard = FakeClipboard(timestamps)
            val core = SecretClipboardCore(clipboard)
            val token = core.copy(code)
            assertEquals(code, clipboard.text)
            assertEquals(SecretClipboardCore.LABEL, clipboard.label)
            assertEquals(1, clipboard.sensitiveWrites)
            assertTrue(core.clear(token))
            assertNull(clipboard.text)
            // A token clears once.
            clipboard.put(SecretClipboardCore.LABEL, code)
            assertFalse(core.clear(token))
            assertEquals(code, clipboard.text)
        }
    }

    @Test
    fun whatThePersonCopiedSinceIsLeftAlone() {
        for (timestamps in listOf(true, false)) {
            val clipboard = FakeClipboard(timestamps)
            val core = SecretClipboardCore(clipboard)
            val token = core.copy(code)
            clipboard.put("text label?", "a message they copied")
            assertFalse(core.clear(token))
            assertEquals("a message they copied", clipboard.text)
            // Another app's clip is never read.
            assertEquals(0, clipboard.textReads)
        }
    }

    @Test
    fun aLaterCopyIsNotClearedByAnEarlierToken() {
        val clipboard = FakeClipboard(timestamps = true)
        val core = SecretClipboardCore(clipboard)
        val first = core.copy(code)
        val second = core.copy("claktory://add?code=ANOTHER")
        assertNotEquals(first, second)
        assertFalse(core.clear(first))
        assertEquals("claktory://add?code=ANOTHER", clipboard.text)
        assertTrue(core.clear(second))
        assertNull(clipboard.text)
    }

    @Test
    fun beforeAndroid8TheSameCodeAgainCountsAsOurs() {
        // No time to tell two writes apart: the label and the text's digest.
        val clipboard = FakeClipboard(timestamps = false)
        val core = SecretClipboardCore(clipboard)
        val first = core.copy(code)
        core.copy("claktory://add?code=ANOTHER")
        assertFalse(core.clear(first))
        clipboard.put(SecretClipboardCore.LABEL, "something else under our label")
        assertFalse(core.clear(2))
        assertEquals("something else under our label", clipboard.text)
    }

    @Test
    fun whenAndroidWontSayWhatsThereNothingIsCleared() {
        val clipboard = FakeClipboard()
        val core = SecretClipboardCore(clipboard)
        val token = core.copy(code)
        clipboard.inFront = false
        assertFalse(core.clear(token))
        assertEquals(code, clipboard.text)
        // Back in front, the same token can still clear it.
        clipboard.inFront = true
        assertTrue(core.clear(token))
        assertNull(clipboard.text)
    }

    @Test
    fun anUnknownTokenDoesNothing() {
        val clipboard = FakeClipboard()
        val core = SecretClipboardCore(clipboard)
        core.copy(code)
        assertFalse(core.clear(99))
        assertEquals(code, clipboard.text)
    }

    @Test
    fun sweepClearsTheCodeOnlyOnceItHasExpired() {
        var now = 1_000_000L
        val clipboard = FakeClipboard()
        // A new core: the app was stopped in between, so no token is left.
        SecretClipboardCore(clipboard) { now }.copy(code, seconds = 600)
        assertEquals(now + 600_000, clipboard.expiresAt)
        val core = SecretClipboardCore(clipboard) { now }
        val expiresAt = now + 600_000
        now += 599_999
        // Not yet: it answers when, for a sweep then.
        assertEquals(expiresAt, core.sweep())
        assertEquals(code, clipboard.text)
        now += 1
        assertNull(core.sweep())
        assertNull(clipboard.text)
        // Only the description is read, never the text.
        assertEquals(0, clipboard.textReads)
    }

    @Test
    fun sweepLeavesOtherClipsAlone() {
        var now = 1_000_000L
        val clipboard = FakeClipboard()
        val core = SecretClipboardCore(clipboard) { now }
        // Copied with no expiry.
        core.copy(code)
        assertNull(clipboard.expiresAt)
        now += 10_000_000
        assertNull(core.sweep())
        assertEquals(code, clipboard.text)
        // What the person copied since, under another label.
        core.copy(code, seconds = 60)
        clipboard.put("text label?", "a message they copied")
        now += 10_000_000
        assertNull(core.sweep())
        assertEquals("a message they copied", clipboard.text)
        // Another label with an expiry, all the same.
        clipboard.expiresAt = now - 1
        assertNull(core.sweep())
        assertEquals("a message they copied", clipboard.text)
        // Android won't say: nothing is cleared.
        core.copy(code, seconds = 60)
        now += 120_000
        clipboard.inFront = false
        assertNull(core.sweep())
        assertEquals(code, clipboard.text)
        clipboard.inFront = true
        assertNull(core.sweep())
        assertNull(clipboard.text)
        assertEquals(0, clipboard.textReads)
    }

    @Test
    fun anExpiryFurtherAheadThanOursIsntOurs() {
        // Any app can label a clip "Claktory code": an expiry no copy of ours sets is left alone, and waits on nothing.
        val now = 1_000_000L
        val clipboard = FakeClipboard()
        val core = SecretClipboardCore(clipboard) { now }
        for (forged in listOf(Long.MAX_VALUE, now + SecretClipboardCore.MAX_AHEAD_MS + 1)) {
            clipboard.put(SecretClipboardCore.LABEL, "forged")
            clipboard.expiresAt = forged
            assertNull(core.sweep())
            assertEquals("forged", clipboard.text)
        }
        // Long.MIN_VALUE is long past: under our label, cleared, without overflowing.
        clipboard.expiresAt = Long.MIN_VALUE
        assertNull(core.sweep())
        assertNull(clipboard.text)
        // The longest of ours, at the limit, still counts.
        core.copy(code, seconds = 3600)
        assertEquals(now + SecretClipboardCore.MAX_AHEAD_MS, core.sweep())
        assertEquals(0, clipboard.textReads)
    }
}
