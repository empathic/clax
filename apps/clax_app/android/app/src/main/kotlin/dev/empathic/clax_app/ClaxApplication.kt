package dev.empathic.clax_app

import android.app.Application
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.embedding.engine.FlutterEngineCache
import io.flutter.plugin.common.MethodChannel

/**
 * Holds the app's one [FlutterEngine] for the life of the process, in
 * [FlutterEngineCache] under [ENGINE], so the Dart side (the open core and
 * the queue's worker) outlives its activity: Back out of Clax, or the
 * system finishing [MainActivity] while a job runs under
 * [BackgroundRunService], leaves the job running, and the next activity
 * attaches to the same engine.
 *
 * The engine is made at the first activity's [MainActivity.provideFlutterEngine],
 * which then runs Dart's `main` on it; nothing runs Dart before an activity
 * is up. The process ending (the system's kill, a swipe from recents once
 * the service stops) ends the engine, and the jobs resume at the next
 * launch: nothing here asks the system to wake the app.
 *
 * [onTrimMemory] sends `trim` on `dev.empathic.clax/background`, which
 * `ChannelBackgroundWork` answers by trimming the core's memory
 * (`CoreHost.trim`), as iOS's Background.swift does when the app leaves
 * the screen or gets a memory warning.
 */
class ClaxApplication : Application() {
    /** The app's engine, made and cached on the first call. */
    fun engine(): FlutterEngine =
        FlutterEngineCache.getInstance().get(ENGINE)
            ?: FlutterEngine(this).also { FlutterEngineCache.getInstance().put(ENGINE, it) }

    override fun onTrimMemory(level: Int) {
        super.onTrimMemory(level)
        val engine = FlutterEngineCache.getInstance().get(ENGINE) ?: return
        if (!engine.dartExecutor.isExecutingDart) return
        MethodChannel(engine.dartExecutor.binaryMessenger, BackgroundWorkChannel.NAME)
            .invokeMethod("trim", null)
    }

    companion object {
        /** The engine's key in [FlutterEngineCache]. */
        const val ENGINE = "main"
    }
}
