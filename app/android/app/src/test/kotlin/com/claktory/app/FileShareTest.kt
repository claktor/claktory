package com.claktory.app

import android.annotation.SuppressLint
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.io.File
import java.nio.file.Files

/** On the JVM, so java.nio's files and links are there whatever the app's minSdk. */
@SuppressLint("NewApi")
class FileShareTest {
    private val base = Files.createTempDirectory("fileshare").toFile()
    private val root = File(base, FileShare.FOLDER).apply { mkdirs() }
    private val guess = { extension: String -> mapOf("png" to "image/png", "pdf" to "application/pdf", "jpg" to "image/jpeg")[extension] }

    @After
    fun tearDown() {
        base.deleteRecursively()
    }

    private fun written(name: String, folder: File = File(root, "a1")): File =
        File(folder, name).apply { parentFile!!.mkdirs(); writeText("x") }

    @Test
    fun oneFileGoesWithItsOwnType() {
        val file = written("Report.pdf")
        val plan = FileShare.plan(listOf(file.path), listOf("application/pdf"), root, guess)!!
        assertEquals(listOf(FileShare.Item(file.canonicalFile, "application/pdf")), plan.items)
        assertEquals("application/pdf", plan.type)
    }

    @Test
    fun anUnknownTypeIsGuessedFromTheNameElseAnyFile() {
        val png = written("shot.PNG")
        val odd = written("notes.zzz")
        val bare = written("README")
        val plan = FileShare.plan(listOf(png.path, odd.path, bare.path), listOf("", "", "not a type"), root, guess)!!
        assertEquals(listOf("image/png", "*/*", "*/*"), plan.items.map { it.mime })
        assertEquals("*/*", plan.type)
    }

    @Test
    fun severalFilesTakeTheTypeTheyShare() {
        val a = written("a.png")
        val b = written("b.jpg")
        assertEquals("image/*", FileShare.plan(listOf(a.path, b.path), listOf("image/png", "image/jpeg"), root, guess)!!.type)
        assertEquals("image/png", FileShare.plan(listOf(a.path, a.path), listOf("image/png", "image/png"), root, guess)!!.type)
        val c = written("c.pdf")
        assertEquals("*/*", FileShare.plan(listOf(a.path, c.path), listOf("image/png", "application/pdf"), root, guess)!!.type)
    }

    @Test
    fun onlyTheAppsOwnShareFilesGo() {
        val inside = written("ok.png")
        val outside = written("secret.db", base)
        val sibling = written("x.png", File(base, "claktory-share-other"))
        // Each refuses the whole share.
        for (path in listOf(
            outside.path,
            sibling.path,
            "${root.path}/a1/../../secret.db",
            root.path,
            File(root, "a1").path,
            "${root.path}/a1/missing.png",
            "a1/ok.png",
        )) {
            assertNull(path, FileShare.plan(listOf(inside.path, path), listOf("", ""), root, guess))
        }
    }

    @Test
    fun aLinkOutOfTheFolderIsRefused() {
        val outside = written("secret.db", base)
        val link = File(File(root, "a1").apply { mkdirs() }, "link.png")
        Files.createSymbolicLink(link.toPath(), outside.toPath())
        assertNull(FileShare.plan(listOf(link.path), listOf("image/png"), root, guess))
    }

    @Test
    fun malformedCallsAreRefused() {
        val file = written("a.png")
        assertNull(FileShare.plan(null, listOf("image/png"), root, guess))
        assertNull(FileShare.plan(emptyList<String>(), emptyList<String>(), root, guess))
        assertNull(FileShare.plan(listOf(file.path), null, root, guess))
        assertNull(FileShare.plan(listOf(file.path), listOf("image/png", "image/png"), root, guess))
        assertNull(FileShare.plan(listOf(42), listOf("image/png"), root, guess))
        val many = (0..FileShare.MAX_FILES).map { written("f$it.png").path }
        assertNull(FileShare.plan(many, many.map { "image/png" }, root, guess))
        assertEquals(FileShare.MAX_FILES, FileShare.plan(many.drop(1), many.drop(1).map { "image/png" }, root, guess)!!.items.size)
    }

    @Test
    fun aShareSweepsFoldersOlderThanADayButNotItsOwn() {
        val now = System.currentTimeMillis()
        val old = written("old.png", File(root, "old")).parentFile!!
        val fresh = written("fresh.png", File(root, "fresh")).parentFile!!
        // As the app lays a share out: <share>/<index>/<name>.
        val mineFile = written("mine.png", File(root, "mine/0"))
        val mine = File(root, "mine")
        val outside = written("keep.db", base)
        for (folder in listOf(old, mine)) folder.setLastModified(now - FileShare.KEEP_MS - 60_000)
        assertEquals(1, FileShare.sweep(root, now, keep = setOf(mineFile)))
        assertEquals(false, old.exists())
        assertEquals(true, fresh.exists())
        assertEquals(true, mineFile.exists())
        assertEquals(true, outside.exists())
        assertEquals(0, FileShare.sweep(File(base, "none"), now))
    }
}
