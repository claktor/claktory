package com.claktory.app

import android.media.AudioAttributes
import android.media.MediaMetadataRetriever
import android.media.MediaPlayer
import android.os.Build
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import io.flutter.view.TextureRegistry

/**
 * How a video's frame is shown: its size in pixels before it's turned,
 * and the quarter turns clockwise the app turns it by, as the iPhone
 * reports them. A texture that already draws frames turned ([turned])
 * needs no turn from the app: its size is the turned one.
 */
data class Frame(val width: Int, val height: Int, val quarterTurns: Int) {
    companion object {
        fun of(width: Int, height: Int, rotation: Int, turned: Boolean): Frame {
            val turns = (Math.round(rotation / 90.0).toInt() % 4 + 4) % 4
            if (!turned) return Frame(width, height, turns)
            return if (turns % 2 == 1) Frame(height, width, 0) else Frame(width, height, 0)
        }
    }
}

/**
 * The app's video viewer's players (app/lib/viewers/video.dart), as on
 * the iPhone (app/macos/Runner/VideoPlayers.swift): each video is a
 * Flutter texture the system's [MediaPlayer] draws into. [open] answers
 * the new player's state with its texture once the file is ready to play,
 * or why it can't be; then play, pause, seek and close by texture, and
 * [changed] gets each player's state, with its texture, whenever it
 * plays, pauses, moves or ends: {playing, position, duration (seconds),
 * width, height, quarterTurns}.
 */
class VideoPlayers(
    private val textures: TextureRegistry,
    private val changed: (Map<String, Any>) -> Unit,
) : AutoCloseable {
    private val players = mutableMapOf<Long, VideoPlayer>()

    /** Calls back once: the state with its texture, or why it can't play. */
    fun open(path: String, done: (Map<String, Any>?, String?) -> Unit) {
        val producer = textures.createSurfaceProducer()
        val texture = producer.id()
        val player = VideoPlayer(producer) { state -> changed(state + ("texture" to texture)) }
        players[texture] = player
        player.load(path) { error ->
            if (error != null) {
                players.remove(texture)?.close()
                done(null, error)
            } else {
                done(player.state() + ("texture" to texture), null)
            }
        }
    }

    fun play(texture: Long) = players[texture]?.play()

    fun pause(texture: Long) = players[texture]?.pause()

    fun seek(texture: Long, seconds: Double, done: () -> Unit) = players[texture]?.seek(seconds, done) ?: done()

    fun close(texture: Long) = players.remove(texture)?.close()

    /** The app left the screen: whatever plays pauses, as on the iPhone. */
    fun pauseAll() = players.values.forEach { it.pause() }

    override fun close() {
        players.values.forEach { it.close() }
        players.clear()
    }
}

/** One video: a [MediaPlayer] drawing into a Flutter texture. */
class VideoPlayer(
    private val producer: TextureRegistry.SurfaceProducer,
    private val changed: (Map<String, Any>) -> Unit,
) {
    private val main = Handler(Looper.getMainLooper())
    private val player = MediaPlayer()
    private val ticker = MainTicker(main)
    private var ready = false
    private var closed = false

    /** Played to its end: it stays there, as the iPhone's does, until played or moved. */
    private var atEnd = false
    private var frame = Frame(0, 0, 0)
    private val seeking = mutableListOf<() -> Unit>()

    /** Calls back once with null when ready to play, or with why it can't be. */
    fun load(path: String, done: (String?) -> Unit) {
        var answered = false
        fun answer(error: String?) {
            if (answered) return
            answered = true
            done(error)
        }
        player.setOnErrorListener { _, _, _ ->
            if (!ready) answer(CANT_PLAY) else report()
            true
        }
        player.setOnPreparedListener {
            if (closed) return@setOnPreparedListener
            ready = true
            frame = frameOf(path)
            producer.setSize(frame.width.coerceAtLeast(1), frame.height.coerceAtLeast(1))
            // The first frame, before anything plays.
            seek(0.0) {}
            answer(null)
        }
        player.setOnCompletionListener {
            atEnd = true
            ticker.stop()
            report()
        }
        player.setOnSeekCompleteListener {
            report()
            val waiting = seeking.toList()
            seeking.clear()
            waiting.forEach { it() }
        }
        producer.setCallback(
            object : TextureRegistry.SurfaceProducer.Callback {
                override fun onSurfaceAvailable() {
                    if (!closed) runCatching { player.setSurface(producer.surface) }
                }

                override fun onSurfaceCleanup() {
                    if (!closed) runCatching { player.setSurface(null) }
                }
            },
        )
        try {
            player.setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                    .build(),
            )
            player.setDataSource(path)
            player.setSurface(producer.surface)
            player.prepareAsync()
        } catch (error: Exception) {
            answer(CANT_PLAY)
        }
    }

    /** Its size and turn: the file's own, as the player draws it into [producer]. */
    private fun frameOf(path: String): Frame {
        val retriever = MediaMetadataRetriever()
        return try {
            retriever.setDataSource(path)
            fun number(key: Int) = retriever.extractMetadata(key)?.toIntOrNull()
            Frame.of(
                number(MediaMetadataRetriever.METADATA_KEY_VIDEO_WIDTH) ?: player.videoWidth,
                number(MediaMetadataRetriever.METADATA_KEY_VIDEO_HEIGHT) ?: player.videoHeight,
                number(MediaMetadataRetriever.METADATA_KEY_VIDEO_ROTATION) ?: 0,
                producer.handlesCropAndRotation(),
            )
        } catch (error: Exception) {
            Frame(player.videoWidth, player.videoHeight, 0)
        } finally {
            runCatching { retriever.release() }
        }
    }

    fun play() {
        if (!ready || closed) return
        // From the start again once it's played to the end.
        if (atEnd || (duration() > 0 && position() >= duration())) seekTo(0.0)
        atEnd = false
        runCatching { player.start() }
        ticker.start(TICK_MS) { report() }
        report()
    }

    fun pause() {
        if (!ready || closed) return
        runCatching { if (player.isPlaying) player.pause() }
        ticker.stop()
        report()
    }

    fun seek(seconds: Double, done: () -> Unit) {
        if (!ready || closed) return done()
        seeking += done
        atEnd = false
        seekTo(seconds)
    }

    private fun seekTo(seconds: Double) {
        val ms = (seconds.coerceAtLeast(0.0) * 1000).toLong()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            player.seekTo(ms, MediaPlayer.SEEK_CLOSEST)
        } else {
            player.seekTo(ms.toInt())
        }
    }

    fun close() {
        if (closed) return
        closed = true
        ticker.stop()
        runCatching { player.setSurface(null) }
        player.release()
        producer.release()
        val waiting = seeking.toList()
        seeking.clear()
        waiting.forEach { it() }
    }

    private fun report() {
        if (!closed && ready) changed(state())
    }

    private fun position() =
        if (atEnd) duration() else runCatching { player.currentPosition }.getOrDefault(0).coerceAtLeast(0) / 1000.0

    private fun duration() = runCatching { player.duration }.getOrDefault(0).coerceAtLeast(0) / 1000.0

    /** What the app's viewer shows: whether it's playing, where, how long it is, and the frame's size and turn. */
    fun state(): Map<String, Any> = mapOf(
        "playing" to (!closed && runCatching { player.isPlaying }.getOrDefault(false)),
        "position" to position(),
        "duration" to duration(),
        "width" to frame.width.toDouble(),
        "height" to frame.height.toDouble(),
        "quarterTurns" to frame.quarterTurns,
    )

    companion object {
        const val TICK_MS = 100L
        const val CANT_PLAY = "Claktory can't play this video."
    }
}

/** claktory/video on Android: [VideoPlayers] on the engine's textures. */
class VideoChannel(
    messenger: BinaryMessenger,
    textures: TextureRegistry,
) : MethodChannel.MethodCallHandler, HostLifecycle, AutoCloseable {
    private val channel = MethodChannel(messenger, "claktory/video")
    val players = VideoPlayers(textures) { state -> channel.invokeMethod("state", state) }

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        val arguments = call.arguments as? Map<*, *> ?: emptyMap<String, Any>()
        if (call.method == "open") {
            val path = arguments["path"] as? String ?: return result.error("arguments", "No file to play.", null)
            return players.open(path) { state, error ->
                if (error != null) result.error("failed", error, null) else result.success(state)
            }
        }
        val texture = (arguments["texture"] as? Number)?.toLong() ?: return result.success(null)
        when (call.method) {
            "play" -> players.play(texture)
            "pause" -> players.pause(texture)
            "seek" -> return players.seek(texture, (arguments["seconds"] as? Number)?.toDouble() ?: 0.0) {
                result.success(null)
            }
            "close" -> players.close(texture)
            else -> return result.notImplemented()
        }
        result.success(null)
    }

    override fun onPause() = players.pauseAll()

    override fun close() {
        players.close()
        channel.setMethodCallHandler(null)
    }
}
