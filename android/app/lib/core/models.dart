import 'dart:convert';

/// Non-sensitive settings, stored as plain JSON next to the encrypted
/// lock (they change how the VM boots, not how the app is unlocked).
class AppSettings {
  AppSettings({
    this.ramMb = 2048,
    this.cpus = 2,
    this.port = 2222,
    this.accel = 'auto',
    this.seccomp = true,
    this.imageUrl = defaultImageUrl,
    this.imageSha256 = '',
    this.biosUrl = defaultBiosUrl,
    this.biosVarsUrl = defaultBiosVarsUrl,
    this.installScriptUrl = defaultInstallScriptUrl,
    this.releaseChannel = 'production',
    this.vmUser = 'decyphertek',
    this.initialVmPassword = '',
  });

  static const defaultImageUrl =
      'https://github.com/decyphertek-io/DeCypherTek.ai/releases/latest/download/decyphertek-mobian-minimal-arm64.qcow2';
  static const defaultBiosUrl =
      'https://github.com/decyphertek-io/DeCypherTek.ai/releases/latest/download/edk2-arm64-QEMU_EFI.fd';
  static const defaultBiosVarsUrl =
      'https://github.com/decyphertek-io/DeCypherTek.ai/releases/latest/download/edk2-arm64-vars.fd';
  static const defaultInstallScriptUrl =
      'https://raw.githubusercontent.com/decyphertek-io/DeCypherTek.ai/main/scripts/install.sh';

  int ramMb;
  int cpus;
  int port;
  String accel; // auto | kvm | tcg
  bool seccomp;
  String imageUrl;
  String imageSha256;
  String biosUrl;
  String biosVarsUrl;
  String installScriptUrl;
  String releaseChannel; // production | experimental (the script asks too)
  String vmUser;
  /// Only for custom, non-standard images. The stock image generates a
  /// one-time first-boot secret inside the guest (printed to the
  /// VM's serial console, read from the app-private serial log, rotated
  /// to the install password) — no credential ships in the app.
  String initialVmPassword;

  Map<String, dynamic> toJson() => {
        'ram_mb': ramMb,
        'cpus': cpus,
        'port': port,
        'accel': accel,
        'seccomp': seccomp,
        'image_url': imageUrl,
        'image_sha256': imageSha256,
        'bios_url': biosUrl,
        'bios_vars_url': biosVarsUrl,
        'install_script_url': installScriptUrl,
        'release_channel': releaseChannel,
        'vm_user': vmUser,
        'initial_vm_password': initialVmPassword,
      };

  factory AppSettings.fromJson(Map<String, dynamic> json) {
    final d = AppSettings();
    d.ramMb = (json['ram_mb'] as num?)?.toInt() ?? d.ramMb;
    d.cpus = (json['cpus'] as num?)?.toInt() ?? d.cpus;
    d.port = (json['port'] as num?)?.toInt() ?? d.port;
    d.accel = (json['accel'] as String?) ?? d.accel;
    d.seccomp = (json['seccomp'] as bool?) ?? d.seccomp;
    d.imageUrl = (json['image_url'] as String?) ?? d.imageUrl;
    d.imageSha256 = (json['image_sha256'] as String?) ?? d.imageSha256;
    d.biosUrl = (json['bios_url'] as String?) ?? d.biosUrl;
    d.biosVarsUrl = (json['bios_vars_url'] as String?) ?? d.biosVarsUrl;
    d.installScriptUrl =
        (json['install_script_url'] as String?) ?? d.installScriptUrl;
    d.releaseChannel = (json['release_channel'] as String?) ?? d.releaseChannel;
    d.vmUser = (json['vm_user'] as String?) ?? d.vmUser;
    d.initialVmPassword =
        (json['initial_vm_password'] as String?) ?? d.initialVmPassword;
    return d;
  }

  String encode() => const JsonEncoder.withIndent('  ').convert(toJson());
}

/// The sealed app-lock profile — decrypted in memory after unlock, never
/// written to disk in plaintext. The Rust side adds `ssh_key` and
/// `host_key` during provisioning.
class AppProfile {
  AppProfile(this.sshUser, this.sshPort, this.sshKey, this.hostKey);

  final String sshUser;
  final int sshPort;
  final String sshKey; // openssh private key PEM
  final String hostKey; // pinned openssh public key line

  factory AppProfile.fromJson(Map<String, dynamic> json) => AppProfile(
        (json['ssh_user'] as String?) ?? 'decyphertek',
        (json['ssh_port'] as num?)?.toInt() ?? 2222,
        (json['ssh_key'] as String?) ?? '',
        (json['host_key'] as String?) ?? '',
      );
}
