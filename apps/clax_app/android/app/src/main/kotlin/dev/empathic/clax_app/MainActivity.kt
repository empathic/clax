package dev.empathic.clax_app

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    private var background: BackgroundWorkChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        background = BackgroundWorkChannel(this, flutterEngine.dartExecutor.binaryMessenger)
        // "nativeLibraryDir" answers the directory holding libclax.so, the
        // embedded clax executable. "openAppSettings" opens this app's page
        // in system settings (its permissions among it) and answers true.
        // "openExternal" opens an http, https or mailto URL with the app
        // the person chose for it, and answers whether one took it.
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, "dev.empathic.clax/native")
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "nativeLibraryDir" -> result.success(applicationInfo.nativeLibraryDir)
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
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        background?.onPermissionResult(requestCode, grantResults)
    }

    override fun onDestroy() {
        // Without its Dart run, the service has nothing to keep alive.
        if (isFinishing) stopService(Intent(this, BackgroundRunService::class.java))
        super.onDestroy()
    }
}
