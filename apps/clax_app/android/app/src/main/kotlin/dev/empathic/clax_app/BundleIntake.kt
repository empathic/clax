package dev.empathic.clax_app

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.IOException
import java.util.UUID
import java.util.concurrent.Executors

/**
 * The `dev.empathic.clax/bundles` channel for `ChannelBundleIntake` in
 * `lib/platform/bundle_intake.dart`: the `.clax` files the person opens in
 * Clax through `MainActivity`'s `VIEW` and `SEND` intent filters.
 *
 * Each intent's `content:` or `file:` URI is copied, off the main thread,
 * to `<cache>/incoming/<random>.clax`, stopping past [MAX_BYTES]. Dart's
 * `initial` answers the files of the launching intent as a list of {path}
 * or {refused}; from then on each file is sent as `opened` {path} or
 * {refused}. `refused` is `too_large` past [MAX_BYTES] and `not_a_bundle`
 * when the URI cannot be read. A file that is not a bundle is Dart's
 * reader's to refuse.
 */
class BundleIntake(private val activity: Activity, messenger: BinaryMessenger) {
    private val channel = MethodChannel(messenger, CHANNEL)
    private val main = Handler(Looper.getMainLooper())
    private val io = Executors.newSingleThreadExecutor()

    // Main thread only.
    private var ready = false
    private var copying = 0
    private val pending = mutableListOf<Map<String, String>>()
    private var initialResult: MethodChannel.Result? = null

    init {
        channel.setMethodCallHandler { call, result ->
            if (call.method != "initial") {
                result.notImplemented()
                return@setMethodCallHandler
            }
            // The launching intent's copy may still run: answer when it ends.
            if (copying > 0) initialResult = result else answerInitial(result)
        }
        // Copies an earlier run left behind go first; the single IO thread
        // runs this before any copy below.
        io.execute { incomingDir().listFiles()?.forEach { it.delete() } }
        // A launch from recents redelivers the old intent; it opens nothing.
        val launch = activity.intent
        if (launch != null &&
            launch.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0 &&
            onNewIntent(launch)
        ) {
            forget(launch)
        }
    }

    /** Takes [intent]'s file when it carries one; true when it did. */
    fun onNewIntent(intent: Intent?): Boolean {
        val uri = uriOf(intent) ?: return false
        copying++
        io.execute {
            val event = copy(uri)
            main.post {
                copying--
                deliver(event)
                if (copying == 0) initialResult?.let {
                    initialResult = null
                    answerInitial(it)
                }
            }
        }
        return true
    }

    fun dispose() {
        channel.setMethodCallHandler(null)
        io.shutdown()
    }

    private fun answerInitial(result: MethodChannel.Result) {
        ready = true
        result.success(pending.toList())
        pending.clear()
    }

    private fun deliver(event: Map<String, String>) {
        if (ready) channel.invokeMethod("opened", event) else pending.add(event)
    }

    private fun incomingDir() = File(activity.cacheDir, "incoming")

    private fun copy(uri: Uri): Map<String, String> {
        val dir = incomingDir()
        dir.mkdirs()
        val out = File(dir, "${UUID.randomUUID()}.clax")
        try {
            val input = activity.contentResolver.openInputStream(uri)
                ?: return mapOf("refused" to "not_a_bundle")
            var total = 0L
            input.use { src ->
                out.outputStream().use { dst ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = src.read(buf)
                        if (n < 0) break
                        total += n
                        if (total > MAX_BYTES) {
                            dst.close()
                            out.delete()
                            return mapOf("refused" to "too_large")
                        }
                        dst.write(buf, 0, n)
                    }
                }
            }
            return mapOf("path" to out.absolutePath)
        } catch (e: IOException) {
            out.delete()
            return mapOf("refused" to "not_a_bundle")
        } catch (e: SecurityException) {
            out.delete()
            return mapOf("refused" to "not_a_bundle")
        }
    }

    companion object {
        const val CHANNEL = "dev.empathic.clax/bundles"

        /** §6.1's cap on a `.clax` file: 32 MiB. */
        const val MAX_BYTES = 32L * 1024 * 1024

        private val schemes = setOf("content", "file")

        /** Makes [intent] carry no file, so a recreated activity opens none. */
        private fun forget(intent: Intent) {
            intent.action = Intent.ACTION_MAIN
            intent.data = null
            intent.removeExtra(Intent.EXTRA_STREAM)
        }

        /** The file a `VIEW` or `SEND` intent carries; null for any other. */
        fun uriOf(intent: Intent?): Uri? {
            val uri = when (intent?.action) {
                Intent.ACTION_VIEW -> intent.data
                Intent.ACTION_SEND -> streamOf(intent)
                else -> null
            }
            return uri?.takeIf { it.scheme?.lowercase() in schemes }
        }

        private fun streamOf(intent: Intent): Uri? =
            if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }
    }
}
