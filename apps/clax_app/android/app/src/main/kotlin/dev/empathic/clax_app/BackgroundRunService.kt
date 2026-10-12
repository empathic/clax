package dev.empathic.clax_app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

/**
 * The notifications Clax posts: the ongoing one of [BackgroundRunService]
 * on the "runs" channel, and how a run ended ("… is ready", "Couldn't …")
 * on the "news" channel. Tapping one opens [MainActivity]; a notice about
 * an artifact carries its ID as [EXTRA_ARTIFACT].
 */
object Notices {
    const val RUNS_CHANNEL = "runs"
    const val NEWS_CHANNEL = "news"
    const val RUN_NOTIFICATION = 1
    const val EXTRA_ARTIFACT = "dev.empathic.clax.artifact"
    const val EXTRA_TITLE = "title"
    const val EXTRA_TEXT = "text"

    fun ensureChannels(context: Context) {
        val nm = context.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(RUNS_CHANNEL, "Apps being made", NotificationManager.IMPORTANCE_LOW)
                .apply { description = "Shown while Clax makes or changes an app" },
        )
        nm.createNotificationChannel(
            NotificationChannel(NEWS_CHANNEL, "Finished apps", NotificationManager.IMPORTANCE_DEFAULT)
                .apply { description = "When an app or a change is ready, or failed" },
        )
    }

    /** Opens the app; on [artifact] when given. */
    fun open(context: Context, artifact: String?): PendingIntent {
        val intent = Intent(context, MainActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        if (artifact != null) intent.putExtra(EXTRA_ARTIFACT, artifact)
        return PendingIntent.getActivity(
            context,
            artifact?.hashCode() ?: 0,
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }

    fun run(context: Context, title: String, text: String): Notification {
        val b = Notification.Builder(context, RUNS_CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_clax)
            .setContentTitle(title)
            .setContentText(text)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setProgress(0, 0, true)
            .setContentIntent(open(context, null))
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
        }
        return b.build()
    }

    /** Posts [title]/[text], replacing the last notice about [artifact]. */
    fun news(context: Context, title: String, text: String, artifact: String?) {
        ensureChannels(context)
        val n = Notification.Builder(context, NEWS_CHANNEL)
            .setSmallIcon(R.drawable.ic_stat_clax)
            .setContentTitle(title)
            .setContentText(text)
            .setAutoCancel(true)
            .setContentIntent(open(context, artifact))
            .build()
        val id = 1000 + ((artifact?.hashCode() ?: 0) and 0xffff)
        context.getSystemService(NotificationManager::class.java).notify(id, n)
    }
}

/**
 * A `dataSync` foreground service that runs while Clax makes or changes
 * an app, so a run the person leaves keeps its process and its network
 * (Android cuts a background app's network: `blocked=APP_BACKGROUND`). It
 * does no work itself: the Dart run does, and stops it when the last run
 * ends. Each start shows [Notices.EXTRA_TITLE] and [Notices.EXTRA_TEXT].
 * It stops on its own when the system's time limit for the type is spent
 * ([onTimeout]) and when the app's task is removed.
 */
class BackgroundRunService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        Notices.ensureChannels(this)
        val n = Notices.run(
            this,
            intent?.getStringExtra(Notices.EXTRA_TITLE) ?: "Clax",
            intent?.getStringExtra(Notices.EXTRA_TEXT) ?: "",
        )
        startForeground(Notices.RUN_NOTIFICATION, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        running = true
        return START_NOT_STICKY
    }

    /** Android 15 and later: the type's daily time is spent. */
    override fun onTimeout(startId: Int, fgsType: Int) {
        stopSelf()
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        stopSelf()
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }

    companion object {
        /** Whether the service is in the foreground now. */
        @Volatile
        var running = false
            private set
    }
}
