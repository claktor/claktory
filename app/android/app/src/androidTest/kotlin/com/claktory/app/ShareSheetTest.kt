package com.claktory.app

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * The share sheet in the running app: the system's chooser opens over it,
 * closing it with Back answers false, and the chooser's report of a pick
 * answers true. Picking a real app in the chooser is checked by hand on a
 * phone; here the report is sent as the chooser sends it, to the same
 * receiver.
 */
@RunWith(AndroidJUnit4::class)
class ShareSheetTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

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

    private fun screenIs(stage: Stage, screen: Screen): Boolean {
        var now: Stage? = null
        instrumentation.runOnMainSync { now = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(screen.activity) }
        return now == stage
    }

    private fun back() {
        instrumentation.shell("input keyevent KEYCODE_BACK")
    }

    @Test
    fun closingTheSheetAnswersFalse() {
        Screen.launch().use { screen ->
            val answer = Answer()
            screen.onActivity { it.adapter(ShareSheet::class.java)!!.core.share("Join my Claktory: ABC-123", answer) }
            waitFor("the chooser over the app") { screenIs(Stage.PAUSED, screen) || screenIs(Stage.STOPPED, screen) }
            Thread.sleep(800)
            back()
            waitFor("the answer") { answer.values.isNotEmpty() }
            assertEquals(listOf<Any?>(false), answer.values.toList())
        }
    }

    @Test
    fun thePickReportedAnswersTrue() {
        Screen.launch().use { screen ->
            val answer = Answer()
            screen.onActivity { it.adapter(ShareSheet::class.java)!!.core.share("Join my Claktory: ABC-123", answer) }
            waitFor("the chooser over the app") { screenIs(Stage.PAUSED, screen) || screenIs(Stage.STOPPED, screen) }
            context.sendBroadcast(Intent("${context.packageName}.SHARED").setPackage(context.packageName))
            waitFor("the answer") { answer.values.isNotEmpty() }
            assertEquals(listOf<Any?>(true), answer.values.toList())
            back()
            waitFor("the app back") { screenIs(Stage.RESUMED, screen) }
            Thread.sleep(ShareCore.LATE_MS + 300)
            assertEquals("answered once", listOf<Any?>(true), answer.values.toList())
        }
    }
}
