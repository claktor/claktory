package com.claktory.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.admin.DevicePolicyManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.ImageFormat
import android.graphics.Matrix
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.ImageReader
import android.net.Uri
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.provider.Settings
import android.util.Size
import android.view.Surface
import android.view.TextureView
import android.view.View
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.EncodeHintType
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader
import com.google.zxing.qrcode.decoder.ErrorCorrectionLevel
import com.google.zxing.qrcode.encoder.Encoder
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.StandardMessageCodec
import io.flutter.plugin.platform.PlatformView
import io.flutter.plugin.platform.PlatformViewFactory
import java.nio.charset.StandardCharsets
import kotlin.math.max

/**
 * QR codes with ZXing, as the iPhone does with Core Image and Vision
 * (ios/Runner/AppDelegate.swift, Scanner.swift). Pure, so it's tested on
 * the JVM.
 */
object QrCodes {
    /** Longer than any pairing code by far: a QR code holding more is ignored, unparsed. */
    const val MAX_TEXT = 4096

    /**
     * [text] as a QR code: {size, modules}, its modules row by row, 1 for
     * dark, no quiet zone, error correction M, as the iPhone answers
     * factory/device's qrCode. Null if it doesn't fit in one.
     */
    fun modules(text: String): Map<String, Any>? {
        if (text.isEmpty()) return null
        // UTF-8 only when it isn't ASCII, so plain codes carry no ECI marker.
        val hints = if (text.all { it.code < 0x80 }) emptyMap() else mapOf(EncodeHintType.CHARACTER_SET to "UTF-8")
        val matrix = try {
            Encoder.encode(text, ErrorCorrectionLevel.M, hints).matrix
        } catch (error: Exception) {
            return null
        }
        val size = matrix.width
        val modules = ByteArray(size * size) { i -> if (matrix.get(i % size, i / size).toInt() == 1) 1 else 0 }
        return mapOf("size" to size, "modules" to modules)
    }

    /**
     * The text of the QR code in a frame's brightness ([luma], [rowStride]
     * bytes a row, the first [width] of them the picture's), or null if
     * there's none, or it's too long to be a pairing code.
     */
    fun read(luma: ByteArray, rowStride: Int, width: Int, height: Int): String? {
        val source = PlanarYUVLuminanceSource(luma, rowStride, height, 0, 0, width, height, false)
        val text = try {
            QRCodeReader().decode(
                BinaryBitmap(HybridBinarizer(source)),
                mapOf(
                    DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE),
                    DecodeHintType.CHARACTER_SET to StandardCharsets.UTF_8.name(),
                ),
            ).text
        } catch (error: ReaderException) {
            return null
        }
        return text?.takeIf { it.isNotEmpty() && it.length <= MAX_TEXT }
    }

    /**
     * How a camera's picture fills a view of [viewWidth]×[viewHeight],
     * cropped, never squashed: a TextureView stretches the camera's
     * [bufferWidth]×[bufferHeight] buffer, already turned upright for the
     * phone's natural orientation, over the whole view; this undoes the
     * stretch, turns it for the display's [displayDegrees] and scales it
     * to cover the view. [sensorDegrees] is how the camera's sensor is
     * turned against the phone.
     */
    fun fill(
        viewWidth: Float,
        viewHeight: Float,
        bufferWidth: Int,
        bufferHeight: Int,
        sensorDegrees: Int,
        displayDegrees: Int,
    ): Fill {
        val (naturalWidth, naturalHeight) = Upright.size(bufferWidth, bufferHeight, sensorDegrees)
        val (shownWidth, shownHeight) = Upright.size(naturalWidth.toInt(), naturalHeight.toInt(), displayDegrees)
        return Fill(
            naturalWidth / viewWidth,
            naturalHeight / viewHeight,
            -displayDegrees.toFloat(),
            max(viewWidth / shownWidth, viewHeight / shownHeight),
        )
    }

    /** The largest of [sizes] (width, height) within [maxWidth]×[maxHeight], of [like]'s shape if there's one. */
    fun largest(sizes: List<Pair<Int, Int>>, maxWidth: Int, maxHeight: Int, like: Pair<Int, Int>? = null): Pair<Int, Int>? {
        val fits = sizes.filter { it.first <= maxWidth && it.second <= maxHeight }
        val shaped = like?.let { l -> fits.filter { it.first.toLong() * l.second == it.second.toLong() * l.first } }
        return (shaped?.takeIf { it.isNotEmpty() } ?: fits).maxByOrNull { it.first.toLong() * it.second }
    }
}

/**
 * A view's transform, about its centre: scaled by [scaleX]×[scaleY],
 * turned [degrees], then scaled by [scale].
 */
data class Fill(val scaleX: Float, val scaleY: Float, val degrees: Float, val scale: Float) {
    fun apply(matrix: Matrix, cx: Float, cy: Float) {
        matrix.setScale(scaleX, scaleY, cx, cy)
        matrix.postRotate(degrees, cx, cy)
        matrix.postScale(scale, scale, cx, cy)
    }
}

/** A picture's size once turned by some degrees: swapped for a quarter or three-quarter turn. */
object Upright {
    fun size(width: Int, height: Int, degrees: Int): Pair<Float, Float> =
        if (((degrees % 360 + 360) % 360) % 180 == 90) height.toFloat() to width.toFloat()
        else width.toFloat() to height.toFloat()
}

/**
 * The camera's permission, for factory/device's cameraAccess,
 * requestCamera and openCameraSettings, answered as on the iPhone:
 * "granted"; "undetermined" while asking can still show the system's
 * question (never asked, or refused once: Android asks a second time);
 * "denied" once the person said no in this run of the app and Android
 * won't ask again, so only Settings changes it (Android doesn't tell
 * "never asked" from "won't ask again" before asking, and the app keeps
 * no record of asking: asked then, the system answers no at once, and
 * the app offers Settings); "restricted" when the phone's administrator
 * turned the camera off; "unavailable" with no camera.
 */
class CameraAccess(private val activity: Activity) : HostLifecycle, AutoCloseable {
    private var asking: MethodChannel.Result? = null
    private var refused = false

    fun access(): String = when {
        !activity.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY) -> "unavailable"
        granted(activity) -> "granted"
        (activity.getSystemService(Context.DEVICE_POLICY_SERVICE) as? DevicePolicyManager)?.getCameraDisabled(null) == true -> "restricted"
        refused && !activity.shouldShowRequestPermissionRationale(Manifest.permission.CAMERA) -> "denied"
        else -> "undetermined"
    }

    fun request(result: MethodChannel.Result) {
        if (granted(activity)) return result.success(true)
        if (access() == "unavailable" || asking != null) return result.success(false)
        asking = result
        activity.requestPermissions(arrayOf(Manifest.permission.CAMERA), REQUEST)
    }

    fun openSettings() {
        runCatching {
            activity.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", activity.packageName, null)),
            )
        }
    }

    override fun onPermissionResult(requestCode: Int, permissions: Array<out String>, grants: IntArray): Boolean {
        if (requestCode != REQUEST) return false
        val allowed = grants.isNotEmpty() && grants.all { it == PackageManager.PERMISSION_GRANTED }
        if (!allowed) refused = true
        asking?.success(allowed)
        asking = null
        return true
    }

    override fun close() {
        asking?.success(false)
        asking = null
    }

    companion object {
        /** Ours alone among the activity's permission requests. */
        const val REQUEST = 0x4176

        fun granted(context: Context) =
            context.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
    }
}

/**
 * Makes the camera views the pairing screen embeds ("claktory/scanner"),
 * each telling Flutter on claktory/scanner/<id> each QR code it reads. The
 * camera is open only while a view shows and the app is on screen: the
 * app leaving the screen closes it, coming back opens it again.
 */
class ScannerFactory(private val messenger: BinaryMessenger) :
    PlatformViewFactory(StandardMessageCodec.INSTANCE), HostLifecycle, AutoCloseable {
    private val views = mutableSetOf<ScannerView>()

    override fun create(context: Context, viewId: Int, args: Any?): PlatformView {
        val channel = MethodChannel(messenger, "claktory/scanner/$viewId")
        return ScannerView(context, channel) { views.remove(it) }.also { views += it }
    }

    override fun onResume() = views.toList().forEach { it.resume() }

    override fun onPause() = views.toList().forEach { it.pause() }

    override fun close() = views.toList().forEach { it.dispose() }
}

/**
 * The camera's picture, filling the view, and the QR codes in it: the
 * back camera (or the only one) with Camera2, its preview drawn into a
 * TextureView and its frames' brightness read a few times a second, in
 * memory only. Each code read is sent ("scanned", its text) once until
 * another is read. Nothing it reads is logged.
 */
class ScannerView(
    context: Context,
    private val channel: MethodChannel,
    private val disposed: (ScannerView) -> Unit,
) : PlatformView {
    private val main = Handler(Looper.getMainLooper())
    private val texture = TextureView(context)
    private val manager = context.getSystemService(Context.CAMERA_SERVICE) as CameraManager
    private var thread: HandlerThread? = null
    private var handler: Handler? = null
    @Volatile private var device: CameraDevice? = null
    @Volatile private var session: CameraCaptureSession? = null
    @Volatile private var frames: ImageReader? = null
    private var preview: Surface? = null
    private var bufferSize = Size(0, 0)
    private var sensorDegrees = 0
    private var paused = false
    private var gone = false

    /** Read on the camera's thread only. */
    private var last = ""
    private var lastRead = 0L

    /** Frames read, for tests. */
    @Volatile
    var framesRead = 0
        private set

    init {
        texture.surfaceTextureListener = object : TextureView.SurfaceTextureListener {
            override fun onSurfaceTextureAvailable(surface: SurfaceTexture, width: Int, height: Int) = open()

            override fun onSurfaceTextureSizeChanged(surface: SurfaceTexture, width: Int, height: Int) = fit()

            override fun onSurfaceTextureDestroyed(surface: SurfaceTexture): Boolean {
                shut()
                return true
            }

            override fun onSurfaceTextureUpdated(surface: SurfaceTexture) {}
        }
        texture.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> fit() }
    }

    override fun getView(): View = texture

    override fun dispose() {
        if (gone) return
        gone = true
        shut()
        channel.setMethodCallHandler(null)
        disposed(this)
    }

    fun pause() {
        paused = true
        shut()
    }

    fun resume() {
        paused = false
        if (texture.isAvailable) open()
    }

    @SuppressLint("MissingPermission")
    private fun open() {
        if (gone || paused || device != null || !texture.isAvailable) return
        if (!CameraAccess.granted(texture.context)) return
        val id = runCatching { camera() }.getOrNull() ?: return
        val characteristics = manager.getCameraCharacteristics(id)
        val sizes = characteristics.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) ?: return
        sensorDegrees = characteristics.get(CameraCharacteristics.SENSOR_ORIENTATION) ?: 0
        fun pairs(all: Array<Size>?) = all.orEmpty().map { it.width to it.height }
        val (shownWidth, shownHeight) = QrCodes.largest(pairs(sizes.getOutputSizes(SurfaceTexture::class.java)), 1920, 1080) ?: return
        val (readWidth, readHeight) =
            QrCodes.largest(pairs(sizes.getOutputSizes(ImageFormat.YUV_420_888)), 1280, 720, like = shownWidth to shownHeight) ?: return
        val shown = Size(shownWidth, shownHeight)
        val read = Size(readWidth, readHeight)
        bufferSize = shown
        val thread = HandlerThread("Claktory scanner").also { it.start() }
        val handler = Handler(thread.looper)
        this.thread = thread
        this.handler = handler
        val frames = ImageReader.newInstance(read.width, read.height, ImageFormat.YUV_420_888, 2)
        frames.setOnImageAvailableListener({ reader -> analyze(reader) }, handler)
        this.frames = frames
        texture.surfaceTexture?.setDefaultBufferSize(shown.width, shown.height)
        val preview = Surface(texture.surfaceTexture)
        this.preview = preview
        fit()
        try {
            manager.openCamera(
                id,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(camera: CameraDevice) {
                        if (this@ScannerView.frames !== frames) return camera.close()
                        device = camera
                        start(camera, preview, frames, handler)
                    }

                    override fun onDisconnected(camera: CameraDevice) {
                        camera.close()
                        if (device === camera) device = null
                    }

                    override fun onError(camera: CameraDevice, error: Int) {
                        camera.close()
                        if (device === camera) device = null
                    }
                },
                handler,
            )
        } catch (error: Exception) {
            shut()
        }
    }

    @Suppress("DEPRECATION")
    private fun start(camera: CameraDevice, preview: Surface, frames: ImageReader, handler: Handler) {
        try {
            camera.createCaptureSession(
                listOf(preview, frames.surface),
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(session: CameraCaptureSession) {
                        if (device !== camera) return session.close()
                        this@ScannerView.session = session
                        val request = camera.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                            addTarget(preview)
                            addTarget(frames.surface)
                            set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
                        }
                        runCatching { session.setRepeatingRequest(request.build(), null, handler) }
                    }

                    override fun onConfigureFailed(session: CameraCaptureSession) {
                        session.close()
                    }
                },
                handler,
            )
        } catch (error: Exception) {
            main.post { shut() }
        }
    }

    /** On the camera's thread: a few reads a second are plenty, and spare the battery. */
    private fun analyze(reader: ImageReader) {
        val image = runCatching { reader.acquireLatestImage() }.getOrNull() ?: return
        try {
            val now = System.nanoTime()
            if (now - lastRead < READ_EVERY_NS) return
            lastRead = now
            val plane = image.planes[0]
            val buffer = plane.buffer
            val luma = ByteArray(plane.rowStride * image.height)
            buffer.get(luma, 0, minOf(buffer.remaining(), luma.size))
            framesRead++
            val code = QrCodes.read(luma, plane.rowStride, image.width, image.height) ?: return
            if (code == last) return
            last = code
            main.post { if (!gone) channel.invokeMethod("scanned", code) }
        } finally {
            image.close()
        }
    }

    /** Closes the camera, its session and its thread. */
    private fun shut() {
        session?.let { runCatching { it.close() } }
        session = null
        device?.let { runCatching { it.close() } }
        device = null
        frames?.let { runCatching { it.close() } }
        frames = null
        preview?.release()
        preview = null
        thread?.quitSafely()
        thread = null
        handler = null
    }

    private fun fit() {
        if (bufferSize.width == 0 || texture.width == 0 || texture.height == 0) return
        val rotation = when (texture.display?.rotation ?: Surface.ROTATION_0) {
            Surface.ROTATION_90 -> 90
            Surface.ROTATION_180 -> 180
            Surface.ROTATION_270 -> 270
            else -> 0
        }
        val matrix = Matrix()
        QrCodes.fill(
            texture.width.toFloat(),
            texture.height.toFloat(),
            bufferSize.width,
            bufferSize.height,
            sensorDegrees,
            rotation,
        ).apply(matrix, texture.width / 2f, texture.height / 2f)
        texture.setTransform(matrix)
    }

    /** The back camera, or the only one there is. */
    private fun camera(): String? {
        val ids = manager.cameraIdList
        return ids.firstOrNull {
            manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK
        } ?: ids.firstOrNull()
    }

    companion object {
        const val READ_EVERY_NS = 200_000_000L
    }
}
