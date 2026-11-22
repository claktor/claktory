package com.claktory.app

import android.app.Application
import android.util.Log

/**
 * Hands the tunnel's Rust library (libclaktory_tunnel.so) the JavaVM and
 * this process's Application, which iroh needs to read the phone's DNS
 * servers (app/rust/src/android.rs, DESIGN.md §4). Called once, from
 * [ClaktoryApplication.onCreate], before any Dart runs; until it has, the
 * tunnel refuses to open rather than resolve through public fallbacks.
 */
object TunnelBootstrap {
    private const val LIBRARY = "claktory_tunnel"

    @Volatile
    var installed: Boolean = false
        private set

    fun install(application: Application) {
        if (installed) return
        installed = try {
            // Loading it here lets the VM bind nativeInstall; Dart's FFI later
            // opens this same library by its soname.
            System.loadLibrary(LIBRARY)
            nativeInstall(application)
        } catch (error: UnsatisfiedLinkError) {
            // The tunnel then says it isn't set up; nothing about the error
            // carries anything private.
            Log.e("Claktory", "The private connection's library didn't load: ${error.javaClass.simpleName}")
            false
        }
        if (!installed) {
            Log.e("Claktory", "The private connection isn't set up on this device.")
            return
        }
        // Off the main thread: it reads the network's configuration through
        // JNI, as iroh does, and sends nothing. The count only, never the
        // addresses.
        Thread({
            val count = nativeSystemDnsServers()
            if (count > 0) {
                Log.i("Claktory", "The private connection reads $count DNS servers from this phone.")
            } else {
                Log.w("Claktory", "The private connection can't read this phone's DNS servers.")
            }
        }, "claktory-dns-check").start()
    }

    /** The DNS servers iroh reads from this phone: 0 none, -1 not installed. */
    @JvmStatic
    private external fun nativeSystemDnsServers(): Int

    /** True once installed (also if it already was). */
    @JvmStatic
    private external fun nativeInstall(application: Application): Boolean
}
