package app.threestrands.mail

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileNotFoundException
import java.security.KeyStore
import java.security.MessageDigest
import java.util.concurrent.Executors
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@TauriPlugin
class NativeServices(private val activity: Activity) : Plugin(activity) {
    private val secrets = SyncSecretStore(activity.applicationContext)

    private companion object {
        val storage = Executors.newSingleThreadExecutor()
    }

    private external fun initializeTls(context: Context)

    init {
        initializeTls(activity.applicationContext)
    }

    @Command
    fun readSecret(invoke: Invoke) = secretCommand(invoke) { service, account ->
        val result = JSObject()
        secrets.read(service, account)?.let { result.put("value", it) }
        invoke.resolve(result)
    }

    @Command
    fun writeSecret(invoke: Invoke) = secretCommand(invoke) { service, account ->
        secrets.write(service, account, invoke.getArgs().getString("value"))
        invoke.resolve()
    }

    @Command
    fun deleteSecret(invoke: Invoke) = secretCommand(invoke) { service, account ->
        secrets.delete(service, account)
        invoke.resolve()
    }

    private fun secretCommand(invoke: Invoke, action: (String, String) -> Unit) {
        storage.execute {
            try {
                val args = invoke.getArgs()
                action(args.getString("service"), args.getString("account"))
            } catch (_: Exception) {
                invoke.reject("Android could not access the secure sync store.")
            }
        }
    }

    @Command
    fun exportFile(invoke: Invoke) {
        try {
            val intent = Intent(Intent.ACTION_CREATE_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("application/zip")
                .putExtra(Intent.EXTRA_TITLE, "ThreeStrands Notes and Tasks.zip")
            startActivityForResult(invoke, intent, "saveExport")
        } catch (_: Exception) {
            invoke.reject("Android could not open the file picker.")
        }
    }

    @ActivityCallback
    fun saveExport(invoke: Invoke, result: ActivityResult) {
        val destination = result.data?.data
        if (result.resultCode != Activity.RESULT_OK || destination == null) {
            invoke.resolveObject(false)
            return
        }
        Thread {
            try {
                val source = File(invoke.getArgs().getString("path")).canonicalFile
                val exports = File(activity.cacheDir, "mobile-exports").canonicalFile
                check(source.parentFile == exports && source.extension == "zip")
                val output = activity.contentResolver.openOutputStream(destination)
                    ?: error("File provider did not open the export")
                output.use { writer -> source.inputStream().use { it.copyTo(writer) } }
                invoke.resolveObject(true)
            } catch (_: Exception) {
                invoke.reject("Android could not write the export.")
            }
        }.start()
    }
}

internal class SyncSecretStore(context: Context) {
    private val directory = File(context.noBackupFilesDir, "sync-secrets")
    private val alias = "threestrands-sync-v1"
    private val ivBytes = 12

    private fun key(create: Boolean): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        check(create)
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder(
                alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT
            ).setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build())
            generateKey()
        }
    }

    private fun entry(service: String, account: String): Pair<AtomicFile, ByteArray> {
        val address = "$service\u0000$account".toByteArray(Charsets.UTF_8)
        val name = MessageDigest.getInstance("SHA-256").digest(address)
            .joinToString("") { "%02x".format(it) }
        return AtomicFile(File(directory, name)) to address
    }

    @Synchronized
    fun read(service: String, account: String): String? {
        val (file, address) = entry(service, account)
        val bytes = try {
            file.readFully()
        } catch (_: FileNotFoundException) {
            return null
        }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(false), GCMParameterSpec(128, bytes, 0, ivBytes))
        cipher.updateAAD(address)
        return cipher.doFinal(bytes, ivBytes, bytes.size - ivBytes).toString(Charsets.UTF_8)
    }

    @Synchronized
    fun write(service: String, account: String, value: String) {
        check(directory.isDirectory || directory.mkdirs())
        val (file, address) = entry(service, account)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key(true))
        check(cipher.iv.size == ivBytes)
        cipher.updateAAD(address)
        val bytes = cipher.iv + cipher.doFinal(value.toByteArray(Charsets.UTF_8))
        val output = file.startWrite()
        try {
            output.write(bytes)
            file.finishWrite(output)
        } catch (error: Exception) {
            file.failWrite(output)
            throw error
        }
    }

    @Synchronized
    fun delete(service: String, account: String) {
        val (file, _) = entry(service, account)
        file.delete()
        check(!file.baseFile.exists())
    }
}
