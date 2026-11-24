package com.claktory.app

import android.content.Intent
import android.content.res.Configuration
import android.view.View
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

/** Thin on purpose: each native feature is an adapter of its own ([Adapters]). */
class MainActivity : FlutterActivity() {
    private var adapters: Adapters? = null

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
