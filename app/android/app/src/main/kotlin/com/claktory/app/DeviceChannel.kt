package com.claktory.app

import android.content.Context
import android.content.res.Configuration
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * factory/device, as on the iPhone (ios/Runner/AppDelegate.swift): what
 * this phone keeps (connections, drafts) and what it is (capabilities).
 * The stores read and write off the main thread, one call at a time, and
 * answer on it. The attach menu and its pickers are [AttachmentPicker]'s,
 * the camera's permission [CameraAccess]'s, QR codes [QrCodes]', web
 * sign-in [WebAuth]'s, the share sheet [ShareSheet]'s.
 */
class DeviceChannel(
    messenger: BinaryMessenger,
    private val context: Context,
    private val attachments: AttachmentPicker? = null,
    private val camera: CameraAccess? = null,
    private val webAuth: WebAuth? = null,
    private val share: ShareSheet? = null,
) :
    MethodChannel.MethodCallHandler, AutoCloseable {
    private val channel = MethodChannel(messenger, "factory/device")
    private val main = Handler(Looper.getMainLooper())

    init {
        channel.setMethodCallHandler(this)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "connections" -> stored(result) { DeviceStores.credentials(context).read() }
            "setConnections" -> stored(result) {
                DeviceStores.credentials(context).write(call.arguments as? Map<*, *> ?: emptyMap<String, Any>())
                null
            }
            // Only when the person chooses Start over.
            "resetConnections" -> stored(result) {
                DeviceStores.credentials(context).reset()
                null
            }
            "drafts" -> stored(result) { DeviceStores.drafts(context).read() }
            "setDrafts" -> stored(result) {
                DeviceStores.drafts(context).write(call.arguments as? Map<*, *> ?: emptyMap<String, String>())
                null
            }
            "capabilities" -> result.success(capabilities(context.resources.configuration))
            "cameraAccess" -> result.success(camera?.access() ?: "unavailable")
            "requestCamera" -> camera?.request(result) ?: result.success(false)
            "openCameraSettings" -> {
                camera?.openSettings()
                result.success(null)
            }
            "qrCode" -> result.success(QrCodes.modules(call.arguments as? String ?: ""))
            "pickAttachments" -> attachments?.pick(result) ?: result.notImplemented()
            "webAuth" -> webAuth?.core?.start(
                (call.arguments as? Map<*, *>)?.get("url") as? String,
                (call.arguments as? Map<*, *>)?.get("scheme") as? String,
                result,
            ) ?: result.notImplemented()
            "share" -> share?.core?.share((call.arguments as? Map<*, *>)?.get("text") as? String, result)
                ?: result.notImplemented()
            "cancelWebAuth" -> {
                webAuth?.core?.cancel()
                result.success(null)
            }
            else -> result.notImplemented()
        }
    }

    override fun close() = channel.setMethodCallHandler(null)

    /** [body] on the stores' thread, its answer or [StoreFailure] back on the main one. */
    private fun stored(result: MethodChannel.Result, body: () -> Any?) {
        DeviceStores.thread.execute {
            val answer = try {
                Result.success(body())
            } catch (error: Throwable) {
                Result.failure(error)
            }
            main.post {
                answer.fold(
                    onSuccess = { result.success(it) },
                    onFailure = { error ->
                        // Codes and categories only: nothing kept is ever in a message.
                        if (error is StoreFailure) result.error(error.code, error.message, null)
                        else result.error(StoreFailure.UNAVAILABLE, error.javaClass.simpleName, null)
                    },
                )
            }
        }
    }

    companion object {
        /**
         * The same map on every platform (app/lib/mobile/device.dart, the
         * fixtures in app/test/ipad/fixtures/bridge): a phone or, with a
         * smallest width of 600 dp or more, a tablet ("pad"); no Pencil and
         * none of the iPad's document features.
         */
        fun capabilities(configuration: Configuration, osVersion: String = Build.VERSION.RELEASE ?: ""): Map<String, Any> =
            mapOf(
                "schema_version" to 1,
                "idiom" to if (configuration.smallestScreenWidthDp >= 600) "pad" else "phone",
                "ios_app_on_mac" to false,
                "os_version" to osVersion,
                "hardware_keyboard" to (
                    configuration.keyboard == Configuration.KEYBOARD_QWERTY &&
                        configuration.hardKeyboardHidden == Configuration.HARDKEYBOARDHIDDEN_NO
                    ),
                "pencil" to mapOf("hover" to false, "squeeze" to false, "double_tap" to false),
                "pdf_page_overlay" to mapOf("available" to false, "min_os" to ""),
                "stroke_recognizer" to mapOf("available" to false, "min_os" to ""),
            )
    }
}

/**
 * The stores, one each for the whole process (not per engine), so a
 * failed read keeps writes refused for as long as the process lives.
 */
object DeviceStores {
    val thread: ExecutorService = Executors.newSingleThreadExecutor { Thread(it, "Claktory stores") }

    private var credentials: CredentialStore? = null
    private var drafts: DraftStore? = null
    private var dmKeys: DmKeyStore? = null

    @Synchronized
    fun credentials(context: Context): CredentialStore = credentials ?: run {
        val log = { message: String -> Log.i(TAG, "connections: $message"); Unit }
        CredentialStore(
            File(context.noBackupFilesDir, CredentialStore.FILE_NAME),
            context.packageName,
            KeystoreSealer(context.applicationContext, log = log),
            log,
        ).also { credentials = it }
    }

    @Synchronized
    fun drafts(context: Context): DraftStore = drafts ?: DraftStore(
        File(context.noBackupFilesDir, DraftStore.FILE_NAME),
    ) { Log.i(TAG, it); Unit }.also { drafts = it }

    /**
     * The DM keys' own Keystore key: apart from the connections', so
     * starting over there never makes them unreadable.
     */
    const val DM_KEY_ALIAS = "claktory.dm.v1"

    @Synchronized
    fun dmKeys(context: Context): DmKeyStore = dmKeys ?: run {
        val log = { message: String -> Log.i(TAG, "dm keys: $message"); Unit }
        DmKeyStore(
            File(context.noBackupFilesDir, DmKeyStore.FOLDER),
            context.packageName,
            KeystoreSealer(context.applicationContext, DM_KEY_ALIAS, log),
            log,
        ).also { dmKeys = it }
    }

    private const val TAG = "Claktory"
}
