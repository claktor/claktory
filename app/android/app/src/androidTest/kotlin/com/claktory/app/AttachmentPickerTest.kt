package com.claktory.app

import android.app.Activity
import android.content.ClipData
import android.annotation.SuppressLint
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The picker's half after the system's picker answers, with real content
 * URIs (MediaStore's, as the photo and document pickers hand over): what
 * was chosen is copied into the cache under its own name, in order, and
 * the copies' paths answered. The system pickers themselves are the
 * phone's, checked by hand on a device.
 */
@RunWith(AndroidJUnit4::class)
class AttachmentPickerTest {
    private val context = InstrumentationRegistry.getInstrumentation().targetContext
    private val made = mutableListOf<Uri>()

    @After
    fun tearDown() {
        made.forEach { context.contentResolver.delete(it, null, null) }
    }

    /**
     * A file in Downloads, as another app would have put it there. MediaStore
     * has Downloads from Android 10; before, putting a file anywhere shared
     * takes a storage permission the app doesn't have, so these tests skip.
     */
    @SuppressLint("NewApi") // the assumption below skips before Android 10
    private fun download(name: String, bytes: ByteArray, type: String): Uri {
        assumeTrue("MediaStore's Downloads (Android 10+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q)
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, name)
            put(MediaStore.Downloads.MIME_TYPE, type)
        }
        val uri = context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)!!
        context.contentResolver.openOutputStream(uri)!!.use { it.write(bytes) }
        made += uri
        return uri
    }

    private class Answer : MethodChannel.Result {
        val done = CountDownLatch(1)
        var value: Any? = null
        var code: String? = null

        override fun success(result: Any?) {
            value = result
            done.countDown()
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            code = errorCode
            done.countDown()
        }

        override fun notImplemented() {
            code = "not_implemented"
            done.countDown()
        }
    }

    @Test
    fun aSourceReadsItsProvidersNameSizeAndBytes() {
        val bytes = "hello attachments".toByteArray()
        val uri = download("note-${System.nanoTime()}.txt", bytes, "text/plain")
        val source = AttachmentPicker.Source.of(context.contentResolver, uri)
        assertTrue(source.name!!.startsWith("note-"))
        assertEquals(bytes.size.toLong(), source.size)
        assertEquals("text/plain", source.mimeType)
        assertArrayEquals(bytes, source.open().use { it.readBytes() })
    }

    @Test
    fun whatThePickerChoseIsCopiedAndAnsweredInOrder() {
        val first = download("first-${System.nanoTime()}.txt", "one".toByteArray(), "text/plain")
        val second = download("second-${System.nanoTime()}.png", byteArrayOf(1, 2, 3), "image/png")
        val chosen = ClipData.newRawUri("", first).apply { addItem(ClipData.Item(second)) }
        val answer = Answer()
        val root = File(context.cacheDir, "attachments-test").apply { deleteRecursively() }
        Screen.launch().use { scenario ->
            scenario.onActivity { activity ->
                val picker = AttachmentPicker(activity, AttachmentCopies(root))
                // As if the person had picked these two, the menu then the system's picker.
                pickWithoutMenu(picker, answer)
                picker.onActivityResult(AttachmentPicker.REQUEST, Activity.RESULT_OK, Intent().apply { clipData = chosen })
            }
            assertTrue(answer.done.await(10, TimeUnit.SECONDS))
        }
        @Suppress("UNCHECKED_CAST")
        val paths = answer.value as List<String>
        assertEquals(2, paths.size)
        assertTrue(File(paths[0]).name.startsWith("first-"))
        assertEquals("one", File(paths[0]).readText())
        assertArrayEquals(byteArrayOf(1, 2, 3), File(paths[1]).readBytes())
        assertTrue(paths.all { it.startsWith(root.path) })
        root.deleteRecursively()
    }

    @Test
    fun aPickerClosedWithNothingChosenAnswersAnEmptyList() {
        val answer = Answer()
        Screen.launch().use { scenario ->
            scenario.onActivity { activity ->
                val picker = AttachmentPicker(activity)
                pickWithoutMenu(picker, answer)
                picker.onActivityResult(AttachmentPicker.REQUEST, Activity.RESULT_CANCELED, null)
            }
            assertTrue(answer.done.await(10, TimeUnit.SECONDS))
        }
        assertEquals(emptyList<String>(), answer.value)
    }

    @Test
    fun anotherActivitysResultIsntTheirs() {
        Screen.launch().use { scenario ->
            scenario.onActivity { activity ->
                assertEquals(false, AttachmentPicker(activity).onActivityResult(1, Activity.RESULT_OK, Intent()))
            }
        }
    }

    /** Opens the menu (as pick does) and takes it away, as the person choosing does. */
    private fun pickWithoutMenu(picker: AttachmentPicker, answer: Answer) {
        picker.pick(answer)
        picker.dismissMenuForTest()
    }
}
