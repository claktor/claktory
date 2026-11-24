package com.claktory.app

import android.os.Build
import android.view.View
import android.view.WindowInsetsController
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The status and navigation bars' icons are dark on the light canvas and
 * light on the dark, at launch, after Flutter's own resume (which before
 * Android 11 resets the window's flags), and after the system switches
 * between light and dark while the app is open (the activity isn't
 * recreated then). Read from what the system draws by: the window's
 * appearance from Android 11, its system UI flags before. The navigation
 * bar's buttons darken from Android 8; the dark appearance exists from
 * Android 10. The system's night mode is put back as it was.
 */
@RunWith(AndroidJUnit4::class)
class SystemBarsTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val darkModes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q
    private var before = "no"

    private fun night(on: Boolean) {
        instrumentation.shell("cmd uimode night ${if (on) "yes" else "no"}")
    }

    @Before
    fun remember() {
        if (!darkModes) return
        // "Night mode: no" / "yes" / "auto" / "custom".
        before = Regex("""Night mode: (\w+)""").find(instrumentation.shell("cmd uimode night"))?.groupValues?.get(1) ?: "no"
    }

    @After
    fun restore() {
        if (darkModes) instrumentation.shell("cmd uimode night $before")
    }

    /** Which bars have dark icons now: (status, navigation). */
    private fun darkIcons(screen: Screen): Pair<Boolean, Boolean> {
        var bars = false to false
        screen.onActivity {
            bars = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val flags = it.window.insetsController!!.systemBarsAppearance
                (flags and WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS != 0) to
                    (flags and WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS != 0)
            } else {
                @Suppress("DEPRECATION")
                val flags = it.window.decorView.systemUiVisibility
                @Suppress("DEPRECATION", "InlinedApi")
                (flags and View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR != 0) to
                    (flags and View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR != 0)
            }
        }
        return bars
    }

    private fun waitForBars(screen: Screen, light: Boolean) {
        val navigation = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
        val want = light to (light && navigation)
        val until = System.currentTimeMillis() + 10_000
        var now = darkIcons(screen)
        while (System.currentTimeMillis() < until) {
            now = darkIcons(screen)
            if (now == want) {
                // Still so once Flutter has had its resume.
                Thread.sleep(500)
                now = darkIcons(screen)
                if (now == want) return
            }
            Thread.sleep(100)
        }
        throw AssertionError("on the ${if (light) "light" else "dark"} canvas, dark icons (status, navigation) are $now, not $want")
    }

    @Test
    fun darkIconsOnTheLightCanvasEvenAfterFlutterResumes() {
        if (darkModes) night(false)
        Screen.launch().use { screen -> waitForBars(screen, light = true) }
    }

    @Test
    fun lightIconsOnTheDarkCanvasAndAcrossALiveSwitch() {
        assumeTrue("no dark appearance before Android 10", darkModes)
        night(false)
        Screen.launch().use { screen ->
            waitForBars(screen, light = true)
            night(true)
            waitForBars(screen, light = false)
            night(false)
            waitForBars(screen, light = true)
        }
        night(true)
        Screen.launch().use { screen -> waitForBars(screen, light = false) }
    }
}
