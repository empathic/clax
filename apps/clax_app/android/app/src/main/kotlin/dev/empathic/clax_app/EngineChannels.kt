package dev.empathic.clax_app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugins.webviewflutter.WebViewFlutterAndroidExternalApi
import java.io.ByteArrayOutputStream
import kotlin.concurrent.thread

/**
 * The app's channels on its one engine ([ClaxApplication.engine]), made
 * once for that engine and kept as long as it is: [SchemeHost] (the app's
 * origins in its WebViews), [BackgroundWorkChannel], [BundleIntake],
 * [CuesChannel] and `dev.empathic.clax/native`. The engine and its WebViews
 * outlive any one [MainActivity], so the objects that hold a WebView's
 * client, a pending request or a method handler outlive it too; an activity
 * only says it is the one on screen ([attach]) and that it is gone
 * ([detach]).
 *
 * `dev.empathic.clax/native`:
 * - `snapshotWebView` {id} answers an image of the WebView that
 *   webview_flutter names `id` (`webViewIdentifier`) as it shows: {png, at
 *   one pixel a dp, width, height}, its size in dp; null without one.
 * - `openAppSettings` opens this app's page in system settings (its
 *   permissions among it) and answers true.
 * - `openExternal` {url} opens an http, https or mailto URL with the app the
 *   person chose for it, and answers whether one took it.
 */
class EngineChannels(private val app: Context, engine: FlutterEngine) {
    private val messenger = engine.dartExecutor.binaryMessenger
    private val main = Handler(Looper.getMainLooper())

    /** The activity on screen, if any (main thread only). */
    var activity: Activity? = null
        private set

    val background = BackgroundWorkChannel(app, messenger) { activity }
    val bundles = BundleIntake(app, messenger)
    val scheme = SchemeHost(messenger) { id ->
        @Suppress("DEPRECATION")
        WebViewFlutterAndroidExternalApi.getWebView(engine, id)
    }

    init {
        CuesChannel(app, messenger)
        MethodChannel(messenger, NATIVE).setMethodCallHandler { call, result ->
            when (call.method) {
                "snapshotWebView" -> {
                    val id = call.argument<Number>("id")?.toLong()
                    @Suppress("DEPRECATION")
                    val view = id?.let { WebViewFlutterAndroidExternalApi.getWebView(engine, it) }
                    if (view == null || view.width <= 0 || view.height <= 0) {
                        result.success(null)
                    } else {
                        // Drawn at one pixel a dp, a fraction of the page's
                        // pixels, since the view draws on this thread.
                        val density = app.resources.displayMetrics.density
                        val w = maxOf(1, (view.width / density).toInt())
                        val h = maxOf(1, (view.height / density).toInt())
                        val bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
                        val canvas = Canvas(bitmap)
                        canvas.scale(w.toFloat() / view.width, h.toFloat() / view.height)
                        view.draw(canvas)
                        val width = view.width / density.toDouble()
                        val height = view.height / density.toDouble()
                        thread(name = "clax-snapshot") {
                            val out = ByteArrayOutputStream()
                            bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)
                            bitmap.recycle()
                            main.post {
                                result.success(
                                    mapOf("png" to out.toByteArray(), "width" to width, "height" to height)
                                )
                            }
                        }
                    }
                }
                "openAppSettings" -> {
                    app.startActivity(
                        Intent(
                            Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                            Uri.fromParts("package", app.packageName, null),
                        ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                    )
                    result.success(true)
                }
                "openExternal" -> {
                    val url = call.argument<String>("url")
                    val uri = url?.let { Uri.parse(it) }
                    if (uri == null || uri.scheme?.lowercase() !in setOf("http", "https", "mailto")) {
                        result.success(false)
                    } else {
                        try {
                            app.startActivity(
                                Intent(Intent.ACTION_VIEW, uri)
                                    .addCategory(Intent.CATEGORY_BROWSABLE)
                                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                            )
                            result.success(true)
                        } catch (e: ActivityNotFoundException) {
                            result.success(false)
                        }
                    }
                }
                else -> result.notImplemented()
            }
        }
    }

    /**
     * [activity] is the one on screen now: the channels that ask through an
     * activity use it, and its launching intent is taken (a notice's
     * artifact, a bundle) as a new intent would be.
     */
    fun attach(activity: Activity) {
        this.activity = activity
        background.attach(activity.intent)
        bundles.attach(activity.intent)
    }

    /**
     * [activity] is gone; nothing is asked through it any more, and a
     * permission request it was asked is answered with what holds now.
     */
    fun detach(activity: Activity) {
        if (this.activity !== activity) return
        this.activity = null
        background.detached()
    }

    companion object {
        const val NATIVE = "dev.empathic.clax/native"
    }
}
