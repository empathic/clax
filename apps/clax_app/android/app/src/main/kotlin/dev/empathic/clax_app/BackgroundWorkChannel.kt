package dev.empathic.clax_app

import android.Manifest
import android.app.Activity
import android.app.NotificationManager
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Serves `dev.empathic.clax/background` for `ChannelBackgroundWork` in
 * `lib/platform/background_work.dart`:
 *
 * - `start` {title, text} starts [BackgroundRunService] and answers whether
 *   it was sent (Android refuses a foreground service started from the
 *   background); `update` {title, text} changes its notification; `stop`
 *   stops it through the service's own command queue, so a stop right
 *   after a start never overtakes it.
 * - `ready` and `failed` {title, text, artifact?} post a notice that opens
 *   the app on that artifact.
 * - `requestPermission` asks for POST_NOTIFICATIONS once per install and
 *   answers whether notifications may be posted.
 * - `launchArtifact` answers, once, the artifact of the notice that
 *   launched the activity.
 *
 * It calls `opened` {artifact} when a notice is tapped while the activity
 * runs ([onNewIntent]), and `stopped` when the service stopped on its own
 * (see [BackgroundRunService]). [ClaxApplication] sends `trim` on the
 * same channel when the system asks the app to trim its memory.
 */
class BackgroundWorkChannel(private val activity: Activity, messenger: BinaryMessenger) :
    MethodChannel.MethodCallHandler {
    private val channel = MethodChannel(messenger, NAME)

    /**
     * The service and the notices go through the application, since the
     * engine this channel is on outlives [activity] (ClaxApplication).
     */
    private val app = activity.applicationContext
    private var permissionResult: MethodChannel.Result? = null
    private var launchArtifact: String? = artifactOf(activity.intent)

    init {
        channel.setMethodCallHandler(this)
        BackgroundRunService.onStopped = { channel.invokeMethod("stopped", null) }
        // A launch from recents redelivers the old intent; it opens nothing.
        if (activity.intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) {
            launchArtifact = null
        }
        activity.intent.removeExtra(Notices.EXTRA_ARTIFACT)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        val title = call.argument<String>("title") ?: ""
        val text = call.argument<String>("text") ?: ""
        when (call.method) {
            "start" -> {
                val intent = Intent(app, BackgroundRunService::class.java)
                    .putExtra(Notices.EXTRA_TITLE, title)
                    .putExtra(Notices.EXTRA_TEXT, text)
                result.success(
                    try {
                        app.startForegroundService(intent)
                        true
                    } catch (e: IllegalStateException) {
                        // ForegroundServiceStartNotAllowedException: in the background.
                        false
                    } catch (e: SecurityException) {
                        false
                    },
                )
            }
            "update" -> {
                if (BackgroundRunService.running) {
                    app.getSystemService(NotificationManager::class.java)
                        .notify(Notices.RUN_NOTIFICATION, Notices.run(app, title, text))
                }
                result.success(null)
            }
            "stop" -> {
                val service = Intent(app, BackgroundRunService::class.java)
                try {
                    app.startService(service.setAction(BackgroundRunService.ACTION_STOP))
                } catch (e: IllegalStateException) {
                    // In the background with no service running: nothing to stop.
                    app.stopService(Intent(app, BackgroundRunService::class.java))
                }
                result.success(null)
            }
            "ready", "failed" -> {
                Notices.news(app, title, text, call.argument<String>("artifact"))
                result.success(null)
            }
            "requestPermission" -> requestPermission(result)
            "launchArtifact" -> {
                result.success(launchArtifact)
                launchArtifact = null
            }
            else -> result.notImplemented()
        }
    }

    private fun requestPermission(result: MethodChannel.Result) {
        val nm = activity.getSystemService(NotificationManager::class.java)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            result.success(nm.areNotificationsEnabled())
            return
        }
        val prefs = activity.getSharedPreferences(PREFS, Activity.MODE_PRIVATE)
        if (prefs.getBoolean(ASKED, false) || permissionResult != null) {
            result.success(false)
            return
        }
        prefs.edit().putBoolean(ASKED, true).apply()
        permissionResult = result
        activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQUEST)
    }

    /** From the activity's `onRequestPermissionsResult`. */
    fun onPermissionResult(requestCode: Int, grantResults: IntArray) {
        if (requestCode != REQUEST) return
        val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
        permissionResult?.success(granted)
        permissionResult = null
    }

    /** From the activity's `onNewIntent`. */
    fun onNewIntent(intent: Intent) {
        val aid = artifactOf(intent) ?: return
        intent.removeExtra(Notices.EXTRA_ARTIFACT)
        channel.invokeMethod("opened", mapOf("artifact" to aid))
    }

    private fun artifactOf(intent: Intent?): String? = intent?.getStringExtra(Notices.EXTRA_ARTIFACT)

    companion object {
        const val NAME = "dev.empathic.clax/background"
        private const val REQUEST = 7401
        /** Where Clax keeps that it asked for POST_NOTIFICATIONS ([ASKED]). */
        const val PREFS = "clax_background"
        const val ASKED = "asked_post_notifications"
    }
}
