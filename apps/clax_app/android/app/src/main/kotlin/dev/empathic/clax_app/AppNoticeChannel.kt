package dev.empathic.clax_app

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.graphics.drawable.Icon
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel

/**
 * Serves `dev.empathic.clax/app_notice` for `ChannelAppNotice` in
 * `lib/platform/app_notice.dart`: the ongoing, silent notification shown
 * while an app is open in Clax, so the person reaches Comment and Threads
 * from the notification shade instead of a control drawn over the app.
 *
 * - `show` {artifact, title} posts it on the low-importance "open_app"
 *   channel (no sound, no vibration, no heads-up), replacing the last one;
 *   `clear` removes it.
 * - `allowed` answers whether it can be seen: notifications are enabled for
 *   Clax (POST_NOTIFICATIONS on Android 13 and later) and the channel is not
 *   turned off.
 * - `launchTap` answers, once, the tap that launched the activity.
 *
 * Its body and its "Comment" and "Threads" actions are explicit, immutable
 * activity intents to [MainActivity] (single top) carrying [EXTRA_ARTIFACT]
 * and [EXTRA_ACTION]; a tap while the activity runs reaches [onNewIntent],
 * which calls `tapped` {artifact, action}. Each has its own intent action
 * ([ACTION_OPEN], [ACTION_COMMENT], [ACTION_THREADS]), so none matches
 * another notification's PendingIntent.
 *
 * Swiping the notice away (Android 14 and later allow it for an ongoing
 * notification without a foreground service) sends its delete intent, an
 * immutable broadcast to this package's [ACTION_DISMISSED], which the
 * receiver registered while the activity lives turns into `dismissed`
 * {artifact}. Call [dispose] from the activity's `onDestroy`.
 */
class AppNoticeChannel(private val activity: Activity, messenger: BinaryMessenger) :
    MethodChannel.MethodCallHandler {
    private val channel = MethodChannel(messenger, NAME)
    private var launchTap: Map<String, String>? = tapOf(activity.intent)
    private val dismissals = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val aid = intent.getStringExtra(EXTRA_ARTIFACT) ?: return
            channel.invokeMethod("dismissed", mapOf("artifact" to aid))
        }
    }

    init {
        channel.setMethodCallHandler(this)
        val filter = IntentFilter(ACTION_DISMISSED)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            activity.registerReceiver(dismissals, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            activity.registerReceiver(dismissals, filter)
        }
        // A launch from recents redelivers the old intent; it asks for nothing.
        if (activity.intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) {
            launchTap = null
        }
        forget(activity.intent)
    }

    override fun onMethodCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "show" -> {
                val aid = call.argument<String>("artifact")
                if (aid.isNullOrEmpty()) {
                    result.error("bad_artifact", "show needs an artifact", null)
                    return
                }
                show(activity, aid, call.argument<String>("title") ?: "Your app")
                result.success(null)
            }
            "clear" -> {
                clear(activity)
                result.success(null)
            }
            "allowed" -> result.success(allowed(activity))
            "launchTap" -> {
                result.success(launchTap)
                launchTap = null
            }
            else -> result.notImplemented()
        }
    }

    /** From the activity's `onDestroy`. */
    fun dispose() {
        try {
            activity.unregisterReceiver(dismissals)
        } catch (e: IllegalArgumentException) {
            // Not registered.
        }
    }

    /** From the activity's `onNewIntent`. */
    fun onNewIntent(intent: Intent) {
        val tap = tapOf(intent) ?: return
        forget(intent)
        channel.invokeMethod("tapped", tap)
    }

    companion object {
        const val NAME = "dev.empathic.clax/app_notice"
        const val CHANNEL = "open_app"
        const val NOTIFICATION = 2
        const val EXTRA_ARTIFACT = "dev.empathic.clax.notice.artifact"
        const val EXTRA_ACTION = "dev.empathic.clax.notice.action"
        const val ACTION_OPEN = "dev.empathic.clax.notice.OPEN"
        const val ACTION_COMMENT = "dev.empathic.clax.notice.COMMENT"
        const val ACTION_THREADS = "dev.empathic.clax.notice.THREADS"
        const val ACTION_DISMISSED = "dev.empathic.clax.notice.DISMISSED"
        private val ACTIONS = listOf("open", "comment", "threads")
        private val INTENT_ACTIONS = listOf(ACTION_OPEN, ACTION_COMMENT, ACTION_THREADS)

        private fun tapOf(intent: Intent?): Map<String, String>? {
            val aid = intent?.getStringExtra(EXTRA_ARTIFACT) ?: return null
            val action = intent.getStringExtra(EXTRA_ACTION)?.takeIf { it in ACTIONS } ?: return null
            return mapOf("artifact" to aid, "action" to action)
        }

        private fun forget(intent: Intent) {
            intent.removeExtra(EXTRA_ARTIFACT)
            intent.removeExtra(EXTRA_ACTION)
        }

        private fun ensureChannel(nm: NotificationManager) {
            nm.createNotificationChannel(
                NotificationChannel(CHANNEL, "Open app", NotificationManager.IMPORTANCE_LOW).apply {
                    description = "Shown while an app is open, with Comment and Threads"
                    setSound(null, null)
                    enableVibration(false)
                    setShowBadge(false)
                },
            )
        }

        fun allowed(context: Context): Boolean {
            val nm = context.getSystemService(NotificationManager::class.java) ?: return false
            if (!nm.areNotificationsEnabled()) return false
            ensureChannel(nm)
            val ch = nm.getNotificationChannel(CHANNEL)
            return ch == null || ch.importance != NotificationManager.IMPORTANCE_NONE
        }

        /** A tap on the notice asking for [action] on [artifact]. */
        private fun tap(context: Context, artifact: String, action: String): PendingIntent {
            val intent = Intent(context, MainActivity::class.java)
                .setAction(INTENT_ACTIONS[ACTIONS.indexOf(action)])
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
                .putExtra(EXTRA_ARTIFACT, artifact)
                .putExtra(EXTRA_ACTION, action)
            return PendingIntent.getActivity(
                context,
                REQUEST_BASE + ACTIONS.indexOf(action),
                intent,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
        }

        private const val REQUEST_BASE = 7410

        /** The broadcast a swipe of [artifact]'s notice sends. */
        private fun dismissed(context: Context, artifact: String): PendingIntent =
            PendingIntent.getBroadcast(
                context,
                REQUEST_BASE + ACTIONS.size,
                Intent(ACTION_DISMISSED).setPackage(context.packageName)
                    .putExtra(EXTRA_ARTIFACT, artifact),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )

        fun show(context: Context, artifact: String, title: String) {
            val nm = context.getSystemService(NotificationManager::class.java) ?: return
            ensureChannel(nm)
            val icon = Icon.createWithResource(context, R.drawable.ic_stat_clax)
            val n = Notification.Builder(context, CHANNEL)
                .setSmallIcon(R.drawable.ic_stat_clax)
                .setContentTitle(title)
                .setContentText("Comment on it, or see its threads")
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setShowWhen(false)
                .setCategory(Notification.CATEGORY_STATUS)
                .setContentIntent(tap(context, artifact, "open"))
                .setDeleteIntent(dismissed(context, artifact))
                .addAction(Notification.Action.Builder(icon, "Comment", tap(context, artifact, "comment")).build())
                .addAction(Notification.Action.Builder(icon, "Threads", tap(context, artifact, "threads")).build())
                .build()
            try {
                nm.notify(NOTIFICATION, n)
            } catch (e: SecurityException) {
                // POST_NOTIFICATIONS was revoked meanwhile: nothing is shown.
            }
        }

        fun clear(context: Context) {
            context.getSystemService(NotificationManager::class.java)?.cancel(NOTIFICATION)
        }
    }
}
