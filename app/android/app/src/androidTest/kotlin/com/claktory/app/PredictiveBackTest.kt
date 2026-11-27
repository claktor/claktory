package com.claktory.app

import android.annotation.SuppressLint
import android.os.Build
import android.os.SystemClock
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.window.BackEvent
import android.window.OnBackAnimationCallback
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith
import java.util.Collections

/**
 * Back reaches the app as the system's back callbacks, not the old
 * onBackPressed (row 187): the packaged manifest opts in
 * (enableOnBackInvokedCallback), so a swipe from the edge that's pulled back
 * and let go is cancelled, never committed, and only one let go away from
 * the edge goes back. FlutterActivity registers its own callback while the
 * Dart has something to pop and hands it start, progress, cancel and
 * commit; a probe registered above it here sees what the system delivers.
 * The swipes are touches on the screen, injected through UiAutomation as
 * one continuous touch, as a finger makes them, so the system's own edge
 * gesture handles them (an emulator whose edge gesture is off, as on a
 * 320×640 screen behind a size override, sees nothing: use a full-size one).
 */
@RunWith(AndroidJUnit4::class)
@SuppressLint("NewApi") // every test assumes the API it needs first
class PredictiveBackTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    private fun waitFor(what: String, seconds: Int = 5, check: () -> Boolean) {
        val until = System.currentTimeMillis() + seconds * 1000L
        while (System.currentTimeMillis() < until) {
            if (check()) return
            Thread.sleep(50)
        }
        throw AssertionError("no $what within $seconds s")
    }

    /** What the system told the probe, in order. */
    private class Probe : OnBackAnimationCallback {
        val events: MutableList<String> = Collections.synchronizedList(mutableListOf<String>())

        override fun onBackStarted(backEvent: BackEvent) {
            events += "started"
        }

        override fun onBackProgressed(backEvent: BackEvent) {
            if (events.lastOrNull() != "progressed") events += "progressed"
        }

        override fun onBackCancelled() {
            events += "cancelled"
        }

        override fun onBackInvoked() {
            events += "invoked"
        }
    }

    /** [body] with [probe] registered above everything else of the app's (Flutter's included). */
    private fun <T> probed(screen: Screen, probe: OnBackInvokedCallback, body: () -> T): T {
        screen.onActivity { it.onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_OVERLAY, probe) }
        try {
            return body()
        } finally {
            screen.onActivity { it.onBackInvokedDispatcher.unregisterOnBackInvokedCallback(probe) }
        }
    }

    private fun gestureNavigation(): Boolean = instrumentation.shell("settings get secure navigation_mode").trim() == "2"

    /**
     * One touch from the left edge in to [inward] px and, if [returning],
     * back to the edge, held there a moment before it lifts: a single down
     * time, a move every 16 ms, as a finger, injected through UiAutomation
     * with exact times (the shell's `input` makes a process per point, and
     * its uneven gaps made the system's own verdict vary).
     */
    private fun swipe(inward: Float, returning: Boolean) {
        val y = instrumentation.targetContext.resources.displayMetrics.heightPixels / 2f
        val edge = 2f
        val device = InputDevice.getDeviceIds().firstOrNull { id ->
            InputDevice.getDevice(id)?.supportsSource(InputDevice.SOURCE_TOUCHSCREEN) == true
        } ?: 0
        val properties = arrayOf(MotionEvent.PointerProperties().apply { id = 0; toolType = MotionEvent.TOOL_TYPE_FINGER })
        val down = SystemClock.uptimeMillis()
        var time = down
        fun send(action: Int, x: Float) {
            val coords = arrayOf(MotionEvent.PointerCoords().apply { this.x = x; this.y = y; pressure = 1f; size = 1f })
            val event = MotionEvent.obtain(
                down, time, action, 1, properties, coords, 0, 0, 1f, 1f, device, 0, InputDevice.SOURCE_TOUCHSCREEN, 0,
            )
            assertTrue("injected", instrumentation.uiAutomation.injectInputEvent(event, true))
            event.recycle()
        }
        fun step(x: Float) {
            SystemClock.sleep(16)
            time = SystemClock.uptimeMillis()
            send(MotionEvent.ACTION_MOVE, x)
        }
        send(MotionEvent.ACTION_DOWN, edge)
        for (i in 1..20) step(edge + (inward - edge) * i / 20)
        if (returning) {
            for (i in 1..20) step(inward - (inward - edge) * i / 20)
            repeat(10) { step(edge) }
        }
        SystemClock.sleep(16)
        time = SystemClock.uptimeMillis()
        send(MotionEvent.ACTION_UP, if (returning) edge else inward)
    }

    @Test
    fun theManifestOptsIntoTheSystemsBackCallbacks() {
        assumeTrue("OnBackInvokedDispatcher (Android 13+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU)
        Screen.launch().use { screen ->
            val invoked = Collections.synchronizedList(mutableListOf<String>())
            // An app opted out has its callbacks ignored and Back goes to
            // onBackPressed; opted in, the key reaches the topmost callback.
            probed(screen, OnBackInvokedCallback { invoked += "invoked" }) {
                instrumentation.sendKeyDownUpSync(KeyEvent.KEYCODE_BACK)
                waitFor("Back at the probe") { invoked.isNotEmpty() }
            }
            assertEquals(listOf("invoked"), invoked.toList())
        }
    }

    @Test
    fun anEdgeSwipePulledBackIsCancelledNotCommitted() {
        assumeTrue("OnBackAnimationCallback (Android 14+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
        assumeTrue("gesture navigation", gestureNavigation())
        Screen.launch().use { screen ->
            val probe = Probe()
            probed(screen, probe) {
                swipe(inward = 350f, returning = true)
                waitFor("the gesture's end") { probe.events.lastOrNull() in setOf("cancelled", "invoked") }
                SystemClock.sleep(500)
            }
            assertEquals("started", probe.events.first())
            assertEquals("cancelled", probe.events.last())
            assertTrue(probe.events.toString(), "invoked" !in probe.events)
        }
    }

    @Test
    fun anEdgeSwipeLetGoAwayFromTheEdgeGoesBack() {
        assumeTrue("OnBackAnimationCallback (Android 14+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
        assumeTrue("gesture navigation", gestureNavigation())
        Screen.launch().use { screen ->
            val probe = Probe()
            probed(screen, probe) {
                swipe(inward = 350f, returning = false)
                waitFor("the gesture's end") { probe.events.lastOrNull() in setOf("cancelled", "invoked") }
            }
            assertEquals("started", probe.events.first())
            assertEquals("invoked", probe.events.last())
            assertTrue(probe.events.toString(), "cancelled" !in probe.events)
        }
    }
}
