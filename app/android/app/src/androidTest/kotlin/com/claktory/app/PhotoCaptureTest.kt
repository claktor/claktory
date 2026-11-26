package com.claktory.app

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.Instrumentation
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.ExifInterface
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import io.flutter.plugin.common.MethodChannel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File
import java.io.OutputStream
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Take photo on a phone: the attach menu offers it first only when asked
 * to, and choosing it hands the choice back to the app (take_photo);
 * takePhoto starts the camera app with a write grant for one address of
 * CameraFiles, and what the camera app wrote there becomes an attachment
 * without its location. The camera app is stood in for by an activity
 * monitor that writes a photo through the address it was given, as a
 * camera app does; the real camera app's shutter is checked by hand.
 */
@RunWith(AndroidJUnit4::class)
class PhotoCaptureTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = instrumentation.targetContext

    private class Answer : MethodChannel.Result {
        val done = CountDownLatch(1)
        var value: Any? = null
        var code: String? = null

        override fun success(result: Any?) {
            value = result
            done.countDown()
        }

        override fun error(errorCode: String, errorMessage: String?, errorDetails: Any?) {
            code = errorCode
            done.countDown()
        }

        override fun notImplemented() {
            code = "not_implemented"
            done.countDown()
        }

        fun await(): Answer = apply { assertTrue("no answer", done.await(10, TimeUnit.SECONDS)) }
    }

    /** A small JPEG, tagged with where it was taken as some camera apps tag theirs. */
    private fun writeTaggedPhoto(file: File) {
        file.outputStream().use { jpeg(it) }
        ExifInterface(file.path).apply {
            setAttribute(ExifInterface.TAG_MAKE, "Claktory test")
            setAttribute("GPSLatitude", "51/1,30/1,0/1")
            setAttribute("GPSLatitudeRef", "N")
            setAttribute("GPSLongitude", "0/1,7/1,0/1")
            setAttribute("GPSLongitudeRef", "W")
            setAttribute("GPSAltitude", "12/1")
            setAttribute("GPSAltitudeRef", "0")
            setAttribute("GPSDateStamp", "2026:05:11")
            saveAttributes()
        }
        assertTrue("the fixture has a location", ExifInterface(file.path).getLatLong(FloatArray(2)))
    }

    private fun jpeg(out: OutputStream) {
        val bitmap = Bitmap.createBitmap(64, 48, Bitmap.Config.ARGB_8888).apply { eraseColor(0xFF3366CC.toInt()) }
        bitmap.compress(Bitmap.CompressFormat.JPEG, 90, out)
        bitmap.recycle()
    }

    private fun assertNoLocation(path: String) {
        val exif = ExifInterface(path)
        PhotoCapture.LOCATION_TAGS.forEach { assertNull("$it left in", exif.getAttribute(it)) }
        assertEquals("Claktory test", exif.getAttribute(ExifInterface.TAG_MAKE))
        assertNotNull("still a picture", BitmapFactory.decodeFile(path))
    }

    private val captures get() = File(context.cacheDir, PhotoCapture.FOLDER)

    /** Files a run that failed half-way left (in use, the next day's sweep takes them). */
    @Before
    fun setUp() {
        captures.listFiles()?.forEach { it.deleteRecursively() }
    }

    /**
     * Stands in for the camera app: [answer] decides what it does with the
     * address it's handed (write a photo, or nothing) and how it returns.
     */
    @SuppressLint("NewApi") // the assumption below skips before Android 8
    private fun <T> camera(answer: (Intent) -> Int, body: () -> T): T {
        assumeTrue("ActivityMonitor() and onStartActivity (Android 8+)", Build.VERSION.SDK_INT >= Build.VERSION_CODES.O)
        // A monitor made without a filter is asked of every start, and the
        // result it gives is delivered to the activity as the camera app's.
        val monitor = object : Instrumentation.ActivityMonitor() {
            override fun onStartActivity(intent: Intent): Instrumentation.ActivityResult? =
                if (intent.action == MediaStore.ACTION_IMAGE_CAPTURE) Instrumentation.ActivityResult(answer(intent), null) else null
        }
        instrumentation.addMonitor(monitor)
        try {
            return body()
        } finally {
            instrumentation.removeMonitor(monitor)
        }
    }

    private fun photoCapture(screen: Screen): PhotoCapture {
        var found: PhotoCapture? = null
        screen.onActivity { found = it.adapter(PhotoCapture::class.java) }
        return found ?: throw AssertionError("no PhotoCapture adapter")
    }

    private fun assumeCamera(screen: Screen) {
        assumeTrue("a camera and a camera app", photoCapture(screen).usable())
    }

    @Test
    fun locationTagsAreRemovedAndTheRestKept() {
        val file = File(context.cacheDir, "exif-test.jpg")
        try {
            writeTaggedPhoto(file)
            PhotoCapture.withoutLocation(file)
            assertNoLocation(file.path)
        } finally {
            file.delete()
        }
    }

    @Test
    fun theMenuOffersTakePhotoFirstOnlyWhenAskedAndHandsTheChoiceBack() {
        Screen.launch().use { screen ->
            assumeCamera(screen)
            var picker: AttachmentPicker? = null
            screen.onActivity { picker = it.adapter(AttachmentPicker::class.java) }
            val plain = Answer()
            var items = emptyList<String>()
            screen.onActivity {
                picker!!.pick(plain, null)
                items = picker!!.menuItemsForTest()
                picker!!.dismissMenuForTest()
                picker!!.close()
            }
            assertEquals("Choose photos or videos", items.first())
            assertTrue(items.none { it == "Take photo" })

            // A fresh picker: close ended the one above.
            val fresh = AttachmentPicker(screen.activity, camera = photoCapture(screen))
            val chosen = Answer()
            screen.onActivity {
                fresh.pick(chosen, mapOf("camera" to true))
                items = fresh.menuItemsForTest()
                fresh.chooseForTest(0)
            }
            assertEquals(listOf("Take photo", "Choose photos or videos", "Choose files"), items.take(3))
            assertEquals("take_photo", chosen.await().code)
            fresh.close()
        }
    }

    @Test
    fun aPhotoTakenBecomesAnAttachmentWithoutItsLocation() {
        grant(Manifest.permission.CAMERA)
        Screen.launch().use { screen ->
            assumeCamera(screen)
            var handed: Intent? = null
            val answer = Answer()
            camera(
                answer = { intent ->
                    handed = intent
                    // What a camera app does with the address: write the photo there.
                    @Suppress("DEPRECATION")
                    val uri = intent.getParcelableExtra<Uri>(MediaStore.EXTRA_OUTPUT)!!
                    val staged = File(context.cacheDir, "staged.jpg").apply { writeTaggedPhoto(this) }
                    context.contentResolver.openOutputStream(uri)!!.use { out -> staged.inputStream().use { it.copyTo(out) } }
                    staged.delete()
                    Activity.RESULT_OK
                },
            ) {
                val capture = photoCapture(screen)
                screen.onActivity { capture.take(answer) }
                answer.await()
            }
            val intent = handed ?: throw AssertionError("the camera app wasn't started")
            @Suppress("DEPRECATION")
            val uri = intent.getParcelableExtra<Uri>(MediaStore.EXTRA_OUTPUT)!!
            assertEquals("${context.packageName}.camera", uri.authority)
            assertTrue("a write grant", intent.flags and Intent.FLAG_GRANT_WRITE_URI_PERMISSION != 0)
            assertEquals(uri, intent.clipData?.getItemAt(0)?.uri)

            assertNull(answer.code)
            @Suppress("UNCHECKED_CAST")
            val paths = answer.value as List<String>
            assertEquals(1, paths.size)
            val photo = File(paths.single())
            assertTrue(photo.path.startsWith(File(context.cacheDir, AttachmentCopies.FOLDER).path))
            assertTrue(photo.name, Regex("""Photo \d{4}-\d\d-\d\d at \d\d\.\d\d\.\d\d\.jpg""").matches(photo.name))
            assertNoLocation(photo.path)
            assertTrue("the camera's file is gone", captures.listFiles().orEmpty().isEmpty())
            photo.parentFile?.deleteRecursively()
        }
    }

    @Test
    fun aCameraClosedWithoutAPhotoAnswersNothingAndLeavesNothing() {
        grant(Manifest.permission.CAMERA)
        Screen.launch().use { screen ->
            assumeCamera(screen)
            for (result in listOf(Activity.RESULT_CANCELED, Activity.RESULT_OK)) {
                // Cancelled, and a camera app that says OK but wrote nothing.
                val answer = Answer()
                val capture = photoCapture(screen)
                camera(answer = { result }) {
                    screen.onActivity { capture.take(answer) }
                    answer.await()
                }
                assertEquals(emptyList<String>(), answer.value)
                assertTrue(captures.listFiles().orEmpty().isEmpty())
            }
        }
    }

    @Test
    fun aSecondTakeWhileTheCameraIsOpenAnswersNothing() {
        grant(Manifest.permission.CAMERA)
        Screen.launch().use { screen ->
            assumeCamera(screen)
            val first = Answer()
            val second = Answer()
            camera(answer = { Activity.RESULT_CANCELED }) {
                val capture = photoCapture(screen)
                screen.onActivity {
                    capture.take(first)
                    capture.take(second)
                }
                assertEquals(emptyList<String>(), second.await().value)
                first.await()
            }
        }
    }

    /**
     * The phone's real camera app, by hand: run with `-e camera_by_hand
     * true`, then take a photo and confirm it within two minutes. Skipped
     * otherwise.
     */
    @Test
    fun byHandTheRealCameraAppsPhotoBecomesAnAttachment() {
        assumeTrue("-e camera_by_hand true", InstrumentationRegistry.getArguments().getString("camera_by_hand") == "true")
        grant(Manifest.permission.CAMERA)
        Screen.launch().use { screen ->
            assumeCamera(screen)
            val answer = Answer()
            val capture = photoCapture(screen)
            screen.onActivity { capture.take(answer) }
            assertTrue("no photo within 2 min", answer.done.await(120, TimeUnit.SECONDS))
            assertNull(answer.code)
            @Suppress("UNCHECKED_CAST")
            val photo = File((answer.value as List<String>).single())
            android.util.Log.i("PhotoCaptureTest", "by hand: ${photo.path} ${photo.length()} bytes")
            assertNotNull("a picture", BitmapFactory.decodeFile(photo.path))
            PhotoCapture.LOCATION_TAGS.forEach { assertNull("$it left in", ExifInterface(photo.path).getAttribute(it)) }
            assertTrue(captures.listFiles().orEmpty().isEmpty())
        }
    }

    /** Only where the camera isn't allowed yet: revoking it would end this process. */
    @Test
    fun withoutTheCameraTakePhotoAnswersCameraDenied() {
        assumeTrue("the camera not allowed yet", !CameraAccess.granted(context))
        Screen.launch().use { screen ->
            assumeCamera(screen)
            val answer = Answer()
            val capture = photoCapture(screen)
            screen.onActivity { capture.take(answer) }
            assertEquals("camera_denied", answer.await().code)
        }
    }
}
