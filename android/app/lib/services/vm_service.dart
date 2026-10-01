import '../core/dvm.dart';
import '../core/models.dart';
import 'app_paths.dart';

/// VM lifecycle + the first-launch provisioning sequence.
class VmService {
  VmService(this._paths);

  final AppPaths _paths;

  /// Boot the headless VM (no-op if already running).
  Future<void> start(AppSettings settings) async {
    await Dvm.instance.call('vm.start', {
      'dir': _paths.supportDir,
      'engine_dir': _paths.engineDir,
      'ram_mb': settings.ramMb,
      'cpus': settings.cpus,
      'port': settings.port,
      'accel': settings.accel,
      'seccomp': settings.seccomp,
    });
  }

  Future<void> stop() => Dvm.instance.call('vm.stop');

  Future<Map<String, dynamic>> state() async {
    final r = await Dvm.instance.call('vm.state');
    return r;
  }

  bool stateRunning(Map<String, dynamic> state) =>
      (state['running'] as bool?) ?? false;

  /// Download/verify the VM image + firmware if missing.
  Future<void> ensureImage(AppSettings settings) =>
      Dvm.instance.call('image.ensure', {
        'dir': _paths.supportDir,
        'url': settings.imageUrl,
        'sha256': settings.imageSha256.isEmpty ? null : settings.imageSha256,
        'bios_url': settings.biosUrl,
        'bios_vars_url': settings.biosVarsUrl,
      });

  /// The whole first-launch sequence (image -> boot -> provision -> seal
  /// the lock). Watch [Dvm.eventsForCall] for logs and download progress.
  DvmCall provision({
    required AppSettings settings,
    required String installPassword,
    required String lockPassword,
  }) =>
      Dvm.instance.startCall('provision.run', {
        'dir': _paths.supportDir,
        'engine_dir': _paths.engineDir,
        'ram_mb': settings.ramMb,
        'cpus': settings.cpus,
        'port': settings.port,
        'user': settings.vmUser,
        // null -> the guest's auto-generated first-boot secret is used.
        'initial_password':
            settings.initialVmPassword.isEmpty ? null : settings.initialVmPassword,
        'new_password': installPassword,
        'lock_password': lockPassword,
        'lock_path': _paths.lockPath,
        'image_url': settings.imageUrl,
        'image_sha256': settings.imageSha256.isEmpty ? null : settings.imageSha256,
        'bios_url': settings.biosUrl,
        'bios_vars_url': settings.biosVarsUrl,
        'profile_extra': {
          'settings': settings.toJson(),
        },
      });
}
