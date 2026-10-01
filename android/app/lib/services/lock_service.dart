import '../core/dvm.dart';
import '../core/models.dart';
import 'app_paths.dart';

/// The app-lock: one password, set during install, that unlocks the app
/// and decrypts the profile holding the SSH key into the VM.
class LockService {
  LockService(this._paths);

  final AppPaths _paths;

  Future<bool> exists() async {
    final r = await Dvm.instance.call('lock.exists', {
      'lock_path': _paths.lockPath,
    });
    return (r['exists'] as bool?) ?? false;
  }

  /// Verify the password and return the decrypted profile.
  Future<AppProfile> unlock(String password) async {
    final r = await Dvm.instance.call('lock.unlock', {
      'password': password,
      'lock_path': _paths.lockPath,
    });
    final profile = (r['profile'] as Map<String, dynamic>?) ?? {};
    return AppProfile.fromJson(profile);
  }

  /// Change the app-lock password (re-seals the same profile).
  Future<void> rekey(String oldPassword, String newPassword) async {
    await Dvm.instance.call('lock.rekey', {
      'lock_path': _paths.lockPath,
      'old_password': oldPassword,
      'new_password': newPassword,
    });
  }
}
