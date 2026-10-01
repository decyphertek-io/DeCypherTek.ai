package io.decyphertek.ai

import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

// The single platform channel: hands the nativeLibraryDir (where the
// bundled QEMU engine must live — Android only allows exec from there)
// and the app data dir to Dart.
class MainActivity : FlutterActivity() {
    private val channelName = "decyphertek/native"

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, channelName)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "info" -> result.success(
                        mapOf(
                            "nativeLibraryDir" to applicationInfo.nativeLibraryDir,
                            "dataDir" to (filesDir.parent ?: ""),
                        )
                    )
                    else -> result.notImplemented()
                }
            }
    }
}
