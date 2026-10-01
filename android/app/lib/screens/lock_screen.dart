import 'package:flutter/material.dart';

import '../core/models.dart';
import '../services/app_paths.dart';
import '../services/lock_service.dart';
import '../services/settings_service.dart';

/// The app lock screen: the install password decrypts the profile that
/// holds the SSH key into the VM. No password, no app.
class LockScreen extends StatefulWidget {
  const LockScreen({super.key, required this.onUnlocked});

  final void Function(String password, AppProfile profile, AppSettings settings)
      onUnlocked;

  @override
  State<LockScreen> createState() => _LockScreenState();
}

class _LockScreenState extends State<LockScreen> {
  final _controller = TextEditingController();
  bool _busy = false;
  String? _error;

  Future<void> _unlock() async {
    final password = _controller.text;
    if (password.isEmpty) {
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final paths = await AppPaths.resolve();
      final profile = await LockService(paths).unlock(password);
      final settings = await SettingsService(paths).load();
      widget.onUnlocked(password, profile, settings);
    } catch (e) {
      setState(() {
        _busy = false;
        _error =
            'Wrong password (or the app lock is corrupted) — there is no recovery, by design.';
      });
    }
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 420),
            child: Padding(
              padding: const EdgeInsets.all(24),
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  const Icon(Icons.lock_outline, size: 56),
                  const SizedBox(height: 8),
                  Text(
                    'DeCypherTek.ai',
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.headlineSmall,
                  ),
                  const SizedBox(height: 4),
                  Text(
                    'Enter the password you set during install — it decrypts this app\'s data and unlocks the system.',
                    textAlign: TextAlign.center,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: 24),
                  TextField(
                    controller: _controller,
                    obscureText: true,
                    autofocus: true,
                    enabled: !_busy,
                    onSubmitted: (_) => _unlock(),
                    decoration: InputDecoration(
                      labelText: 'Password',
                      border: const OutlineInputBorder(),
                      errorText: _error,
                    ),
                  ),
                  const SizedBox(height: 16),
                  FilledButton(
                    onPressed: _busy ? null : _unlock,
                    child: _busy
                        ? const SizedBox(
                            height: 20,
                            width: 20,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Text('Unlock'),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}
