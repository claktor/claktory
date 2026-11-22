package com.claktory.app

import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.graphics.Bitmap
import android.graphics.Color
import android.os.Build
import android.os.Bundle
import android.provider.MediaStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream
import java.security.MessageDigest

/**
 * Not a test: a fixture for the end-to-end paste check (Doppio's), which
 * puts a known image on the phone's clipboard through the OS's own APIs,
 * for the person's Paste image to find. Skipped unless asked for with a
 * nonce:
 *
 *     adb shell am instrument -w -e class com.claktory.app.ClipboardImageFixture \
 *       -e nonce <nonce> com.claktory.app.dev.test/androidx.test.runner.AndroidJUnitRunner
 *
 * It draws a 96×96 PNG whose pixels follow from the nonce, saves it to
 * MediaStore's Pictures as claktory-paste-<nonce>.png, and puts its
 * content URI on the clipboard as an image (ClipData.newUri, so the clip's
 * description says image/png). The status lines (INSTRUMENTATION_STATUS)
 * give the URI, the byte count and the SHA-256, to check against what
 * reaches the backend. `-e clear true` instead removes every earlier
 * fixture image and clears the clipboard.
 *
 * The app's screen opens for a moment while it does, since Android lets
 * only the app in front use the clipboard. Instrumentation runs in the
 * app's own process, so the image is the app's
 * own and needs no grant: this checks the clipboard → Paste image → copy →
 * upload path with the OS's real clipboard, not another app's grant. An
 * image copied in another app (Chrome, Photos) is the phone check.
 */
@RunWith(AndroidJUnit4::class)
class ClipboardImageFixture {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val arguments: Bundle = InstrumentationRegistry.getArguments()

    private val clipboard get() = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager

    @Test
    fun putsAKnownImageOnTheClipboard() {
        val nonce = arguments.getString("nonce")
        assumeTrue("only when asked for, with -e nonce", nonce != null)
        assertTrue("a nonce of letters, digits, - and _", nonce!!.matches(Regex("[A-Za-z0-9_-]{1,64}")))
        val png = picture(nonce)
        val values = ContentValues().apply {
            put(MediaStore.Images.Media.DISPLAY_NAME, "$PREFIX$nonce.png")
            put(MediaStore.Images.Media.MIME_TYPE, "image/png")
        }
        val uri = context.contentResolver.insert(MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values)!!
        context.contentResolver.openOutputStream(uri)!!.use { it.write(png) }
        // Android lets only the app in front read the clipboard: the app's
        // screen is up while the clip is put and checked.
        var described = false
        Screen.launch().use { screen ->
            instrumentation.waitForIdleSync()
            screen.onActivity {
                clipboard.setPrimaryClip(ClipData.newUri(context.contentResolver, "Claktory paste fixture", uri))
            }
            waitForFocus(screen)
            screen.onActivity { described = clipboard.primaryClipDescription?.hasMimeType("image/*") == true }
        }
        assertTrue("the clipboard says it holds an image", described)
        val read = context.contentResolver.openInputStream(uri)!!.use { it.readBytes() }
        assertEquals(sha256(png), sha256(read))
        instrumentation.sendStatus(
            0,
            Bundle().apply {
                putString("fixture_uri", uri.toString())
                putString("fixture_name", "$PREFIX$nonce.png")
                putLong("fixture_bytes", png.size.toLong())
                putString("fixture_sha256", sha256(png))
            },
        )
    }

    @Test
    fun clearsEarlierFixtures() {
        assumeTrue("only when asked for, with -e clear true", arguments.getString("clear") == "true")
        context.contentResolver.delete(
            MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
            "${MediaStore.Images.Media.DISPLAY_NAME} LIKE ?",
            arrayOf("$PREFIX%"),
        )
        Screen.launch().use { screen ->
            screen.onActivity {
                // Android 9 can clear it; before, an empty text replaces it.
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) clipboard.clearPrimaryClip()
                else clipboard.setPrimaryClip(ClipData.newPlainText("", ""))
            }
        }
    }

    private fun waitForFocus(screen: Screen) {
        val until = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < until) {
            var focused = false
            screen.onActivity { focused = it.hasWindowFocus() }
            if (focused) return
            Thread.sleep(50)
        }
    }

    /** A 96×96 PNG whose pixels follow from [nonce], so each run's bytes differ. */
    private fun picture(nonce: String): ByteArray {
        val seed = MessageDigest.getInstance("SHA-256").digest(nonce.toByteArray())
        val bitmap = Bitmap.createBitmap(96, 96, Bitmap.Config.ARGB_8888)
        for (y in 0 until 96) {
            for (x in 0 until 96) {
                val i = ((y / 12) * 8 + x / 12) % seed.size
                val v = seed[i].toInt() and 0xFF
                bitmap.setPixel(x, y, Color.rgb(v, (v * 7 + x) and 0xFF, (v * 13 + y) and 0xFF))
            }
        }
        return ByteArrayOutputStream().use { out ->
            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
            bitmap.recycle()
            out.toByteArray()
        }
    }

    private fun sha256(bytes: ByteArray) =
        MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }

    companion object {
        const val PREFIX = "claktory-paste-"
    }
}
