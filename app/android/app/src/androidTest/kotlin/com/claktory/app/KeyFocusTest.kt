package com.claktory.app

import android.os.SystemClock
import android.view.InputDevice
import android.view.MotionEvent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.android.FlutterView
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * A keyboard's shortcuts reach the app when it was opened and used by
 * touch: Android gives no view focus in touch mode, and a key that isn't
 * typing (Ctrl+F) doesn't leave it, so without a focused view the key went
 * nowhere. The app's view takes focus with its window.
 */
@RunWith(AndroidJUnit4::class)
class KeyFocusTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()

    @After
    fun leaveTouchMode() = instrumentation.setInTouchMode(false)

    private fun focused(screen: Screen): Boolean {
        var focused = false
        screen.onActivity { focused = it.currentFocus is FlutterView }
        return focused
    }

    @Test
    fun theAppsViewHoldsFocusInTouchMode() {
        // As when the person opens it from the launcher with a tap.
        instrumentation.setInTouchMode(true)
        Screen.launch().use { screen ->
            val until = System.currentTimeMillis() + 10_000
            while (!focused(screen) && System.currentTimeMillis() < until) Thread.sleep(100)
            var touchMode = false
            screen.onActivity { touchMode = it.window.decorView.isInTouchMode }
            assertTrue("still in touch mode", touchMode)
            assertTrue("FlutterView has focus", focused(screen))
            // A tap on the app doesn't take it away.
            var view: FlutterView? = null
            screen.onActivity { view = it.findViewById(FlutterActivity.FLUTTER_VIEW_ID) }
            val now = SystemClock.uptimeMillis()
            for (action in listOf(MotionEvent.ACTION_DOWN, MotionEvent.ACTION_UP)) {
                val event = MotionEvent.obtain(now, now, action, view!!.width / 2f, view!!.height / 2f, 0)
                    .apply { source = InputDevice.SOURCE_TOUCHSCREEN }
                instrumentation.uiAutomation.injectInputEvent(event, true)
                event.recycle()
            }
            Thread.sleep(500)
            assertTrue("FlutterView keeps focus after a tap", focused(screen))
        }
    }
}
