package com.claktory.app

import android.app.Activity
import android.content.res.Configuration
import android.os.Build
import androidx.core.view.WindowCompat

/**
 * The system bars' icons follow the canvas, as the app's palette follows
 * the system's appearance: dark icons on the light canvas, light ones on
 * the dark. The theme says so for the first window, but the activity
 * handles uiMode itself (configChanges), so switching between light and
 * dark doesn't recreate it and the theme's flags would stay as they were:
 * this sets them when the activity starts, resumes, gains focus and on
 * every change.
 *
 * Before Android 15 the navigation bar has a color of its own (black by
 * default), so it takes the canvas's, or its dark buttons would vanish on
 * black. From Android 15 the app draws behind it and the color is ignored.
 * Android 7 can't darken the navigation bar's buttons; it keeps its black bar.
 */
class SystemBars(private val activity: Activity) : HostLifecycle, AutoCloseable {
    init {
        apply(activity.resources.configuration)
    }

    override fun onConfiguration(config: Configuration) = apply(config)

    /**
     * Again once Flutter has resumed: its onPostResume sets the window's
     * system UI flags to its own, and before Android 11 those flags carry
     * the light-bar ones too, so it would turn the icons white again.
     * And when the window gains focus, as a last word after anything else.
     */
    override fun onPostResume() = apply(activity.resources.configuration)

    override fun onFocus(focused: Boolean) {
        if (focused) apply(activity.resources.configuration)
    }

    private fun apply(config: Configuration) {
        val light = light(config)
        val window = activity.window
        val bars = WindowCompat.getInsetsController(window, window.decorView)
        bars.isAppearanceLightStatusBars = light
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            bars.isAppearanceLightNavigationBars = light
            if (Build.VERSION.SDK_INT < Build.VERSION_CODES.VANILLA_ICE_CREAM) {
                @Suppress("DEPRECATION")
                window.navigationBarColor = activity.getColor(R.color.canvas)
            }
        }
    }

    override fun close() {}

    companion object {
        /** Light unless the system says night: the palette's own test. */
        fun light(config: Configuration): Boolean =
            config.uiMode and Configuration.UI_MODE_NIGHT_MASK != Configuration.UI_MODE_NIGHT_YES
    }
}
