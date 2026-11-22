package com.claktory.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream

class AttachmentCopiesTest {
    @get:Rule
    val folder = TemporaryFolder()

    private val types = mapOf("image/jpeg" to "jpg", "application/pdf" to "pdf")

    private fun copies(maxSize: Long = AttachmentCopies.MAX_SIZE, spare: Long = Long.MAX_VALUE, now: Long = System.currentTimeMillis()) =
        AttachmentCopies(File(folder.root, AttachmentCopies.FOLDER), maxSize, { types[it] }, { spare }, { now })

    private fun source(name: String?, bytes: ByteArray, size: Long? = bytes.size.toLong(), type: String? = null) =
        AttachmentPicker.Source(name, size, type) { ByteArrayInputStream(bytes) }

    private fun refused(code: String, body: () -> Unit) {
        try {
            body()
            fail("expected $code")
        } catch (refused: AttachmentCopies.Refused) {
            assertEquals(code, refused.code)
        }
    }

    private fun pickFolders() = File(folder.root, AttachmentCopies.FOLDER).listFiles()?.toList().orEmpty()

    @Test
    fun `copies keep their order, names and bytes, in a folder of their own`() {
        val paths = copies().copyAll(
            listOf(source("b.txt", "two".toByteArray()), source("a.pdf", "one".toByteArray())),
        )
        assertEquals(listOf("b.txt", "a.pdf"), paths.map { File(it).name })
        assertEquals("two", File(paths[0]).readText())
        assertEquals(File(paths[0]).parentFile, File(paths[1]).parentFile)
        val again = copies().copyAll(listOf(source("b.txt", "three".toByteArray())))
        assertTrue("each pick its own folder", File(again[0]).parentFile != File(paths[0]).parentFile)
        assertEquals("two", File(paths[0]).readText())
    }

    @Test
    fun `nothing chosen copies nothing`() {
        assertEquals(emptyList<String>(), copies().copyAll(emptyList()))
        assertTrue(pickFolders().isEmpty())
    }

    @Test
    fun `names are made safe, and kept apart`() {
        val bytes = "x".toByteArray()
        val paths = copies().copyAll(
            listOf(
                source("../../etc/passwd", bytes),
                source("a\\b\\c.txt", bytes),
                source(".hidden", bytes),
                source("what?.txt", bytes),
                source("c.txt", bytes),
                source("C.TXT", bytes),
                source(null, bytes, type = "image/jpeg"),
                source("", bytes, type = "application/x-unknown"),
                source("x".repeat(300) + ".jpeg", bytes),
            ),
        )
        val names = paths.map { File(it).name }
        assertEquals("passwd", names[0])
        assertEquals("c.txt", names[1])
        assertEquals("hidden", names[2])
        assertEquals("what_.txt", names[3])
        assertEquals("c 2.txt", names[4])
        assertEquals("C 3.TXT", names[5])
        assertEquals("Attachment 7.jpg", names[6])
        assertEquals("Attachment 8", names[7])
        assertEquals(120, names[8].length)
        assertTrue(names[8].endsWith(".jpeg"))
        // All in the pick's folder, none outside it.
        val pick = File(paths[0]).parentFile!!
        assertTrue(paths.all { File(it).parentFile == pick })
    }

    @Test
    fun `a file said to be too large is refused before anything is copied`() {
        refused("too_large") {
            copies(maxSize = 10).copyAll(listOf(source("ok", ByteArray(5)), source("big", ByteArray(5), size = 11)))
        }
        assertTrue(pickFolders().isEmpty())
    }

    @Test
    fun `a file larger than it said is refused, and nothing of the pick is left`() {
        refused("too_large") {
            copies(maxSize = 10).copyAll(listOf(source("ok", ByteArray(5)), source("liar", ByteArray(11), size = 3)))
        }
        refused("too_large") {
            copies(maxSize = 10).copyAll(listOf(source("unsaid", ByteArray(11), size = null)))
        }
        assertTrue(pickFolders().isEmpty())
    }

    @Test
    fun `no room for the copies is refused`() {
        refused("no_space") {
            copies(spare = AttachmentCopies.MARGIN + 9).copyAll(listOf(source("ten", ByteArray(10))))
        }
        assertEquals(1, copies(spare = AttachmentCopies.MARGIN + 10).copyAll(listOf(source("ten", ByteArray(10)))).size)
    }

    @Test
    fun `a file that can't be read fails the pick, and nothing of it is left`() {
        val broken = AttachmentPicker.Source("broken", 4, null) {
            object : InputStream() {
                override fun read(): Int = throw IOException("gone")
            }
        }
        refused("copy_failed") { copies().copyAll(listOf(source("ok", ByteArray(3)), broken)) }
        assertTrue(pickFolders().isEmpty())
    }

    @Test
    fun `picks over a day old are removed, newer ones kept`() {
        val now = System.currentTimeMillis()
        val old = copies().copyAll(listOf(source("old", ByteArray(1))))
        File(old[0]).parentFile!!.setLastModified(now - 25L * 3_600_000)
        val recent = copies().copyAll(listOf(source("recent", ByteArray(1))))
        copies(now = now).prune()
        assertFalse(File(old[0]).exists())
        assertTrue(File(recent[0]).exists())
        assertArrayEquals(ByteArray(1), File(recent[0]).readBytes())
    }
}
