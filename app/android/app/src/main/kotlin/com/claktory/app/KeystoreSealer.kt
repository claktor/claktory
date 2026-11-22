package com.claktory.app

import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyInfo
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.security.keystore.StrongBoxUnavailableException
import java.security.KeyStore
import javax.crypto.AEADBadTagException
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import javax.crypto.spec.GCMParameterSpec

/**
 * The connections' key: AES-256-GCM in Android Keystore under [alias],
 * never exportable, no user authentication (the app reaches its
 * Claktories whenever it's open), StrongBox when the phone has one, else
 * the TEE; never anything outside Keystore. Keystore chooses every nonce
 * (randomized encryption). Where the key lives is logged once, when it's
 * made (DESIGN.md §5).
 */
class KeystoreSealer(
    private val context: Context,
    private val alias: String = ALIAS,
    private val log: (String) -> Unit = {},
) : Sealer {
    override fun hasKey(): Boolean = keystore { it.containsAlias(alias) }

    override fun seal(plain: ByteArray, aad: ByteArray): Sealed {
        val key = existingKey() ?: makeKey()
        val cipher = guarded { Cipher.getInstance(TRANSFORMATION) }
        guarded { cipher.init(Cipher.ENCRYPT_MODE, key) }
        return guarded {
            cipher.updateAAD(aad)
            val bytes = cipher.doFinal(plain)
            Sealed(cipher.iv, bytes)
        }
    }

    override fun open(nonce: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray {
        val key = existingKey()
            ?: throw StoreFailure(StoreFailure.KEY_MISSING, "The key that protects them is gone from this phone.")
        val cipher = guarded { Cipher.getInstance(TRANSFORMATION) }
        guarded { cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, nonce)) }
        return try {
            cipher.updateAAD(aad)
            cipher.doFinal(sealed)
        } catch (error: AEADBadTagException) {
            throw StoreFailure(StoreFailure.UNREADABLE, "They were changed or damaged.")
        } catch (error: StoreFailure) {
            throw error
        } catch (error: Exception) {
            throw failure(error)
        }
    }

    override fun deleteKey() = keystore { store ->
        if (store.containsAlias(alias)) store.deleteEntry(alias)
    }

    private fun existingKey(): SecretKey? = keystore { it.getKey(alias, null) as SecretKey? }

    private fun makeKey(): SecretKey {
        val strongBox = Build.VERSION.SDK_INT >= Build.VERSION_CODES.P &&
            context.packageManager.hasSystemFeature(PackageManager.FEATURE_STRONGBOX_KEYSTORE)
        val key = if (strongBox) {
            try {
                generate(strongBox = true)
            } catch (error: StrongBoxUnavailableException) {
                generate(strongBox = false)
            }
        } else {
            generate(strongBox = false)
        }
        log("made the key that protects them, in ${where(key)}")
        return key
    }

    private fun generate(strongBox: Boolean): SecretKey = guarded {
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setRandomizedEncryptionRequired(true)
            .setUserAuthenticationRequired(false)
            .apply { if (strongBox && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) setIsStrongBoxBacked(true) }
            .build()
        KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER).run {
            init(spec)
            generateKey()
        }
    }

    /** StrongBox, the TEE or software (an emulator), for the log. */
    private fun where(key: SecretKey): String = runCatching {
        val info = SecretKeyFactory.getInstance(key.algorithm, PROVIDER).getKeySpec(key, KeyInfo::class.java) as KeyInfo
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            when (info.securityLevel) {
                KeyProperties.SECURITY_LEVEL_STRONGBOX -> "StrongBox"
                KeyProperties.SECURITY_LEVEL_TRUSTED_ENVIRONMENT -> "the TEE"
                KeyProperties.SECURITY_LEVEL_SOFTWARE -> "software"
                else -> "an unknown place"
            }
        } else {
            @Suppress("DEPRECATION")
            if (info.isInsideSecureHardware) "secure hardware" else "software"
        }
    }.getOrDefault("an unknown place")

    private fun <T> keystore(body: (KeyStore) -> T): T = guarded {
        body(KeyStore.getInstance(PROVIDER).apply { load(null) })
    }

    /** [body], any Keystore failure as the store's: invalidated keys apart, it may work later. */
    private fun <T> guarded(body: () -> T): T = try {
        body()
    } catch (error: StoreFailure) {
        throw error
    } catch (error: Exception) {
        throw failure(error)
    }

    private fun failure(error: Exception) = if (error is KeyPermanentlyInvalidatedException) {
        StoreFailure(StoreFailure.KEY_INVALIDATED, "The key that protects them stopped working on this phone.")
    } else {
        StoreFailure(StoreFailure.UNAVAILABLE, "This phone's keystore didn't answer (${error.javaClass.simpleName}).")
    }

    companion object {
        const val ALIAS = "claktory.device.v1"
        private const val PROVIDER = "AndroidKeyStore"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val TAG_BITS = 128
    }
}
