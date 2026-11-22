package com.claktory.app

import android.content.Context
import io.flutter.embedding.engine.FlutterEngine

/**
 * Which adapter answers which channel. Each owns its channel and its
 * Android resources; [close] releases them when the engine goes.
 */
class Adapters private constructor(private val all: List<AutoCloseable>) : AutoCloseable {
    override fun close() = all.forEach { it.close() }

    companion object {
        fun register(engine: FlutterEngine, context: Context): Adapters {
            val messenger = engine.dartExecutor.binaryMessenger
            return Adapters(
                listOf(
                    // factory/network: iroh can't see network moves on Android.
                    NetworkPath(messenger, context),
                    // factory/device: what this phone keeps and what it is.
                    DeviceChannel(messenger, context),
                ),
            )
        }
    }
}
