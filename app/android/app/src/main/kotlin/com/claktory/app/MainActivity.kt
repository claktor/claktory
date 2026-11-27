package com.claktory.app

import android.content.Intent
import android.content.res.Configuration
import android.os.Build
import android.os.Bundle
import android.view.View
import android.window.OnBackInvokedCallback
import android.window.OnBackInvokedDispatcher
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

/** Thin on purpose: each native feature is an adapter of its own ([Adapters]). */
class MainActivity : FlutterActivity() {
    private var adapters: Adapters? = null

    /**
     * Back at the root from Android 13, where Back comes as the system's
     * callbacks (enableOnBackInvokedCallback, row 187) and onBackPressed no
     * longer does: registered before Flutter's own, so Flutter's (there while
     * the Dart has something to pop) is always above it and this answers
     * only when nothing else does. The system's default there finishes a
     * task that wasn't started from the launcher (a link, a share), tearing
     * the engine down; this leaves as [popSystemNavigator] does, as Home.
     */
    private val rootBack: OnBackInvokedCallback? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) OnBackInvokedCallback { leaveAtRoot() } else null

    override fun onCreate(savedInstanceState: Bundle?) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            onBackInvokedDispatcher.registerOnBackInvokedCallback(OnBackInvokedDispatcher.PRIORITY_DEFAULT, rootBack!!)
        }
        super.onCreate(savedInstanceState)
    }

    private fun leaveAtRoot() {
        if (!popSystemNavigator()) finish()
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        adapters = Adapters.register(flutterEngine, this)
    }

    /** The running adapter of this kind, for device tests. */
    internal fun <T> adapter(kind: Class<T>): T? = adapters?.find(kind)

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        adapters?.close()
        adapters = null
        super.cleanUpFlutterEngine(flutterEngine)
    }

    /**
     * Back at the app's root leaves it as Home does: the task goes behind,
     * the screen only stops, and a recording pauses and waits with Send and
     * Cancel, as for a call. Flutter's own answer is finish(), which tore the
     * engine down and threw the recording away. Android 12+ already treats
     * Back on a launcher's root this way, which finish() went around; this
     * makes it so on every version. Not the task's root (never so today),
     * Flutter finishes as before.
     */
    override fun popSystemNavigator(): Boolean = isTaskRoot && moveTaskToBack(false)

    override fun onResume() {
        super.onResume()
        adapters?.onResume()
    }

    override fun onPostResume() {
        super.onPostResume()
        adapters?.onPostResume()
    }

    override fun onPause() {
        adapters?.onPause()
        super.onPause()
    }

    override fun onConfigurationChanged(newConfig: Configuration) {
        super.onConfigurationChanged(newConfig)
        adapters?.onConfiguration(newConfig)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) keepKeysComing()
        adapters?.onFocus(hasFocus)
    }

    /**
     * The app's view holds focus whenever its window does. Opened by touch,
     * Android gives no view focus (targetSdk 28+), and a tap doesn't give
     * it either (Flutter takes touches itself), so a keyboard's shortcuts
     * (Ctrl+F, which isn't typing and doesn't leave touch mode) reached no
     * view until a letter was typed. FlutterView is focusable in touch
     * mode; focusing it opens no keyboard.
     */
    private fun keepKeysComing() {
        if (currentFocus != null) return
        window.decorView.findViewById<View>(FLUTTER_VIEW_ID)?.requestFocus()
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        if (adapters?.onPermissionResult(requestCode, permissions, grantResults) != true) {
            super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        adapters?.onNewIntent(intent)
    }

    @Deprecated("FlutterActivity is an Activity, not a ComponentActivity: results come here.")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (adapters?.onActivityResult(requestCode, resultCode, data) != true) {
            @Suppress("DEPRECATION")
            super.onActivityResult(requestCode, resultCode, data)
        }
    }
}
