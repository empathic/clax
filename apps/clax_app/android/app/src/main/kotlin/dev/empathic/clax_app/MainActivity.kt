package dev.empathic.clax_app

import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
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
}
