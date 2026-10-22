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
 * The engine's channels ([EngineChannels]) are made with it, once, and
 * kept as long as it is; an activity attaches to them and detaches.
 *
 * [onTrimMemory] sends `trim` on `dev.empathic.clax/background`, which
 * `ChannelBackgroundWork` answers by trimming the core's memory
 * (`CoreHost.trim`), as iOS's Background.swift does when the app leaves
 * the screen or gets a memory warning.
 */
class ClaxApplication : Application() {
    private var channels: EngineChannels? = null
    private var channelsEngine: FlutterEngine? = null

    /** The app's engine, made and cached on the first call. */
    fun engine(): FlutterEngine =
        FlutterEngineCache.getInstance().get(ENGINE)
            ?: FlutterEngine(this).also { FlutterEngineCache.getInstance().put(ENGINE, it) }

    /** [engine]'s channels, made on the first call for that engine. */
    fun channels(engine: FlutterEngine): EngineChannels {
        channels?.let { if (channelsEngine === engine) return it }
        return EngineChannels(this, engine).also {
            channels = it
            channelsEngine = engine
        }
    }

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
