package dev.empathic.clax_app

import android.content.ContentResolver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.InputStream
import java.util.UUID
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicReference

/**
 * The `dev.empathic.clax/bundles` channel for `ChannelBundleIntake` in
 * `lib/platform/bundle_intake.dart`: the `.clax` files and Get in Clax
 * links the person opens in Clax. [BundleReceiverActivity] takes them from
 * other apps and forwards each to `MainActivity`: a file as an
 * [ACTION_OPEN_BUNDLE] intent whose data is the file's `content:` URI, a
 * link as an [ACTION_OPEN_LINK] intent whose data is the link. A link is
 * sent to Dart as {url}, which Dart parses and may drop.
 *
 * Each URI is copied on its own thread to `<cache>/incoming/<random>.clax`,
 * stopping past [MAX_BYTES] and giving up after [COPY_TIMEOUT_MS]. Dart's
 * `initial` answers the files of the launching intent as a list of {path}
 * or {refused}, once their copies end; from then on each file is sent as
 * `opened` {path} or {refused}. `refused` is `too_large` past [MAX_BYTES]
 * and `not_a_bundle` when the URI cannot be read in time. A file that is
 * not a bundle is Dart's reader's to refuse.
 *
 * It is made once per engine ([EngineChannels]), on the application
 * context [context]; each activity that comes on screen hands it its
 * launching intent ([attach]).
 */
class BundleIntake(private val context: Context, messenger: BinaryMessenger) {
    private val channel = MethodChannel(messenger, CHANNEL)
    private val main = Handler(Looper.getMainLooper())
    private val io = Executors.newCachedThreadPool()

    // Main thread only.
    private var ready = false
    private var copying = 0
    private val pending = mutableListOf<Map<String, String>>()
    private val initialResults = mutableListOf<MethodChannel.Result>()

    init {
        channel.setMethodCallHandler { call, result ->
            if (call.method != "initial") {
                result.notImplemented()
                return@setMethodCallHandler
            }
            // The launching intent's copy may still run: answer when it ends.
            initialResults.add(result)
            if (copying == 0) answerInitial()
        }
        // Copies an earlier run left behind are removed first.
        val stale = incomingDir().listFiles()?.toList() ?: emptyList()
        io.execute { stale.forEach { it.delete() } }
    }

    /** An activity came on screen, launched by [launch]. */
    fun attach(launch: Intent?) {
        // A launch from recents redelivers the old intent; it opens nothing.
        if (launch != null &&
            launch.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY == 0 &&
            onNewIntent(launch)
        ) {
            forget(launch)
        }
    }

    /** Takes [intent]'s file or link when it carries one; true when it did. */
    fun onNewIntent(intent: Intent?): Boolean {
        if (intent?.action == ACTION_OPEN_LINK) {
            val link = asLink(intent.data) ?: return false
            deliver(mapOf("url" to link.toString()))
            return true
        }
        if (intent?.action != ACTION_OPEN_BUNDLE) return false
        val uri = acceptable(context, intent.data) ?: return false
        copying++
        var done = false
        val input = AtomicReference<InputStream?>(null)
        val out = File(incomingDir(), "${UUID.randomUUID()}.clax")
        // Main thread: the first of the copy and the watchdog to end wins.
        fun finish(event: Map<String, String>) {
            if (done) {
                out.delete()
                return
            }
            done = true
            copying--
            deliver(event)
            if (copying == 0) answerInitial()
        }
        val watchdog = Runnable {
            if (!done) {
                // Unblocks a read stuck in the provider, where it can.
                try {
                    input.get()?.close()
                } catch (e: Exception) {
                }
                finish(mapOf("refused" to "not_a_bundle"))
                out.delete()
            }
        }
        main.postDelayed(watchdog, COPY_TIMEOUT_MS)
        io.execute {
            val event = try {
                copy(context.contentResolver, uri, out) { input.set(it) }
            } catch (e: Exception) {
                out.delete()
                mapOf("refused" to "not_a_bundle")
            }
            main.post {
                main.removeCallbacks(watchdog)
                finish(event)
            }
        }
        return true
    }

    private fun answerInitial() {
        if (initialResults.isEmpty()) return
        ready = true
        val files = pending.toList()
        pending.clear()
        // Every `initial` asked gets the files; only the first can be new to
        // Dart, as ChannelBundleIntake asks once.
        initialResults.forEachIndexed { i, r -> r.success(if (i == 0) files else emptyList<Any>()) }
        initialResults.clear()
    }

    private fun deliver(event: Map<String, String>) {
        if (ready) channel.invokeMethod("opened", event) else pending.add(event)
    }

    private fun incomingDir() = File(context.cacheDir, "incoming")

    companion object {
        const val CHANNEL = "dev.empathic.clax/bundles"

        /** The intent [BundleReceiverActivity] sends `MainActivity`. */
        const val ACTION_OPEN_BUNDLE = "dev.empathic.clax.OPEN_BUNDLE"

        /** The intent [BundleReceiverActivity] sends `MainActivity` for a link. */
        const val ACTION_OPEN_LINK = "dev.empathic.clax.OPEN_LINK"

        /** The schemes of the links the manifest's filters take. */
        private val LINK_SCHEMES = setOf("https", "clax")

        /**
         * The link a `VIEW` intent carries: an `https:` or `clax:` URI; null
         * for any other intent, such as a file's `content:` URI.
         */
        fun linkOf(intent: Intent?): Uri? =
            if (intent?.action == Intent.ACTION_VIEW) asLink(intent.data) else null

        private fun asLink(uri: Uri?): Uri? = uri?.takeIf { it.scheme?.lowercase() in LINK_SCHEMES }

        /** §6.1's cap on a `.clax` file: 32 MiB. */
        const val MAX_BYTES = 32L * 1024 * 1024

        /** How long one copy may take before it is given up. */
        const val COPY_TIMEOUT_MS = 120_000L

        /**
         * [uri] when Clax may read it as a bundle: a `content:` URI of
         * another app's provider. `file:` URIs and Clax's own providers are
         * refused, so no app can make Clax copy one of its private files.
         */
        fun acceptable(context: Context, uri: Uri?): Uri? {
            if (uri?.scheme?.lowercase() != ContentResolver.SCHEME_CONTENT) return null
            val authority = uri.authority ?: return null
            val owner = try {
                context.packageManager.resolveContentProvider(authority, 0)?.packageName
            } catch (e: Exception) {
                null
            }
            if (owner == context.packageName) return null
            if (authority.split(';').any { it.startsWith(context.packageName) }) return null
            return uri
        }

        /** The file a `VIEW` or `SEND` intent carries; null for any other. */
        fun uriOf(intent: Intent?): Uri? = when (intent?.action) {
            Intent.ACTION_VIEW -> intent.data
            Intent.ACTION_SEND -> streamOf(intent)
            else -> null
        }

        /** Makes [intent] carry no file, so a recreated activity opens none. */
        private fun forget(intent: Intent) {
            intent.action = Intent.ACTION_MAIN
            intent.data = null
        }

        private fun streamOf(intent: Intent): Uri? =
            if (Build.VERSION.SDK_INT >= 34) {
                intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableExtra(Intent.EXTRA_STREAM)
            }

        /** Copies [uri] to [out]; any exception is the caller's. */
        private fun copy(
            resolver: ContentResolver,
            uri: Uri,
            out: File,
            opened: (InputStream) -> Unit,
        ): Map<String, String> {
            out.parentFile?.mkdirs()
            val input = resolver.openInputStream(uri) ?: return mapOf("refused" to "not_a_bundle")
            opened(input)
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
        }
    }
}
