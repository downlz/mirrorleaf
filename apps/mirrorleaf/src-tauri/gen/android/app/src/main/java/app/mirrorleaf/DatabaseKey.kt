package app.mirrorleaf

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import app.mirrorleaf.engine.newDatabaseKey
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The database's key (M11, E4). The engine encrypts the database with a random 32-byte key; this
 * keeps that key wrapped by an AES key that lives in the Android Keystore and never leaves it. The
 * Keystore key doesn't ask for the person's unlock, so the push-woken job can sync while the app
 * is locked (ADR-011): the app lock covers the screen.
 */
object DatabaseKey {
    private const val ALIAS = "mirrorleaf-database-key"
    private const val FILE = "database.key.wrapped"
    private const val TAG_BITS = 128

    fun get(context: Context): ByteArray {
        val dir = File(context.filesDir, "mirrorleaf").apply { mkdirs() }
        val file = File(dir, FILE)
        val wrapping = wrappingKey()
        if (file.exists()) {
            val stored = file.readBytes()
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, wrapping, GCMParameterSpec(TAG_BITS, stored, 0, 12))
            return cipher.doFinal(stored, 12, stored.size - 12)
        }
        val key = newDatabaseKey()
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrapping)
        val sealed = cipher.iv + cipher.doFinal(key)
        // Written whole, then moved into place, so a crash can't leave half a key.
        val partial = File(dir, "$FILE.partial")
        partial.writeBytes(sealed)
        partial.renameTo(file)
        return key
    }

    private fun wrappingKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }
}
