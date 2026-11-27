package com.claktory.app

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.pm.PackageManager
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaRecorder
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import kotlin.math.log10

/**
 * The microphone, as [RecorderCore] uses it: one recording into a file.
 * [MediaMic] on a phone, a fake in tests.
 */
interface Mic {
    /** Starts recording into [path]; throws if it can't. */
    fun start(path: String, maxMs: Int, onLimit: () -> Unit, onError: () -> Unit)

    /** The loudest it's been since last asked, 0 to 32767. */
    fun amplitude(): Int

    fun pause()

    /** Finishes the file; throws if nothing usable was recorded. */
    fun stop()

    fun release()
}

/** Emits [RecorderCore]'s events: about every [every] ms, [tick]. */
interface Ticker {
    fun start(every: Long, tick: () -> Unit)

    fun stop()
}

/**
 * factory/recorder's recording, as on the iPhone (app/macos/Runner/
 * Recorder.swift, which the iPhone builds): AAC in .m4a, 16 kHz mono,
 * about 32 kbps, into the path the app gives, ten minutes at most. While
 * recording, [emit] gets {level 0…1, ms} about every 50 ms; once, if it
 * stops by itself, {ended: limit | interrupted | error}. After limit or
 * interrupted the file stays for [stop] to hand back; after error it's
 * gone. Pure logic over a [Mic], a clock and a [Ticker], so it's tested
 * on the JVM.
 */
class RecorderCore(
    private val newMic: () -> Mic,
    private val permitted: () -> Boolean,
    private val ticker: Ticker,
    private val emit: (Map<String, Any>) -> Unit,
    private val clock: () -> Long = SystemClock::elapsedRealtime,
) {
    /** Why [start] refused: "denied", "busy" or "failed", as the iPhone says. */
    class Refused(val code: String, message: String) : Exception(message)

    private var mic: Mic? = null
    private var path: String? = null
    private var startedAt = 0L

    /** How long it ran, once it stopped by itself (limit or interrupted). */
    private var stoppedAfter: Long? = null

    /** The mic stopped its file itself (at the limit): not stopped again. */
    private var micFinished = false

    val recording get() = mic != null

    fun start(path: String) {
        if (mic != null) throw Refused("busy", "Already recording.")
        if (!permitted()) throw Refused("denied", "Claktory can't use the microphone.")
        File(path).parentFile?.mkdirs()
        val mic = newMic()
        try {
            mic.start(path, MAX_MS, onLimit = { limit(mic) }, onError = { failed(mic) })
        } catch (error: Exception) {
            mic.release()
            File(path).delete()
            throw Refused("failed", "The microphone couldn't start.")
        }
        this.mic = mic
        this.path = path
        startedAt = clock()
        stoppedAfter = null
        micFinished = false
        ticker.start(TICK_MS) { tick() }
    }

    /** {path, durationMs}, or null if nothing was recording. */
    fun stop(): Map<String, Any>? {
        val mic = mic ?: return null
        val path = path!!
        var duration = stoppedAfter ?: (clock() - startedAt)
        val finished = micFinished
        finish()
        if (!finished) {
            try {
                mic.stop()
            } catch (error: Exception) {
                // Too short to make a file of: it says nothing, which the
                // app takes as too short to send.
                duration = 0
            }
        }
        mic.release()
        return mapOf("path" to path, "durationMs" to duration.coerceAtMost(MAX_MS.toLong()).toInt())
    }

    /** Stops and deletes the file. */
    fun cancel() {
        val mic = mic ?: return
        val path = path
        val finished = micFinished
        finish()
        if (!finished) runCatching { mic.stop() }
        mic.release()
        path?.let { File(it).delete() }
    }

    /**
     * Something else took the microphone (a call), or the app left the
     * screen, where Android gives it nothing: paused, the file kept for
     * [stop].
     */
    fun interrupt() {
        val mic = mic ?: return
        if (stoppedAfter != null) return
        stoppedAfter = clock() - startedAt
        ticker.stop()
        runCatching { mic.pause() }
        emit(mapOf("ended" to "interrupted"))
    }

    private fun tick() {
        val mic = mic ?: return
        if (stoppedAfter != null) return
        emit(mapOf("level" to level(mic.amplitude()), "ms" to (clock() - startedAt).toInt()))
    }

    private fun limit(mic: Mic) {
        if (mic !== this.mic || stoppedAfter != null) return
        stoppedAfter = MAX_MS.toLong()
        micFinished = true
        ticker.stop()
        emit(mapOf("ended" to "limit"))
    }

    private fun failed(mic: Mic) {
        if (mic !== this.mic) return
        val path = path
        finish()
        mic.release()
        path?.let { File(it).delete() }
        emit(mapOf("ended" to "error"))
    }

    private fun finish() {
        ticker.stop()
        mic = null
        path = null
        stoppedAfter = null
        micFinished = false
    }

    companion object {
        /** Ten minutes, as on the iPhone. */
        const val MAX_MS = 600_000
        const val TICK_MS = 50L

        /** Loudness 0…1 from an amplitude: -50 dB and below is silence, as on the iPhone. */
        fun level(amplitude: Int): Double {
            if (amplitude <= 0) return 0.0
            val db = 20 * log10(amplitude / 32767.0)
            return ((db + 50) / 50).coerceIn(0.0, 1.0)
        }
    }
}

/** The phone's microphone: [MediaRecorder], AAC in MPEG-4 at 16 kHz mono, 32 kbps. */
class MediaMic(private val context: Context) : Mic {
    private var recorder: MediaRecorder? = null

    override fun start(path: String, maxMs: Int, onLimit: () -> Unit, onError: () -> Unit) {
        val recorder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) MediaRecorder(context) else @Suppress("DEPRECATION") MediaRecorder()
        this.recorder = recorder
        recorder.setAudioSource(MediaRecorder.AudioSource.MIC)
        recorder.setOutputFormat(MediaRecorder.OutputFormat.MPEG_4)
        recorder.setAudioEncoder(MediaRecorder.AudioEncoder.AAC)
        recorder.setAudioSamplingRate(16_000)
        recorder.setAudioChannels(1)
        recorder.setAudioEncodingBitRate(32_000)
        recorder.setMaxDuration(maxMs)
        recorder.setOutputFile(path)
        recorder.setOnInfoListener { _, what, _ ->
            if (what == MediaRecorder.MEDIA_RECORDER_INFO_MAX_DURATION_REACHED) onLimit()
        }
        recorder.setOnErrorListener { _, _, _ -> onError() }
        recorder.prepare()
        recorder.start()
    }

    override fun amplitude(): Int = runCatching { recorder?.maxAmplitude ?: 0 }.getOrDefault(0)

    override fun pause() {
        recorder?.pause()
    }

    override fun stop() {
        recorder?.stop()
    }

    override fun release() {
        recorder?.release()
        recorder = null
    }
}

/**
 * factory/recorder and factory/recorder/events on Android: [RecorderCore]
 * with the phone's microphone, its permission, and what interrupts it.
 *
 * Access as [PermissionState] reads it. Recording takes the audio focus,
 * as the iPhone's session does: a call taking it, or the app leaving the
 * screen (Android gives an app in the background no microphone),
 * interrupts it.
 */
class RecorderChannel(
    messenger: BinaryMessenger,
    private val activity: Activity,
) : MethodChannel.MethodCallHandler, EventChannel.StreamHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/recorder")
    private val events = EventChannel(messenger, "factory/recorder/events")
    private val main = Handler(Looper.getMainLooper())
    private var sink: EventChannel.EventSink? = null
    private var asking: MethodChannel.Result? = null
    private val audio = activity.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private var focus: AudioFocusRequest? = null
    private val focusChanged = AudioManager.OnAudioFocusChangeListener { change ->
        if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) core.interrupt()
    }

    private val core = RecorderCore(
        newMic = { MediaMic(activity.applicationContext) },
        permitted = ::granted,
        ticker = MainTicker(main),
        emit = { event -> sink?.success(event) },
    )

    init {
        channel.setMethodCallHandler(this)
        events.setStreamHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "access" -> result.success(
                PermissionState.of(
                    granted = granted(),
                    refusedForGood = Refusals.refusedForGood(Manifest.permission.RECORD_AUDIO),
                    androidWouldExplain = activity.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO),
                ),
            )
            "refusedOnce" -> result.success(
                PermissionState.refusedOnce(
                    granted = granted(),
                    androidWouldExplain = activity.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO),
                ),
            )
            "request" -> request(result)
            "openSettings" -> {
                AppSettings.open(activity)
                result.success(null)
            }
            "start" -> {
                val path = call.arguments as? String ?: return result.error("failed", "No file to record into.", null)
                try {
                    takeFocus()
                    core.start(path)
                    result.success(null)
                } catch (refused: RecorderCore.Refused) {
                    if (!core.recording) giveFocus()
                    result.error(refused.code, refused.message, null)
                }
            }
            "stop" -> {
                val answer = core.stop()
                giveFocus()
                result.success(answer)
            }
            "cancel" -> {
                core.cancel()
                giveFocus()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    private fun granted() = activity.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    private fun request(result: MethodChannel.Result) {
        if (granted()) return result.success(true)
        if (asking != null) return result.success(false)
        asking = result
        Refusals.asking(
            Manifest.permission.RECORD_AUDIO,
            explains = activity.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO),
            at = SystemClock.elapsedRealtime(),
        )
        activity.requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST)
    }

    override fun onPermissionResult(requestCode: Int, permissions: Array<out String>, grants: IntArray): Boolean {
        if (requestCode != REQUEST) return false
        val allowed = grants.isNotEmpty() && grants.all { it == PackageManager.PERMISSION_GRANTED }
        Refusals.answered(
            Manifest.permission.RECORD_AUDIO,
            allowed,
            explains = activity.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO),
            at = SystemClock.elapsedRealtime(),
        )
        asking?.success(allowed)
        asking = null
        return true
    }

    /** Away from the screen, Android gives the app only silence. */
    override fun onPause() = core.interrupt()

    private fun takeFocus() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build(),
                )
                .setOnAudioFocusChangeListener(focusChanged, main)
                .build()
            focus = request
            audio.requestAudioFocus(request)
        } else {
            @Suppress("DEPRECATION")
            audio.requestAudioFocus(focusChanged, AudioManager.STREAM_VOICE_CALL, AudioManager.AUDIOFOCUS_GAIN_TRANSIENT_EXCLUSIVE)
        }
    }

    private fun giveFocus() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focus?.let(audio::abandonAudioFocusRequest)
            focus = null
        } else {
            @Suppress("DEPRECATION")
            audio.abandonAudioFocus(focusChanged)
        }
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        sink = events
    }

    override fun onCancel(arguments: Any?) {
        sink = null
    }

    override fun close() {
        core.cancel()
        giveFocus()
        asking?.success(false)
        asking = null
        channel.setMethodCallHandler(null)
        events.setStreamHandler(null)
    }

    companion object {
        /** Ours alone among the activity's permission requests. */
        const val REQUEST = 0x4175
    }
}
