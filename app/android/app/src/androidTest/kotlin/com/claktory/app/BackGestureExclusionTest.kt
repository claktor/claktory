package com.claktory.app

import android.graphics.Rect
import android.os.Build
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.filters.SdkSuppress
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

/**
 * factory/back-gesture in the running app (Sale's row 86): the rects the
 * app sends, in its logical pixels, become the Flutter view's system
 * gesture exclusion, in its pixels, and an empty list gives the edges
 * back. Android 10 and later, where gesture navigation's Back is.
 * That a real finger's swipe from there then stays the app's is checked
 * by hand: on the emulator, injected swipes (input swipe, motionevent)
 * never start the Back gesture, so a test of it here couldn't fail.
 */
@RunWith(AndroidJUnit4::class)
class BackGestureExclusionTest {
    private fun exclude(adapter: BackGestureExclusion, rects: List<List<Number>>): Any? {
        var answer: Any? = "none"
        adapter.onMethodCall(MethodCall("exclude", mapOf("rects" to rects)), object : MethodChannel.Result {
            override fun success(result: Any?) { answer = result }
            override fun error(code: String, message: String?, details: Any?) { answer = "error $code" }
            override fun notImplemented() { answer = "notImplemented" }
        })
        return answer
    }

    @Test
    @SdkSuppress(minSdkVersion = Build.VERSION_CODES.Q)
    fun theMicsPlaceIsTakenOutOfBackAndGivenBack() {
        Screen.launch().use { screen ->
            screen.onActivity { activity ->
                val adapter = activity.adapter(BackGestureExclusion::class.java)!!
                val density = activity.resources.displayMetrics.density
                assertEquals(true, exclude(adapter, listOf(listOf(337, 700, 48, 48))))
                val expected = Rect(
                    Math.floor(337.0 * density).toInt(),
                    Math.floor(700.0 * density).toInt(),
                    Math.ceil(385.0 * density).toInt(),
                    Math.ceil(748.0 * density).toInt(),
                )
                assertEquals(listOf(expected), adapter.excluded())
                assertEquals("error bad_rects", exclude(adapter, listOf(listOf(1, 2, 3))))
                assertEquals("a refused call changes nothing", listOf(expected), adapter.excluded())
                assertEquals(true, exclude(adapter, emptyList()))
                assertEquals(emptyList<Rect>(), adapter.excluded())
            }
        }
    }
}
