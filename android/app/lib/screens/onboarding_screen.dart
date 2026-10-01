import 'dart:async';

import 'package:flutter/material.dart';
import 'package:xterm/xterm.dart';

import '../core/dvm.dart';
import '../core/models.dart';
import '../services/app_paths.dart';
import '../services/lock_service.dart';
import '../services/settings_service.dart';
import '../services/terminal_session.dart';
import '../services/vm_service.dart';
import 'home_shell.dart';

/// First launch: set the install password, provision the Mobian VM, then
/// run the repo's own install script inside the terminal — the user
/// finishes the agent's walkthrough right there (typing the same password
/// as the vault password keeps app lock and vault in sync).
class OnboardingScreen extends StatefulWidget {
  const OnboardingScreen({super.key});

  @override
  State<OnboardingScreen> createState() => _OnboardingScreenState();
}

enum _Phase { intro, provisioning, terminal }

class _OnboardingScreenState extends State<OnboardingScreen> {
  _Phase _phase = _Phase.intro;
  final _password = TextEditingController();
  final _confirm = TextEditingController();
  final _logs = <String>[];
  double? _progress;
  String _progressDetail = '';
  bool _busy = false;
  String? _error;

  late AppPaths _paths;
  late VmService _vm;
  late AppSettings _settings;
  Terminal? _terminal;
  TerminalSession? _session;
  StreamSubscription<DvmEvent>? _sub;

  @override
  void initState() {
    super.initState();
    _init();
  }

  Future<void> _init() async {
    _paths = await AppPaths.resolve();
    _vm = VmService(_paths);
    _settings = await SettingsService(_paths).load();
  }

  Future<void> _start() async {
    final password = _password.text;
    if (password.length < 6) {
      setState(() => _error = 'At least 6 characters.');
      return;
    }
    if (password != _confirm.text) {
      setState(() => _error = 'The two passwords do not match.');
      return;
    }
    setState(() {
      _busy = true;
      _error = null;
      _phase = _Phase.provisioning;
    });

    final call = _vm.provision(
      settings: _settings,
      installPassword: password,
      lockPassword: password,
    );
    _sub = Dvm.instance.eventsForCall(call.id).listen((e) {
      if (e.event == 'log') {
        setState(() => _logs.add(e.line));
      } else if (e.event == 'progress') {
        setState(() {
          _progress = e.pct;
          _progressDetail = e.detail;
        });
      }
    });

    try {
      await call.future;
      if (!mounted) {
        return;
      }
      await _enterTerminal(password);
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _busy = false;
        _error = '$e';
        _phase = _Phase.intro;
      });
    }
  }

  /// Provisioning sealed the lock; now hand the user a terminal and run
  /// the repo's install script inside the VM.
  Future<void> _enterTerminal(String password) async {
    // The lock now exists — unlock it to get the sealed SSH key.
    final profile = await LockService(_paths).unlock(password);
    final terminal = Terminal(maxLines: 10000);
    final session = await TerminalSession.open(
      profile: profile,
      terminal: terminal,
      settings: _settings,
    );
    setState(() {
      _terminal = terminal;
      _session = session;
      _phase = _Phase.terminal;
      _busy = false;
    });
    // Run the repo's own installer — it asks Prod/Experimental, installs
    // podman + the agent, and ends in the walkthrough wizard where the
    // user sets the vault password (the same one, to keep app lock and
    // vault in sync).
    session.write(
      'curl -fsSL ${_settings.installScriptUrl} | bash -s -- ${_settings.releaseChannel}\n',
    );
  }

  @override
  void dispose() {
    _sub?.cancel();
    _session?.close();
    _password.dispose();
    _confirm.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('DeCypherTek.ai — first launch'),
        automaticallyImplyLeading: false,
      ),
      body: SafeArea(
        child: _buildPhase(),
      ),
    );
  }

  Widget _buildPhase() {
    switch (_phase) {
      case _Phase.intro:
        return _intro();
      case _Phase.provisioning:
        return _provisioning();
      case _Phase.terminal:
        return _terminalView();
    }
  }

  Widget _intro() {
    return ListView(
      padding: const EdgeInsets.all(24),
      children: [
        Text('Set up your secure AI terminal', style: Theme.of(context).textTheme.headlineSmall),
        const SizedBox(height: 12),
        const Text(
          'This app boots a headless Mobian Linux VM (QEMU) on your phone. '
          'It is reachable only through SSH on this device\'s loopback — '
          'no VNC, no exposed ports, nothing on the network. Inside it, '
          'the same DeCypherTek.ai terminal AI runs unchanged.',
        ),
        const SizedBox(height: 12),
        const Text(
          'First launch will: download the minimal Mobian image (~800 MB, '
          'resumable), boot the VM, generate this app\'s SSH key, and run '
          'the installer. You finish the setup wizard inside the terminal.',
        ),
        const SizedBox(height: 24),
        TextField(
          controller: _password,
          obscureText: true,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'Install password',
            helperText: 'Unlocks the app AND seals your agent vault — you will type it again in the wizard.',
            border: OutlineInputBorder(),
          ),
        ),
        const SizedBox(height: 12),
        TextField(
          controller: _confirm,
          obscureText: true,
          onSubmitted: (_) => _start(),
          decoration: const InputDecoration(
            labelText: 'Confirm password',
            border: OutlineInputBorder(),
          ),
        ),
        if (_error != null) ...[
          const SizedBox(height: 12),
          Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
        ],
        const SizedBox(height: 24),
        FilledButton(
          onPressed: _busy ? null : _start,
          child: _busy
              ? const SizedBox(height: 20, width: 20, child: CircularProgressIndicator(strokeWidth: 2))
              : const Text('Install'),
        ),
      ],
    );
  }

  Widget _provisioning() {
    return Padding(
      padding: const EdgeInsets.all(24),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          if (_progress != null) ...[
            LinearProgressIndicator(value: _progress),
            const SizedBox(height: 4),
            Text('Downloading $_progressDetail…',
                style: Theme.of(context).textTheme.bodySmall),
            const SizedBox(height: 12),
          ] else ...[
            const LinearProgressIndicator(),
            const SizedBox(height: 4),
            Text('Preparing the VM…', style: Theme.of(context).textTheme.bodySmall),
            const SizedBox(height: 12),
          ],
          Expanded(
            child: Container(
              decoration: BoxDecoration(
                color: Colors.black,
                borderRadius: BorderRadius.circular(8),
              ),
              padding: const EdgeInsets.all(12),
              child: ListView.builder(
                itemCount: _logs.length,
                itemBuilder: (_, i) => Text(
                  _logs[i],
                  style: const TextStyle(fontFamily: 'monospace', fontSize: 12, color: Colors.greenAccent),
                ),
              ),
            ),
          ),
          if (_error != null) ...[
            const SizedBox(height: 12),
            Text(_error!, style: TextStyle(color: Theme.of(context).colorScheme.error)),
          ],
        ],
      ),
    );
  }

  Widget _terminalView() {
    final terminal = _terminal;
    if (terminal == null) {
      return const Center(child: CircularProgressIndicator());
    }
    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 8, 16, 8),
          child: Text(
            'The installer is running inside the VM — finish the wizard here. '
            'Use the same password for the vault.',
            style: Theme.of(context).textTheme.bodySmall,
            textAlign: TextAlign.center,
          ),
        ),
        Expanded(
          child: TerminalView(
            terminal,
            autofocus: true,
            textStyle: const TerminalStyle(fontSize: 12),
          ),
        ),
        Padding(
          padding: const EdgeInsets.all(8),
          child: OutlinedButton(
            onPressed: () async {
              final password = _password.text;
              final profile = await LockService(_paths).unlock(password);
              if (!context.mounted) {
                return;
              }
              Navigator.of(context).pushReplacement(
                MaterialPageRoute(
                  builder: (_) => HomeShell(
                    password: password,
                    profile: profile,
                    settings: _settings,
                  ),
                ),
              );
            },
            child: const Text('Done — open the terminal'),
          ),
        ),
      ],
    );
  }
}
