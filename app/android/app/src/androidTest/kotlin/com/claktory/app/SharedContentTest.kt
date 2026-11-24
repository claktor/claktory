package com.claktory.app

import android.app.Activity
import android.app.PendingIntent
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.Collections

/**
 * Sharing to Claktory from another app, on a phone or emulator: what the
 * system offers it, a share that launches the app and one while it runs
 * (each reaching the app's one screen), the files copied from another
 * package's provider, and what's refused: anything but another app's
 * content, extras that aren't the share's, and a provider that hangs.
 */
@RunWith(AndroidJUnit4::class)
class SharedContentTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val heard: MutableList<Map<String, Any?>> = Collections.synchronizedList(mutableListOf())

    private val shareActivity = android.content.ComponentName(context, ShareActivity::class.java)

    @After
    fun tearDown() {
        SharedChannel.heard = null
        SharedChannel.stallMs = 30_000
        SharedChannel.holdCopyMs = 0
        mainScreens().forEach { screen -> instrumentation.runOnMainSync { screen.finish() } }
        // Gone before the next test, so its share can't reach a screen on its way out.
        waitFor("the screens gone") { mainScreens().isEmpty() }
    }

    private fun waitFor(what: String, seconds: Int = 15, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
    }

    private fun mainScreens(): List<Activity> {
        var screens = emptyList<Activity>()
        instrumentation.runOnMainSync {
            screens = Stage.values().filter { it != Stage.DESTROYED }
                .flatMap { ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(it) }
                .filterIsInstance<MainActivity>()
                .distinct()
        }
        return screens
    }

    private fun handlers(intent: Intent) =
        context.packageManager.queryIntentActivities(intent, PackageManager.MATCH_ALL)
            .filter { it.activityInfo.packageName == context.packageName }
            .map { it.activityInfo.name }

    /** A share as another app sends it: to whatever takes it, here ShareActivity. */
    private fun share(intent: Intent) {
        context.startActivity(intent.setClass(context, ShareActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
    }

    @Suppress("UNCHECKED_CAST")
    private fun files(share: Map<String, Any?>) = share["files"] as List<String>

    @Test
    fun theSystemOffersItForTextImagesAndFiles() {
        // On as the manifest ships it, not switched on by a test.
        assertEquals(
            PackageManager.COMPONENT_ENABLED_STATE_DEFAULT,
            context.packageManager.getComponentEnabledSetting(shareActivity),
        )
        val share = listOf(ShareActivity::class.java.name)
        assertEquals(share, handlers(Intent(Intent.ACTION_SEND).setType("text/plain")))
        assertEquals(share, handlers(Intent(Intent.ACTION_SEND).setType("image/png")))
        assertEquals(share, handlers(Intent(Intent.ACTION_SEND).setType("application/pdf")))
        assertEquals(share, handlers(Intent(Intent.ACTION_SEND_MULTIPLE).setType("image/*")))
    }

    @Test
    fun aShareThatLaunchesTheAppComesWithItsCopy() {
        SharedChannel.heard = { heard += it }
        share(
            Intent(Intent.ACTION_SEND).setType("image/png")
                .putExtra(Intent.EXTRA_TEXT, "Look at this")
                .putExtra(Intent.EXTRA_SUBJECT, "A screenshot")
                .putExtra(Intent.EXTRA_STREAM, ShareTestProvider.uri("picture.png")),
        )
        waitFor("the share") { heard.isNotEmpty() }
        val got = heard.single()
        assertEquals("Look at this", got["text"])
        assertEquals(false, got["text_cut"])
        assertEquals("A screenshot", got["subject"])
        assertNull(got["error"])
        assertTrue((got["id"] as Int) > 0)
        val copy = File(files(got).single())
        assertEquals("picture.png", copy.name)
        assertTrue(copy.path.startsWith(File(context.cacheDir, AttachmentCopies.FOLDER).path))
        assertArrayEquals(ShareTestProvider.BYTES, copy.readBytes())
        waitFor("one screen") { mainScreens().size == 1 }
    }

    @Test
    fun aShareWhileItRunsReachesTheSameScreen() {
        SharedChannel.heard = { heard += it }
        Screen.launch().use { screen ->
            var first: Activity? = null
            screen.onActivity { first = it }
            share(
                Intent(Intent.ACTION_SEND_MULTIPLE).setType("*/*")
                    .putParcelableArrayListExtra(
                        Intent.EXTRA_STREAM,
                        arrayListOf(
                            ShareTestProvider.uri("picture.png"),
                            ShareTestProvider.uri("notes.txt"),
                            // This app's own files: never.
                            Uri.fromFile(File(context.filesDir, "anything")),
                            // This app's own provider: never.
                            Uri.parse("content://${context.packageName}.androidx-startup/x"),
                        ),
                    ),
            )
            waitFor("the share") { heard.isNotEmpty() }
            val got = heard.single()
            assertEquals(listOf("picture.png", "notes.txt"), files(got).map { File(it).name })
            assertEquals(ShareIntake.REFUSED, got["error"])
            waitFor("the screen back") { mainScreens().size == 1 }
            assertTrue("the same screen, not a second", mainScreens().single() === first)
            // Discarded: its copies go.
            val folder = File(files(got).first()).parentFile!!
            screen.onActivity {
                it.adapter(SharedChannel::class.java)!!.onMethodCall(
                    io.flutter.plugin.common.MethodCall("discard", mapOf("id" to got["id"])),
                    object : io.flutter.plugin.common.MethodChannel.Result {
                        override fun success(result: Any?) {}
                        override fun error(code: String, message: String?, details: Any?) {}
                        override fun notImplemented() {}
                    },
                )
            }
            waitFor("the copies gone") { !folder.exists() }
        }
    }

    @Test
    fun anotherAppsGrantReachesTheAppsOneScreen() {
        // Another package shares a file of its provider that isn't exported: only the grant reads it,
        // and ShareActivity must pass that grant on to the app's screen: its own ends with it, so the
        // copy waits until it's gone.
        SharedChannel.holdCopyMs = 3_000
        SharedChannel.heard = { heard += it }
        val target = "${context.packageName}/${ShareActivity::class.java.name}"
        instrumentation.shell("am start -n ${instrumentation.context.packageName}/com.claktory.app.ShareSender --es target $target")
        waitFor("the granted share") { heard.isNotEmpty() }
        val got = heard.single()
        assertNull(got["error"])
        assertEquals("granted", got["text"])
        val copy = File(files(got).single())
        assertEquals("granted.png", copy.name)
        assertArrayEquals(ShareTestProvider.BYTES, copy.readBytes())
        // Without the grant, this app can't read it.
        val denied = runCatching { context.contentResolver.openInputStream(Uri.parse("content://${ShareSender.PRIVATE_AUTHORITY}/other.png"))?.close() }
        assertTrue("read without a grant", denied.isFailure)
        waitFor("one screen") { mainScreens().size == 1 }
    }

    @Test
    fun onlyTheShareIsForwardedAndOnlyToRead() {
        val incoming = Intent(Intent.ACTION_SEND).setType("text/plain")
            .putExtra(Intent.EXTRA_TEXT, "hello")
            .putExtra(Intent.EXTRA_STREAM, ShareTestProvider.uri("notes.txt"))
            // What a hostile app might add: none of it may reach the app.
            .putExtra("route", "/somewhere")
            .putExtra("nested", Intent(Intent.ACTION_VIEW, Uri.parse("https://example.com")))
            .putExtra("pending", PendingIntent.getActivity(context, 0, Intent(), PendingIntent.FLAG_IMMUTABLE))
            .putExtra("bundle", Bundle().apply { putString("dart-flags", "--observe") })
            .addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or Intent.FLAG_GRANT_PREFIX_URI_PERMISSION,
            )
        val out = ShareActivity.forward(context, incoming)!!
        assertEquals(MainActivity::class.java.name, out.component!!.className)
        assertEquals(context.packageName, out.component!!.packageName)
        assertEquals(
            setOf(Intent.EXTRA_TEXT, Intent.EXTRA_STREAM, ShareActivity.EXTRA_TOKEN),
            out.extras!!.keySet(),
        )
        assertTrue(out.flags and Intent.FLAG_GRANT_READ_URI_PERMISSION != 0)
        assertEquals(0, out.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        assertEquals(0, out.flags and Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION)
        assertEquals(0, out.flags and Intent.FLAG_GRANT_PREFIX_URI_PERMISSION)
        assertEquals(ShareTestProvider.uri("notes.txt"), out.clipData!!.getItemAt(0).uri)
    }

    @Test
    fun aStreamThatIsntAnAddressIsDropped() {
        // Another parcelable where the file's address should be.
        val odd = Intent(Intent.ACTION_SEND).setType("image/png")
            .putExtra(Intent.EXTRA_STREAM, Intent(Intent.ACTION_VIEW))
        assertTrue(SharedChannel.Shared.read(odd)!!.uris.isEmpty())
        assertNull("nothing left to share", ShareActivity.forward(context, odd)!!.clipData)
        val many = Intent(Intent.ACTION_SEND_MULTIPLE).setType("*/*")
            .putParcelableArrayListExtra(Intent.EXTRA_STREAM, arrayListOf(Bundle(), ShareTestProvider.uri("notes.txt")))
        // The address may come or (Android 13's typed getter refusing the mixed list) not; the bundle never.
        val read = SharedChannel.Shared.read(many)!!.uris
        assertTrue(read.isEmpty() || read == listOf(ShareTestProvider.uri("notes.txt")))
        assertTrue(read.all { (it as Any) is Uri })
        // Nothing at all to share: no share reaches the app.
        SharedChannel.heard = { heard += it }
        Screen.launch().use {
            share(odd)
            share(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "after"))
            waitFor("the share after") { heard.isNotEmpty() }
            Thread.sleep(300)
            assertEquals(listOf("after"), heard.map { it["text"] })
        }
    }

    @Test
    fun aProviderThatHangsFailsItsShare() {
        SharedChannel.stallMs = 2_000
        SharedChannel.heard = { heard += it }
        Screen.launch().use {
            val started = System.currentTimeMillis()
            share(
                Intent(Intent.ACTION_SEND).setType("text/plain")
                    .putExtra(Intent.EXTRA_TEXT, "still comes")
                    .putExtra(Intent.EXTRA_STREAM, ShareTestProvider.uri("never")),
            )
            waitFor("the failed share", seconds = 20) { heard.isNotEmpty() }
            val got = heard.single()
            assertEquals(SharedChannel.COPY_FAILED, got["error"])
            assertTrue(files(got).isEmpty())
            assertEquals("still comes", got["text"])
            assertTrue("gave up within the stall and its grace", System.currentTimeMillis() - started < 15_000)
            // The next share isn't held up behind it.
            share(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, "next"))
            waitFor("the next share") { heard.size == 2 }
            assertFalse(heard[1]["text"] == null)
        }
    }

    @Test
    fun tooManyFilesTakesNone() {
        SharedChannel.heard = { heard += it }
        Screen.launch().use {
            share(
                Intent(Intent.ACTION_SEND_MULTIPLE).setType("*/*")
                    .putParcelableArrayListExtra(Intent.EXTRA_STREAM, ArrayList(List(ShareIntake.MAX_FILES + 1) { ShareTestProvider.uri("notes$it.txt") })),
            )
            waitFor("the share") { heard.isNotEmpty() }
            assertEquals(ShareIntake.TOO_MANY, heard.single()["error"])
            assertTrue(files(heard.single()).isEmpty())
        }
    }
}
