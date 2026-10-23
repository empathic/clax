package dev.empathic.clax_app

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugins.webviewflutter.WebViewFlutterAndroidExternalApi

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
 * - `openAppSettings` opens this app's page in system settings (its
 *   permissions among it) and answers true.
 * - `openExternal` {url} opens an http, https or mailto URL with the app the
 *   person chose for it, and answers whether one took it. With `mode:
 *   "auth"` it opens an https sign-in page in a Custom Tab over the
 *   activity on screen (the default browser's, when it has them; the
 *   browser itself otherwise).
 * - `closeAuth` answers false: a Custom Tab is closed by the person.
 * - `deviceName` answers the device's name (Settings' device name where
 *   the system keeps one, else its model).
 */
class EngineChannels(private val app: Context, engine: FlutterEngine) {
    private val messenger = engine.dartExecutor.binaryMessenger

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
                    if (call.argument<String>("mode") == "auth") {
                        result.success(uri != null && uri.scheme?.lowercase() == "https" && openAuth(uri))
                    } else if (uri == null || uri.scheme?.lowercase() !in setOf("http", "https", "mailto")) {
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
                "closeAuth" -> result.success(false)
                "deviceName" -> result.success(deviceName())
                else -> result.notImplemented()
            }
        }
    }

    /**
     * Opens [uri] in a Custom Tab: an `ACTION_VIEW` intent carrying the
     * Custom Tabs session extra (a null session), which a browser that
     * has Custom Tabs shows over the app and any other opens as usual.
     * Started from the activity on screen, so the tab sits in Clax's
     * task; from the application in a task of its own when none is.
     */
    private fun openAuth(uri: Uri): Boolean {
        val intent = Intent(Intent.ACTION_VIEW, uri)
            .addCategory(Intent.CATEGORY_BROWSABLE)
            .putExtras(Bundle().apply { putBinder(CUSTOM_TABS_SESSION, null) })
        val from: Context = activity ?: app.also {
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        }
        return try {
            from.startActivity(intent)
            true
        } catch (e: ActivityNotFoundException) {
            false
        }
    }

    /** The device's name as Settings shows it, else its model. */
    private fun deviceName(): String {
        val named = Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
        return named?.takeIf { it.isNotBlank() } ?: Build.MODEL
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

        /** The Custom Tabs protocol's session extra (androidx.browser's `CustomTabsIntent.EXTRA_SESSION`). */
        private const val CUSTOM_TABS_SESSION = "android.support.customtabs.extra.SESSION"
    }
}
