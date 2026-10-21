package dev.empathic.clax_app

import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.core.splashscreen.SplashScreen.Companion.installSplashScreen
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine

class MainActivity : FlutterActivity() {
    /** The engine's channels (ClaxApplication), which outlive this activity. */
    private var channels: EngineChannels? = null
    private var notice: AppNoticeChannel? = null

    // The window opens on Theme.Clax.Starting: the system's splash screen
    // (androidx core-splashscreen's on API 29-30), the Clax mark on `bg`.
    // It gives way at Flutter's first frame, which draws the same mark at
    // the same size and place (ClaxSplash), so nothing is held on screen.
    override fun onCreate(savedInstanceState: Bundle?) {
        installSplashScreen().setKeepOnScreenCondition { false }
        super.onCreate(savedInstanceState)
    }

    /**
     * The application's engine ([ClaxApplication.engine]), which outlives
     * this activity: a job keeps running after Back or a finish, and the
     * next activity shows the same Dart.
     */
    override fun provideFlutterEngine(context: Context): FlutterEngine =
        (application as ClaxApplication).engine()

    override fun shouldDestroyEngineWithHost(): Boolean = false

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        // Made once per engine; this activity only says it is on screen.
        channels = (application as ClaxApplication).channels(flutterEngine).also { it.attach(this) }
        notice = AppNoticeChannel(this, flutterEngine.dartExecutor.binaryMessenger)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        channels?.background?.onNewIntent(intent)
        notice?.onNewIntent(intent)
        channels?.bundles?.onNewIntent(intent)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        channels?.background?.onPermissionResult(requestCode, grantResults)
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
        // The open app's notice belongs to this activity's screen.
        AppNoticeChannel.clear(this)
        notice?.dispose()
        channels?.detach(this)
        // The engine and its Dart run outlive this activity
        // (ClaxApplication): the service goes on while they hold a job,
        // and Dart stops it when none runs.
        super.onDestroy()
    }
}
