import 'package:flutter/services.dart';
import 'package:path_provider/path_provider.dart';

import '../core/dvm.dart';

/// All filesystem locations the app uses, plus the nativeLibraryDir
/// (where the bundled QEMU engine must live — Android only allows exec
/// from there).
class AppPaths {
  AppPaths._(this.supportDir, this.engineDir);

  static AppPaths? _instance;

  /// The already-resolved instance (call [resolve] once at startup).
  static AppPaths get instance {
    final i = _instance;
    if (i == null) {
      throw StateError('AppPaths.resolve() must be called first');
    }
    return i;
  }

  /// Resolve once at startup (needs path_provider + the platform channel).
  static Future<AppPaths> resolve() async {
    if (_instance != null) {
      return _instance!;
    }
    final support = await getApplicationSupportDirectory();
    var engineDir = support.path;
    try {
      final info = await const MethodChannel('decyphertek/native')
          .invokeMethod<Map<dynamic, dynamic>>('info');
      engineDir = (info?['nativeLibraryDir'] as String?) ?? engineDir;
    } on PlatformException {
      // Desktop/debug hosts: the engine sits next to the core library.
    }
    _instance = AppPaths._(support.path, engineDir);
    return _instance!;
  }

  final String supportDir;
  final String engineDir;

  String get lockPath => '$supportDir/app.lock';
  String get settingsPath => '$supportDir/settings.json';
  String get vmDir => '$supportDir/vm';

  /// The Rust core's version (for the About box).
  Future<String> coreVersion() async {
    final v = await Dvm.instance.call('core.version');
    return (v['version'] as String?) ?? '?';
  }
}
