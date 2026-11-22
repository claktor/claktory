package com.claktory.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.MediaPlayer
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.core.content.ContextCompat
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/** A [Ticker] on the main thread's looper. */
class MainTicker(private val main: Handler = Handler(Looper.getMainLooper())) : Ticker {
    private var running: Runnable? = null

    override fun start(every: Long, tick: () -> Unit) {
        stop()
        val next = object : Runnable {
            override fun run() {
                tick()
                main.postDelayed(this, every)
            }
        }
        running = next
        main.postDelayed(next, every)
    }

    override fun stop() {
        running?.let(main::removeCallbacks)
        running = null
    }
}

/** One recording, ready to play, as [AudioCore] uses it: [MediaSound] on a phone, a fake in tests. */
interface Sound {
    val durationMs: Int
    val positionMs: Int
    val playing: Boolean

    /** Throws if it can't. */
    fun play()

    fun pause()

    fun seekTo(ms: Int)

    fun release()
}

/**
 * factory/audio's player, as on the iPhone (app/macos/Runner/
 * AudioPlayer.swift, which the iPhone builds): one recording at a time,
 * from a file on this device. play(path, fromMs) starts that file, or
 * resumes it from fromMs, and whatever played before stops; pause, seek,
 * stop. While one plays, about every 100 ms, and on every change, [emit]
 * gets {path, positionMs, durationMs, state: playing | paused | ended |
 * stopped}. [active] says when it starts and stops needing the speaker
 * (the audio focus). Pure logic over [open] and a [Ticker], tested on the
 * JVM.
 */
class AudioCore(
    /** Opens the file, calling back when it plays to its end; throws if it can't be played. */
    private val open: (path: String, onEnd: () -> Unit) -> Sound,
    private val ticker: Ticker,
    private val emit: (Map<String, Any>) -> Unit,
    private val active: (Boolean) -> Unit = {},
) {
    class Refused : Exception("This recording can't be played.")

    private var sound: Sound? = null
    private var path: String? = null
    private var ticking = false

    fun play(path: String, fromMs: Int) {
        if (path != this.path || sound == null) {
            stop()
            lateinit var opened: Sound
            opened = try {
                open(path) { ended(opened) }
            } catch (error: Exception) {
                throw Refused()
            }
            sound = opened
            this.path = path
        }
        val sound = sound!!
        active(true)
        sound.seekTo(fromMs.coerceIn(0, sound.durationMs.coerceAtLeast(0)))
        try {
            sound.play()
        } catch (error: Exception) {
            throw Refused()
        }
        ticking = true
        ticker.start(TICK_MS) { report("playing") }
        report("playing")
    }

    fun pause() {
        sound?.let { runCatching { it.pause() } }
        stopTicking()
        report("paused")
    }

    fun seek(ms: Int) {
        val sound = sound ?: return
        sound.seekTo(ms.coerceIn(0, sound.durationMs.coerceAtLeast(0)))
        report(if (sound.playing) "playing" else "paused")
    }

    /** Done with what played, telling the app. */
    fun stop() {
        val sound = sound ?: return
        runCatching { sound.pause() }
        stopTicking()
        report("stopped")
        release(sound)
    }

    /**
     * Something else took the speaker (a call), the headphones went, or
     * the app left the screen: paused, as the iPhone does, for the app to
     * play on from there.
     */
    fun interrupt() {
        val sound = sound ?: return
        if (!sound.playing && !ticking) return
        runCatching { sound.pause() }
        stopTicking()
        report("paused")
    }

    private fun ended(sound: Sound) {
        if (sound !== this.sound) return
        val path = path ?: return
        stopTicking()
        emit(mapOf("path" to path, "positionMs" to sound.durationMs, "durationMs" to sound.durationMs, "state" to "ended"))
        release(sound)
    }

    private fun release(sound: Sound) {
        runCatching { sound.release() }
        this.sound = null
        path = null
        active(false)
    }

    private fun stopTicking() {
        ticker.stop()
        ticking = false
    }

    private fun report(state: String) {
        val sound = sound ?: return
        val path = path ?: return
        emit(mapOf("path" to path, "positionMs" to sound.positionMs, "durationMs" to sound.durationMs, "state" to state))
    }

    companion object {
        const val TICK_MS = 100L
    }
}

/** A file played by the system's [MediaPlayer], as speech. */
class MediaSound(path: String, onEnd: () -> Unit) : Sound {
    private val player = MediaPlayer()

    init {
        try {
            player.setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            player.setDataSource(path)
            player.prepare()
        } catch (error: Exception) {
            player.release()
            throw error
        }
        player.setOnCompletionListener { onEnd() }
    }

    override val durationMs get() = runCatching { player.duration }.getOrDefault(0).coerceAtLeast(0)
    override val positionMs get() = runCatching { player.currentPosition }.getOrDefault(0).coerceAtLeast(0)
    override val playing get() = runCatching { player.isPlaying }.getOrDefault(false)

    override fun play() = player.start()

    override fun pause() {
        if (player.isPlaying) player.pause()
    }

    override fun seekTo(ms: Int) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            player.seekTo(ms.toLong(), MediaPlayer.SEEK_CLOSEST)
        } else {
            player.seekTo(ms)
        }
    }

    override fun release() = player.release()
}

/**
 * factory/audio and factory/audio/events on Android: [AudioCore] with the
 * system's player. While a recording is open it holds the audio focus,
 * as the iPhone's playback session does: other apps' sound pauses and
 * resumes after. A call taking the focus, headphones unplugged, or the
 * app leaving the screen pauses it.
 */
class AudioChannel(
    messenger: BinaryMessenger,
    private val context: Context,
) : MethodChannel.MethodCallHandler, EventChannel.StreamHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/audio")
    private val events = EventChannel(messenger, "factory/audio/events")
    private val main = Handler(Looper.getMainLooper())
    private var sink: EventChannel.EventSink? = null
    private val audio = context.getSystemService(Context.AUDIO_SERVICE) as AudioManager
    private var focus: AudioFocusRequest? = null
    private val focusChanged = AudioManager.OnAudioFocusChangeListener { change ->
        if (change < 0) core.interrupt()
    }
    private val noisy = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) core.interrupt()
        }
    }
    private var listening = false

    val core = AudioCore(
        open = { path, onEnd -> MediaSound(path) { main.post(onEnd) } },
        ticker = MainTicker(main),
        emit = { event -> sink?.success(event) },
        active = { on -> if (on) hold() else letGo() },
    )

    init {
        channel.setMethodCallHandler(this)
        events.setStreamHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "play" -> {
                val arguments = call.arguments as? Map<*, *>
                val path = arguments?.get("path") as? String ?: return result.error("failed", "No file to play.", null)
                val from = (arguments["fromMs"] as? Number)?.toInt() ?: 0
                try {
                    core.play(path, from)
                    result.success(null)
                } catch (refused: AudioCore.Refused) {
                    result.error("failed", refused.message, null)
                }
            }
            "pause" -> {
                core.pause()
                result.success(null)
            }
            "seek" -> {
                (call.arguments as? Number)?.let { core.seek(it.toInt()) }
                result.success(null)
            }
            "stop" -> {
                core.stop()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    /** Away from the screen it pauses, as on the iPhone. */
    override fun onPause() = core.interrupt()

    private fun hold() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            if (focus == null) {
                val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
                    .setAudioAttributes(
                        AudioAttributes.Builder()
                            .setUsage(AudioAttributes.USAGE_MEDIA)
                            .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                            .build(),
                    )
                    // Speech isn't ducked under something else: it pauses.
                    .setWillPauseWhenDucked(true)
                    .setOnAudioFocusChangeListener(focusChanged, main)
                    .build()
                focus = request
                audio.requestAudioFocus(request)
            }
        } else {
            @Suppress("DEPRECATION")
            audio.requestAudioFocus(focusChanged, AudioManager.STREAM_MUSIC, AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
        }
        if (!listening) {
            // Only the system sends it; before Android 13 androidx's
            // permission keeps every other app out too, as ShareSheet's.
            ContextCompat.registerReceiver(
                context,
                noisy,
                IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY),
                ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            listening = true
        }
    }

    private fun letGo() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            focus?.let(audio::abandonAudioFocusRequest)
            focus = null
        } else {
            @Suppress("DEPRECATION")
            audio.abandonAudioFocus(focusChanged)
        }
        if (listening) {
            runCatching { context.unregisterReceiver(noisy) }
            listening = false
        }
    }

    override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
        sink = events
    }

    override fun onCancel(arguments: Any?) {
        sink = null
    }

    override fun close() {
        core.stop()
        letGo()
        channel.setMethodCallHandler(null)
        events.setStreamHandler(null)
    }
}
