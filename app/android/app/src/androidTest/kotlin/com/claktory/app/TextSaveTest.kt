package com.claktory.app

import android.annotation.SuppressLint
import android.app.Activity
import android.app.Instrumentation
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * saveText on a phone: the save dialog is asked for a text file with the
 * name suggested, and the text goes, as UTF-8, into the document it made.
 * The dialog is stood in for by an activity monitor that answers with a
 * document of the phone's Downloads (MediaStore's), as the dialog answers
 * with the one the person made; the real dialog is checked by hand.
 */
@RunWith(AndroidJUnit4::class)
@SuppressLint("NewApi") // setUp skips before Android 10
class TextSaveTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val made = mutableListOf<Uri>()

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

        fun await(): Answer = apply { assertTrue("no answer", done.await(10, TimeUnit.SECONDS)) }
    }

    @Before
    fun setUp() {
        assumeTrue("MediaStore's Downloads (Android 10+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q)
    }

    @After
    fun tearDown() {
        made.forEach { runCatching { context.contentResolver.delete(it, null, null) } }
    }

    /** A document the way the save dialog makes one: there, and empty. */
    private fun document(): Uri {
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, "claktory-save-test-${System.nanoTime()}.txt")
            put(MediaStore.Downloads.MIME_TYPE, "text/plain")
        }
        return context.contentResolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)!!.also { made += it }
    }

    private fun read(uri: Uri): String? = runCatching {
        context.contentResolver.openInputStream(uri)?.use { it.readBytes().toString(Charsets.UTF_8) }
    }.getOrNull()

    private fun exists(uri: Uri): Boolean =
        context.contentResolver.query(uri, null, null, null, null)?.use { it.count > 0 } ?: false

    /** Stands in for the save dialog: [answer] sees what it was asked and says how it returns. */
    private fun <T> dialog(answer: (Intent) -> Instrumentation.ActivityResult, body: () -> T): T {
        val monitor = object : Instrumentation.ActivityMonitor() {
            override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? =
                if (intent.action == Intent.ACTION_CREATE_DOCUMENT) answer(intent) else null
        }
        instrumentation.addMonitor(monitor)
        try {
            return body()
        } finally {
            instrumentation.removeMonitor(monitor)
        }
    }

    private fun saves(screen: Screen): TextSave {
        var found: TextSave? = null
        screen.onActivity { found = it.adapter(TextSave::class.java) }
        return found ?: throw AssertionError("no TextSave adapter")
    }

    private fun save(screen: Screen, arguments: Any?): Answer {
        val answer = Answer()
        val saves = saves(screen)
        screen.onActivity { saves.save(arguments, answer) }
        return answer.await()
    }

    private fun ok(uri: Uri) = Instrumentation.ActivityResult(Activity.RESULT_OK, Intent().setData(uri))

    @Test
    fun theTextGoesIntoTheDocumentAsUtf8() {
        val target = document()
        val key = "café 🔑 ABCD-EFGH-IJKL\n"
        var asked: Intent? = null
        Screen.launch().use { screen ->
            val answer = dialog({ asked = it; ok(target) }) {
                save(screen, mapOf("name" to "Claktory recovery key (Studio).txt", "text" to key))
            }
            assertEquals(true, answer.value)
            assertNull(answer.code)
        }
        assertEquals("text/plain", asked!!.type)
        assertEquals("Claktory recovery key (Studio).txt", asked!!.getStringExtra(Intent.EXTRA_TITLE))
        assertTrue(asked!!.hasCategory(Intent.CATEGORY_OPENABLE))
        assertEquals(key, read(target))
    }

    @Test
    fun cancelledAnswersFalse() {
        Screen.launch().use { screen ->
            val answer = dialog({ Instrumentation.ActivityResult(Activity.RESULT_CANCELED, null) }) {
                save(screen, mapOf("name" to "key.txt", "text" to "secret"))
            }
            assertEquals(false, answer.value)
            assertNull(answer.code)
        }
    }

    @Test
    fun aSecondSaveWhileTheDialogIsOpenAnswersFalseAndTheFirstStillSaves() {
        val target = document()
        Screen.launch().use { screen ->
            val saves = saves(screen)
            val first = Answer()
            val second = Answer()
            dialog({ ok(target) }) {
                screen.onActivity {
                    saves.save(mapOf("name" to "a.txt", "text" to "first"), first)
                    saves.save(mapOf("name" to "b.txt", "text" to "second"), second)
                }
                assertEquals(false, second.await().value)
                assertEquals(true, first.await().value)
            }
        }
        assertEquals("first", read(target))
    }

    @Test
    fun aPlaceOfTheAppsOwnIsRefusedAndNothingWritten() {
        val inside = File(context.cacheDir, PhotoCapture.FOLDER).apply { mkdirs() }
        val file = File(inside, "taken.txt").apply { delete() }
        val own = Uri.parse("content://${CameraFiles.authority(context)}/camera/taken.txt")
        Screen.launch().use { screen ->
            val answer = dialog({ ok(own) }) { save(screen, mapOf("name" to "key.txt", "text" to "secret")) }
            assertEquals(TextSave.FAILED, answer.code)
        }
        assertFalse(file.exists())
    }

    @Test
    fun aDocumentThatCantBeWrittenFails() {
        val gone = document()
        context.contentResolver.delete(gone, null, null)
        Screen.launch().use { screen ->
            val answer = dialog({ ok(gone) }) { save(screen, mapOf("name" to "key.txt", "text" to "secret")) }
            assertEquals(TextSave.FAILED, answer.code)
        }
    }

    @Test
    fun withoutANameOrATextItFailsBeforeAnyDialog() {
        var opened = false
        Screen.launch().use { screen ->
            dialog({ opened = true; Instrumentation.ActivityResult(Activity.RESULT_CANCELED, null) }) {
                assertEquals(TextSave.FAILED, save(screen, mapOf("text" to "secret")).code)
                assertEquals(TextSave.FAILED, save(screen, mapOf("name" to " ", "text" to "secret")).code)
                assertEquals(TextSave.FAILED, save(screen, mapOf("name" to "key.txt")).code)
                assertEquals(TextSave.FAILED, save(screen, null).code)
            }
        }
        assertFalse(opened)
    }

    @Test
    fun aDocumentMadeAfterTheTextWasLostIsDeleted() {
        val target = document()
        Screen.launch().use { screen ->
            // A new adapter, as after Android ended the process while the dialog was open.
            val fresh = TextSave(screen.activity)
            var handled = false
            screen.onActivity { handled = fresh.onActivityResult(TextSave.REQUEST, Activity.RESULT_OK, Intent().setData(target)) }
            assertTrue(handled)
            val until = System.currentTimeMillis() + 5_000
            while (exists(target) && System.currentTimeMillis() < until) Thread.sleep(50)
            fresh.close()
        }
        assertFalse("the empty document is left", exists(target))
    }
}
