package com.claktory.app

import android.app.Activity
import android.graphics.Rect
import android.os.Build
import android.view.View
import io.flutter.embedding.android.FlutterActivity
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import kotlin.math.ceil
import kotlin.math.floor

/**
 * factory/back-gesture (Sale's row 86): `exclude {rects: [[left, top,
 * width, height], …]}` in the app's logical pixels, from its window's
 * top left, takes those places out of the system's Back gesture, so a
 * finger that starts there stays the app's. That's the hold-to-record
 * mic, near the right edge, which was otherwise "slide to cancel" and
 * left the app. The whole list each time; an empty one gives the edges
 * back. Android decides Back as the finger comes down, so the app keeps
 * the mic's place excluded whenever it's on screen, not only when held.
 * Android 10 and later (gesture navigation's Back); answers whether it
 * applied. Android takes at most 200 dp of each edge's height.
 */
class BackGestureExclusion(messenger: BinaryMessenger, private val activity: Activity) :
    MethodChannel.MethodCallHandler, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/back-gesture")

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "exclude" -> {
                val boxes = boxes((call.arguments as? Map<*, *>)?.get("rects"), activity.resources.displayMetrics.density)
                    ?: return result.error("bad_rects", "rects: [[left, top, width, height], …], at most $MAX", null)
                result.success(apply(boxes))
            }
            else -> result.notImplemented()
        }
    }

    /** What's excluded now, in the view's pixels, for tests. */
    internal fun excluded(): List<Rect> =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) flutterView()?.systemGestureExclusionRects.orEmpty() else emptyList()

    private fun flutterView(): View? = activity.findViewById(FlutterActivity.FLUTTER_VIEW_ID)

    private fun apply(boxes: List<IntArray>): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return false
        val view = flutterView() ?: return false
        view.systemGestureExclusionRects = boxes.map { Rect(it[0], it[1], it[2], it[3]) }
        return true
    }

    override fun close() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) flutterView()?.systemGestureExclusionRects = emptyList()
        channel.setMethodCallHandler(null)
    }

    companion object {
        /** More than the app ever has on screen at once. */
        const val MAX = 8

        /**
         * [rects] as the view's pixels ([left, top, right, bottom], covering
         * every pixel the logical rect touches), or null when it isn't a list
         * of at most [MAX] finite rects of no negative size.
         */
        fun boxes(rects: Any?, density: Float): List<IntArray>? {
            val list = rects as? List<*> ?: return null
            if (list.size > MAX) return null
            return list.map { item ->
                val numbers = (item as? List<*>)?.map { (it as? Number)?.toDouble() ?: return null } ?: return null
                if (numbers.size != 4 || numbers.any { !it.isFinite() }) return null
                val (left, top, width, height) = numbers
                if (width < 0 || height < 0) return null
                intArrayOf(
                    floor(left * density).toInt(),
                    floor(top * density).toInt(),
                    ceil((left + width) * density).toInt(),
                    ceil((top + height) * density).toInt(),
                )
            }
        }
    }
}
