import 'package:flutter/material.dart';

import '../core/models.dart';
import '../services/app_paths.dart';
import '../services/settings_service.dart';

/// Settings: VM resources, image URLs, release channel, change password,
/// stop VM. Kept deliberately small — the terminal is the product.
class SettingsScreen extends StatefulWidget {
  const SettingsScreen({
    super.key,
    required this.settings,
    required this.vmRunning,
    required this.onStopVm,
    required this.onChangePassword,
  });

  final AppSettings settings;
  final bool vmRunning;
  final Future<void> Function() onStopVm;
  final Future<void> Function() onChangePassword;

  @override
  State<SettingsScreen> createState() => _SettingsScreenState();
}

class _SettingsScreenState extends State<SettingsScreen> {
  late final AppSettings _settings = widget.settings;
  String? _coreVersion;
  bool _saving = false;

  @override
  void initState() {
    super.initState();
    _loadCoreVersion();
  }

  Future<void> _loadCoreVersion() async {
    try {
      final v = await AppPaths.instance.coreVersion();
      if (mounted) {
        setState(() => _coreVersion = v);
      }
    } catch (_) {
      // Non-fatal.
    }
  }

  Future<void> _save() async {
    setState(() => _saving = true);
    try {
      await SettingsService(AppPaths.instance).save(_settings);
    } finally {
      if (mounted) {
        setState(() => _saving = false);
      }
    }
    if (mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        const SnackBar(
          content: Text('Saved — restart the VM to apply.'),
        ),
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Settings')),
      body: ListView(
        padding: const EdgeInsets.all(16),
        children: [
          Text('VM', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 8),
          _slider(
            label: 'Memory (MB)',
            value: _settings.ramMb,
            min: 1024,
            max: 4096,
            divisions: 12,
            onChanged: (v) => setState(() => _settings.ramMb = v),
          ),
          _slider(
            label: 'CPUs',
            value: _settings.cpus,
            min: 1,
            max: 8,
            divisions: 7,
            onChanged: (v) => setState(() => _settings.cpus = v),
          ),
          SwitchListTile(
            title: const Text('QEMU seccomp sandbox'),
            subtitle: const Text('-sandbox enable=on (falls back if unsupported)'),
            value: _settings.seccomp,
            onChanged: (v) => setState(() => _settings.seccomp = v),
          ),
          ListTile(
            title: const Text('Acceleration'),
            subtitle: Text(_settings.accel == 'auto'
                ? 'auto (KVM if available, else TCG)'
                : _settings.accel),
            trailing: DropdownButton<String>(
              value: _settings.accel,
              items: const [
                DropdownMenuItem(value: 'auto', child: Text('auto')),
                DropdownMenuItem(value: 'kvm', child: Text('kvm (rooted)')),
                DropdownMenuItem(value: 'tcg', child: Text('tcg')),
              ],
              onChanged: (v) => setState(() => _settings.accel = v ?? 'auto'),
            ),
          ),
          const Divider(),
          Text('Install', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 8),
          ListTile(
            title: const Text('Release channel'),
            subtitle: const Text('what the in-VM installer pulls'),
            trailing: DropdownButton<String>(
              value: _settings.releaseChannel,
              items: const [
                DropdownMenuItem(value: 'production', child: Text('Production')),
                DropdownMenuItem(value: 'experimental', child: Text('Experimental')),
              ],
              onChanged: (v) =>
                  setState(() => _settings.releaseChannel = v ?? 'production'),
            ),
          ),
          const Divider(),
          Text('Security', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 8),
          ListTile(
            leading: const Icon(Icons.lock_outline),
            title: const Text('Change app password'),
            subtitle: const Text('re-seals this app\'s encrypted profile'),
            onTap: widget.onChangePassword,
          ),
          ListTile(
            leading: const Icon(Icons.stop_circle_outlined),
            title: const Text('Stop the VM'),
            subtitle: Text(widget.vmRunning ? 'running' : 'not running'),
            onTap: widget.vmRunning
                ? () async {
                    await widget.onStopVm();
                    if (context.mounted) {
                      Navigator.pop(context);
                    }
                  }
                : null,
          ),
          const Divider(),
          Text('About', style: Theme.of(context).textTheme.titleMedium),
          const SizedBox(height: 8),
          ListTile(
            title: const Text('Rust core'),
            subtitle: Text(_coreVersion ?? '…'),
          ),
          const ListTile(
            title: Text('Security posture'),
            subtitle: Text(
              'Headless QEMU (no VNC, port 5900 never used). SSH forwarded to '
              'loopback only. Host key pinned. App data sealed with '
              'AES-256-GCM + Argon2id.',
            ),
          ),
          const SizedBox(height: 24),
          FilledButton(
            onPressed: _saving ? null : _save,
            child: Text(_saving ? 'Saving…' : 'Save'),
          ),
        ],
      ),
    );
  }

  Widget _slider({
    required String label,
    required int value,
    required int min,
    required int max,
    int? divisions,
    required ValueChanged<int> onChanged,
  }) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text('$label: $value'),
        Slider(
          value: value.toDouble(),
          min: min.toDouble(),
          max: max.toDouble(),
          divisions: divisions,
          label: '$value',
          onChanged: (v) => onChanged(v.round()),
        ),
      ],
    );
  }
}
