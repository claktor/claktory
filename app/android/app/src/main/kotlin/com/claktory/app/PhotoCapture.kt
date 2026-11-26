package com.claktory.app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.ExifInterface
import android.os.Handler
import android.os.Looper
import android.provider.MediaStore
import androidx.core.content.FileProvider
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * factory/device's takePhoto, the attach menu's Take photo (Bruno 46437):
 * the phone's own camera app (an image capture intent) writes the photo
 * to a file of the app's cache it may write only through a grant for that
 * one address ([CameraFiles]); taken, its location tags are removed and it
 * becomes an attachment as a chosen photo does ([AttachmentCopies.adopt]),
 * its path answered. Cancelled, or a camera app that wrote nothing,
 * answers an empty list, and so does a second takePhoto while one is open.
 *
 * The app asks for the camera itself first (the shared permission sheet,
 * explained before the system's question): Android refuses an image
 * capture intent to an app that declares the camera permission without
 * holding it, so takePhoto without it answers camera_denied, and a phone
 * with no camera or no camera app camera_unavailable.
 *
 * Android may end the app's process while the camera app is on screen (a
 * phone short of memory); the photo then has no one to answer and its
 * file goes with the next day's sweep.
 */
class PhotoCapture(
    private val activity: Activity,
    private val copies: AttachmentCopies = AttachmentCopies(File(activity.cacheDir, AttachmentCopies.FOLDER)),
    private val captures: File = File(activity.cacheDir, FOLDER),
    private val worker: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "Claktory photo") },
) : HostLifecycle, AutoCloseable {
    private val main = Handler(Looper.getMainLooper())
    private var result: MethodChannel.Result? = null
    private var file: File? = null

    /** Whether this phone has a camera and an app to take a photo with it. */
    fun usable(): Boolean =
        activity.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY) &&
            Intent(MediaStore.ACTION_IMAGE_CAPTURE).resolveActivity(activity.packageManager) != null

    fun take(result: MethodChannel.Result) {
        if (this.result != null || activity.isFinishing) return result.success(emptyList<String>())
        if (!usable()) return result.error("camera_unavailable", "This phone has no camera to take a photo with.", null)
        if (!CameraAccess.granted(activity)) {
            return result.error("camera_denied", "Claktory isn't allowed to use the camera.", null)
        }
        val target = try {
            sweep(captures, System.currentTimeMillis())
            File(captures.apply { mkdirs() }, "${UUID.randomUUID()}.jpg").apply { createNewFile() }
        } catch (error: Exception) {
            return result.error("copy_failed", "The camera's file couldn't be made (${error.javaClass.simpleName}).", null)
        }
        val uri = FileProvider.getUriForFile(activity, CameraFiles.authority(activity), target)
        val intent = Intent(MediaStore.ACTION_IMAGE_CAPTURE)
            .putExtra(MediaStore.EXTRA_OUTPUT, uri)
            // The grant goes with the clip: the camera app may write this
            // one address, and nothing else of the app's.
            .addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION or Intent.FLAG_GRANT_READ_URI_PERMISSION)
        intent.clipData = ClipData.newRawUri("", uri)
        this.result = result
        file = target
        try {
            @Suppress("DEPRECATION")
            activity.startActivityForResult(intent, REQUEST)
        } catch (error: ActivityNotFoundException) {
            fail("camera_unavailable", "This phone has no camera app.")
        } catch (error: SecurityException) {
            fail("camera_denied", "Claktory isn't allowed to use the camera.")
        }
    }

    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?): Boolean {
        if (requestCode != REQUEST) return false
        val taken = file
        file = null
        // The camera app's own data is never read: only the file it was given.
        if (resultCode != Activity.RESULT_OK || taken == null || !taken.isFile || taken.length() == 0L) {
            taken?.delete()
            finish(emptyList())
            return true
        }
        val name = photoName(Date())
        worker.execute {
            val answer = try {
                withoutLocation(taken)
                Result.success(listOf(copies.adopt(taken, name)))
            } catch (error: AttachmentCopies.Refused) {
                Result.failure(error)
            } catch (error: Exception) {
                Result.failure(AttachmentCopies.Refused("copy_failed", "The photo couldn't be read (${error.javaClass.simpleName})."))
            } finally {
                taken.delete()
            }
            main.post {
                answer.fold(
                    onSuccess = { finish(it) },
                    onFailure = { error -> (error as AttachmentCopies.Refused).let { fail(it.code, it.message ?: "") } },
                )
            }
        }
        return true
    }

    private fun finish(paths: List<String>) {
        result?.success(paths)
        result = null
    }

    private fun fail(code: String, message: String) {
        file?.delete()
        file = null
        result?.error(code, message, null)
        result = null
    }

    override fun close() {
        finish(emptyList())
        worker.shutdown()
    }

    companion object {
        /** Ours alone among the activity's requests (MainActivity forwards every result). */
        const val REQUEST = 0x4178

        /** The folder under the cache the camera app writes to, and [CameraFiles] serves. */
        const val FOLDER = "claktory-camera"

        private const val DAY = 24L * 3_600_000

        /**
         * Every location tag a photo's EXIF can hold: the GPS directory's.
         * Some camera apps tag photos with where they were taken; a photo
         * sent to an agent carries none, as the iPhone's camera picture
         * doesn't.
         */
        val LOCATION_TAGS = listOf(
            "GPSVersionID", "GPSLatitudeRef", "GPSLatitude", "GPSLongitudeRef", "GPSLongitude",
            "GPSAltitudeRef", "GPSAltitude", "GPSTimeStamp", "GPSSatellites", "GPSStatus",
            "GPSMeasureMode", "GPSDOP", "GPSSpeedRef", "GPSSpeed", "GPSTrackRef", "GPSTrack",
            "GPSImgDirectionRef", "GPSImgDirection", "GPSMapDatum", "GPSDestLatitudeRef",
            "GPSDestLatitude", "GPSDestLongitudeRef", "GPSDestLongitude", "GPSDestBearingRef",
            "GPSDestBearing", "GPSDestDistanceRef", "GPSDestDistance", "GPSProcessingMethod",
            "GPSAreaInformation", "GPSDateStamp", "GPSDifferential", "GPSHPositioningError",
        )

        /**
         * Removes [LOCATION_TAGS] from the JPEG at [file], rewriting it only
         * when it had any. A file whose EXIF can't be read or written fails,
         * rather than going with its location.
         */
        fun withoutLocation(file: File) {
            val exif = ExifInterface(file.path)
            val present = LOCATION_TAGS.filter { exif.getAttribute(it) != null }
            if (present.isEmpty()) return
            present.forEach { exif.setAttribute(it, null) }
            exif.saveAttributes()
        }

        /** "Photo 2026-05-11 at 14.32.05.jpg", in the phone's time, as a Mac names a screenshot. */
        fun photoName(taken: Date): String =
            "Photo " + SimpleDateFormat("yyyy-MM-dd 'at' HH.mm.ss", Locale.US).format(taken) + ".jpg"

        /** Camera files over a day old: left by a process Android ended while the camera was open. */
        fun sweep(folder: File, now: Long) {
            folder.listFiles()?.filter { now - it.lastModified() > DAY }?.forEach { it.deleteRecursively() }
        }
    }
}

/**
 * The content provider the camera app writes a photo through: not
 * exported, reachable only through the write grant for one address that
 * takePhoto gives, and only for files in cache/claktory-camera
 * (res/xml/camera_paths.xml). Its own class and authority, so a camera
 * grant can never reach the files the app shares ([ShareFiles]).
 */
class CameraFiles : FileProvider() {
    companion object {
        fun authority(context: Context) = "${context.packageName}.camera"
    }
}
