package com.claktory.app

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.runner.lifecycle.ActivityLifecycleMonitorRegistry
import androidx.test.runner.lifecycle.Stage
import io.flutter.plugin.common.MethodChannel
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * When Android takes the app's screen away while a system screen is over
 * it (as under memory pressure, or Developer options' "Don't keep
 * activities"), every call waiting on that system screen answers,
 * honestly, as it goes, never left hanging: the share sheet false, web
 * sign-in null, the attach menu an empty list. Nothing brings the app
 * back by itself. The screen is finished here as Android would finish it,
 * with the system's screen over it; no device setting is changed.
 */
@RunWith(AndroidJUnit4::class)
class RecreationTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    private fun shell(command: String): String =
        instrumentation.uiAutomation.executeShellCommand(command).use { pipe ->
            java.io.FileInputStream(pipe.fileDescriptor).use { String(it.readBytes()) }
        }

    @After
    fun closeWhatsLeftOver() {
        shell("input keyevent KEYCODE_BACK")
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

    private fun gone(screen: Screen): Boolean {
        var stage: Stage? = null
        instrumentation.runOnMainSync { stage = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(screen.activity) }
        return stage == Stage.DESTROYED
    }

    private fun covered(screen: Screen): Boolean {
        var stage: Stage? = null
        instrumentation.runOnMainSync { stage = ActivityLifecycleMonitorRegistry.getInstance().getLifecycleStageOf(screen.activity) }
        return stage == Stage.PAUSED || stage == Stage.STOPPED
    }

    private fun screensUp(): Int {
        var count = 0
        instrumentation.runOnMainSync {
            count = listOf(Stage.CREATED, Stage.STARTED, Stage.RESUMED)
                .flatMap { ActivityLifecycleMonitorRegistry.getInstance().getActivitiesInStage(it) }
                .count { it is MainActivity }
        }
        return count
    }

    @Test
    fun theShareSheetAnswersFalseWhenTheScreenIsTakenAway() {
        val screen = Screen.launch()
        val answer = Answer()
        screen.onActivity { it.adapter(ShareSheet::class.java)!!.core.share("Join my Claktory: ABC-123", answer) }
        waitFor("the chooser over the app") { covered(screen) }
        screen.onActivity { it.finish() }
        waitFor("the screen taken away") { gone(screen) }
        waitFor("the answer") { answer.values.isNotEmpty() }
        assertEquals(listOf<Any?>(false), answer.values.toList())
    }

    @Test
    fun aSignInAnswersNullAndDoesntBringTheAppBack() {
        val screen = Screen.launch()
        val answer = Answer()
        screen.onActivity { WebAuth.current!!.core.start("https://example.invalid/sign-in", screen.activity.packageName, answer) }
        Thread.sleep(1_000)
        if (answer.values.firstOrNull() == "error failed") return // No browser on this image: nothing was shown.
        waitFor("the page over the app") { covered(screen) }
        screen.onActivity { it.finish() }
        waitFor("the screen taken away") { gone(screen) }
        waitFor("the answer") { answer.values.isNotEmpty() }
        assertEquals(listOf<Any?>(null), answer.values.toList())
        Thread.sleep(1_500)
        assertEquals("nothing opened the app again", 0, screensUp())
    }

    @Test
    fun theAttachMenuAnswersAnEmptyListWhenTheScreenGoes() {
        val screen = Screen.launch()
        val answer = Answer()
        screen.onActivity { it.adapter(AttachmentPicker::class.java)!!.pick(answer) }
        // The menu is the app's own; the screen going takes it along.
        screen.close()
        waitFor("the answer") { answer.values.isNotEmpty() }
        assertEquals(listOf<Any?>(emptyList<String>()), answer.values.toList())
        assertTrue(gone(screen))
    }
}
