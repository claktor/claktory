package com.claktory.app

import android.content.Intent
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

    override fun onPause() {
        adapters?.onPause()
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        adapters?.onFocus(hasFocus)
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
