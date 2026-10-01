import 'dart:convert';
import 'dart:io';

import '../core/models.dart';
import 'app_paths.dart';

/// Plain-JSON settings persistence (non-sensitive values only).
class SettingsService {
  SettingsService(this._paths);

  final AppPaths _paths;

  Future<AppSettings> load() async {
    final file = File(_paths.settingsPath);
    if (!await file.exists()) {
      return AppSettings();
    }
    try {
      final json =
          jsonDecode(await file.readAsString()) as Map<String, dynamic>;
      return AppSettings.fromJson(json);
    } catch (_) {
      return AppSettings();
    }
  }

  Future<void> save(AppSettings settings) async {
    final file = File(_paths.settingsPath);
    await file.parent.create(recursive: true);
    await file.writeAsString(settings.encode());
  }
}
