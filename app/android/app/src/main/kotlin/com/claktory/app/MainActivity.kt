package com.claktory.app

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

/** Thin on purpose: each native feature is an adapter of its own ([Adapters]). */
class MainActivity : FlutterActivity() {
    private var adapters: Adapters? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        adapters = Adapters.register(flutterEngine, applicationContext)
    }

    override fun cleanUpFlutterEngine(flutterEngine: FlutterEngine) {
        adapters?.close()
        adapters = null
        super.cleanUpFlutterEngine(flutterEngine)
    }
}
