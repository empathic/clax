package dev.empathic.clax_app

import android.content.ActivityNotFoundException
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugins.webviewflutter.WebViewFlutterAndroidExternalApi
import java.io.ByteArrayOutputStream
import kotlin.concurrent.thread

class MainActivity : FlutterActivity() {
    private var background: BackgroundWorkChannel? = null
    private var notice: AppNoticeChannel? = null
    private var bundles: BundleIntake? = null
    private var scheme: SchemeHost? = null

    // The window opens on Theme.Clax.Starting: the system's splash screen
    // (androidx core-splashscreen's on API 29-30), the Clax mark on `bg`.
    // It gives way at Flutter's first frame, which draws the same mark at
    // the same size and place (ClaxSplash), so nothing is held on screen.
    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen().setKeepOnScreenCondition { false }
        super.onCreate(savedInstanceState)
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        background = BackgroundWorkChannel(this, flutterEngine.dartExecutor.binaryMessenger)
        notice = AppNoticeChannel(this, flutterEngine.dartExecutor.binaryMessenger)
        CuesChannel(applicationContext, flutterEngine.dartExecutor.binaryMessenger)
        bundles = BundleIntake(this, flutterEngine.dartExecutor.binaryMessenger)
        // The app's origins and the ClaxHost channel in its WebViews, over
        // dev.empathic.clax/scheme (SchemeHost.kt).
        scheme = SchemeHost(flutterEngine.dartExecutor.binaryMessenger) { id ->
            @Suppress("DEPRECATION")
            WebViewFlutterAndroidExternalApi.getWebView(flutterEngine, id)
        }
        // "openAppSettings" opens this app's page
        // in system settings (its permissions among it) and answers true.
        // "openExternal" opens an http, https or mailto URL with the app
        // the person chose for it, and answers whether one took it.
        // "snapshotWebView" {id} answers an image of the WebView that
        // webview_flutter names `id` (`webViewIdentifier`) as it shows:
        // {png, at one pixel a dp, width, height}, its size in dp; null
        // without one.
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dev.empathic.clax/native")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "snapshotWebView" -> {
                        val id = call.argument<Number>("id")?.toLong()
                        @Suppress("DEPRECATION")
                        val view = id?.let { WebViewFlutterAndroidExternalApi.getWebView(flutterEngine, it) }
                        if (view == null || view.width <= 0 || view.height <= 0) {
                            result.success(null)
                        } else {
                            // Drawn at one pixel a dp, a fraction of the page's
                            // pixels, since the view draws on this thread.
                            val density = resources.displayMetrics.density
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
                                runOnUiThread {
                                    result.success(
                                        mapOf("png" to out.toByteArray(), "width" to width, "height" to height)
                                    )
                                }
                            }
                        }
                    }
                    "openAppSettings" -> {
                        startActivity(
                            Intent(
                                Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                                Uri.fromParts("package", packageName, null),
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
                                startActivity(
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

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        background?.onNewIntent(intent)
        notice?.onNewIntent(intent)
        bundles?.onNewIntent(intent)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        background?.onPermissionResult(requestCode, grantResults)
        notice?.onPermissionResult(requestCode, grantResults)
    }

    /**
     * Back on the home screen sends the task to the background instead of
     * finishing the activity, so its engine and any run in it live on.
     */
    override fun popSystemNavigator(): Boolean {
        moveTaskToBack(true)
        return true
    }

    override fun onDestroy() {
        BackgroundRunService.onStopped = null
        // The open app's notice belongs to this activity's screen.
        AppNoticeChannel.clear(this)
        notice?.dispose()
        bundles?.dispose()
        // Without its Dart run, the service has nothing to keep alive.
        if (isFinishing) {
            val stop = Intent(this, BackgroundRunService::class.java)
                .setAction(BackgroundRunService.ACTION_STOP)
            try {
                startService(stop)
            } catch (e: IllegalStateException) {
                stopService(Intent(this, BackgroundRunService::class.java))
            }
        }
        super.onDestroy()
    }
}
