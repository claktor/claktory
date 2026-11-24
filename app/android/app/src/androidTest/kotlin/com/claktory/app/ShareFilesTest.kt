package com.claktory.app

import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.util.Collections
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Sharing files from Claktory to another app (#418), on a phone or
 * emulator: the files the app wrote in its share folder reach the app
 * picked, read-only, named as they are, and nothing else of the app's can
 * be read through the same provider. The other app is the test APK's
 * [ShareReceiver], its own package and user; it gets the very send the
 * chooser would hand on. The chooser itself opens over the app, closing
 * it answers false, and a refused share shows nothing.
 */
@RunWith(AndroidJUnit4::class)
class ShareFilesTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext
    private val root = File(context.cacheDir, FileShare.FOLDER)
    // As the app lays a share out: <share>/<index>/<name>.
    private val folder = File(root, "t${System.nanoTime()}/0")

    @Before
    fun setUp() {
        folder.mkdirs()
    }

    @After
    fun tearDown() {
        folder.parentFile!!.deleteRecursively()
        instrumentation.shell("input keyevent KEYCODE_BACK")
    }

    private class Answer : MethodChannel.Result {
        val values: MutableList<Any?> = Collections.synchronizedList(mutableListOf())

        override fun success(result: Any?) {
            values += result
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            values += "error $errorCode"
        }

        override fun notImplemented() {
            values += "notImplemented"
        }
    }

    private fun waitFor(what: String, seconds: Int = 10, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(100)
        }
        throw AssertionError("no $what within $seconds s")
    }

    private fun stage(screen: Screen): Stage? {
        var now: Stage? = null
        instrumentation.runOnMainSync { now = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(screen.activity) }
        return now
    }

    private fun written(name: String, text: String) = File(folder, name).apply { writeText(text) }

    /** What [ShareReceiver] reports once it got [send]. */
    private fun sentTo(screen: Screen, send: Intent): List<String> {
        val got = CountDownLatch(1)
        var report: List<String> = emptyList()
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                report = intent.getStringArrayListExtra("report") ?: emptyList()
                got.countDown()
            }
        }
        // From another app: the test APK's process.
        ContextCompat.registerReceiver(context, receiver, IntentFilter(ShareReceiver.REPORT), ContextCompat.RECEIVER_EXPORTED)
        try {
            send.component = ComponentName(instrumentation.context.packageName, ShareReceiver::class.java.name)
            send.putExtra("reply", context.packageName)
            screen.onActivity { it.startActivity(send) }
            assertTrue("ShareReceiver reported", got.await(15, TimeUnit.SECONDS))
        } finally {
            context.unregisterReceiver(receiver)
        }
        return report
    }

    @Test
    fun theFilesReachTheAppPickedReadOnlyAndNothingElse() {
        val report = written("Report.pdf", "the report")
        val shot = written("shot.png", "the picture")
        val notShared = written("other.png", "not shared")
        val secret = File(context.filesDir, "secret.txt").apply { writeText("private") }
        try {
            Screen.launch().use { screen ->
                val sheet = screen.activity.adapter(ShareSheet::class.java)!!
                val plan = FileShare.plan(
                    listOf(report.path, shot.path),
                    listOf("application/pdf", ""),
                    sheet.root,
                ) { if (it == "png") "image/png" else null }!!
                val send = sheet.files(plan)
                    .putExtra("probe", FileProvider.getUriForFile(context, ShareFiles.authority(context), notShared).toString())
                assertEquals(
                    listOf(
                        "action android.intent.action.SEND_MULTIPLE type */*",
                        "read Report.pdf the report",
                        "write denied",
                        "read shot.png the picture",
                        "write denied",
                        // A file beside them, in the same folder, wasn't granted.
                        "probe denied",
                    ),
                    sentTo(screen, send),
                )
                assertEquals("the report", report.readText())

                // One file goes as ACTION_SEND, with its own type.
                val one = sheet.files(FileShare.plan(listOf(shot.path), listOf("image/png"), sheet.root) { null }!!)
                assertEquals(listOf("action android.intent.action.SEND type image/png", "read shot.png the picture", "write denied"), sentTo(screen, one))
            }
            // The provider serves nothing outside the share folder, whatever asks.
            val outside = runCatching { FileProvider.getUriForFile(context, ShareFiles.authority(context), secret) }
            assertTrue(outside.exceptionOrNull() is IllegalArgumentException)
            // And without a grant, not even the share folder: the shell is another app.
            val uri = FileProvider.getUriForFile(context, ShareFiles.authority(context), report)
            val shell = instrumentation.shell("content read --uri $uri")
            assertTrue("the shell read: $shell", !shell.contains("the report"))
        } finally {
            secret.delete()
        }
    }

    @Test
    fun theChooserOpensAndClosingItAnswersFalseAndOldSharesGo() {
        val file = written("a.png", "x")
        Screen.launch().use { screen ->
            // Once the app's own launch sweep is done: a share from two days
            // ago, still there, which only this share's sweep can take, and
            // this share's own folder made as old, which it must leave.
            Thread.sleep(3_000)
            val twoDaysAgo = System.currentTimeMillis() - 2 * FileShare.KEEP_MS
            val stale = File(root, "stale${System.nanoTime()}").apply { mkdirs(); File(this, "b.png").writeText("y") }
            stale.setLastModified(twoDaysAgo)
            folder.parentFile!!.setLastModified(twoDaysAgo)
            Thread.sleep(1_000)
            assertTrue("nothing else swept it", stale.exists())
            val answer = Answer()
            screen.onActivity {
                it.adapter(ShareSheet::class.java)!!.shareFiles(mapOf("paths" to listOf(file.path), "mimes" to listOf("image/png")), answer)
            }
            // The system's chooser can take a while the first time after a boot.
            waitFor("the chooser over the app (answered ${answer.values})", seconds = 30) {
                stage(screen) == Stage.PAUSED || stage(screen) == Stage.STOPPED
            }
            Thread.sleep(800)
            instrumentation.shell("input keyevent KEYCODE_BACK")
            waitFor("the answer") { answer.values.isNotEmpty() }
            assertEquals(listOf<Any?>(false), answer.values.toList())
            waitFor("the old share swept") { !stale.exists() }
            assertTrue("this share's own files stay", file.exists())
        }
    }

    @Test
    fun aFileOutsideTheShareFolderShowsNothingAndAnswersFalse() {
        val secret = File(context.filesDir, "secret.txt").apply { writeText("private") }
        try {
            Screen.launch().use { screen ->
                val answer = Answer()
                screen.onActivity {
                    it.adapter(ShareSheet::class.java)!!.shareFiles(mapOf("paths" to listOf(secret.path), "mimes" to listOf("")), answer)
                }
                assertEquals(listOf<Any?>(false), answer.values.toList())
                Thread.sleep(800)
                assertEquals(Stage.RESUMED, stage(screen))
            }
        } finally {
            secret.delete()
        }
    }
}
