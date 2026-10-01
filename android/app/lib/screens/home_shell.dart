import 'dart:async';

import 'package:flutter/material.dart';
import 'package:xterm/xterm.dart';

import '../core/dvm.dart';
import '../core/models.dart';
import '../services/app_paths.dart';
import '../services/lock_service.dart';
import '../services/terminal_session.dart';
import '../services/vm_service.dart';
import 'settings_screen.dart';

/// The main screen: the terminal IS the app. A settings button is the
/// only chrome — simple and functional.
class HomeShell extends StatefulWidget {
  const HomeShell({
    super.key,
    required this.password,
    required this.profile,
    required this.settings,
  });

  final String password;
  final AppProfile profile;
  final AppSettings settings;

  @override
  State<HomeShell> createState() => _HomeShellState();
}

class _HomeShellState extends State<HomeShell> {
  final _terminal = Terminal(maxLines: 10000);
  TerminalSession? _session;
  late VmService _vm;
  bool _vmRunning = false;
  bool _connecting = false;
  String? _error;
  StreamSubscription<DvmEvent>? _sub;

  @override
  void initState() {
    super.initState();
    _vm = VmService(AppPaths.instance);
    _sub = Dvm.instance.events.listen((e) {
      if (e.event == 'vm_exit') {
        setState(() => _vmRunning = false);
      }
    });
    _bootAndConnect();
  }

  /// Boot the VM if needed, then attach the terminal.
  Future<void> _bootAndConnect() async {
    setState(() {
      _connecting = true;
      _error = null;
    });
    try {
      final state = await _vm.state();
      var running = _vm.stateRunning(state);
      if (!running) {
        await _vm.start(widget.settings);
        running = true;
      }
      final session = await TerminalSession.open(
        profile: widget.profile,
        terminal: _terminal,
        settings: widget.settings,
      );
      session.onClosed.listen((_) {
        if (mounted) {
          setState(() {
            _session = null;
            _vmRunning = false;
          });
        }
      });
      if (!mounted) {
        return;
      }
      setState(() {
        _session = session;
        _vmRunning = running;
        _connecting = false;
        _vmState = 'running';
      });
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _connecting = false;
        _error = '$e';
      });
    }
  }

  Future<void> _stopVm() async {
    await _session?.close();
    await _vm.stop();
    setState(() {
      _session = null;
      _vmRunning = false;
      _vmState = 'stopped';
    });
  }

  Future<void> _changePassword() async {
    final controller = TextEditingController();
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Change app password'),
        content: TextField(
          controller: controller,
          obscureText: true,
          autofocus: true,
          decoration: const InputDecoration(
            labelText: 'New password',
            border: OutlineInputBorder(),
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(context, false),
            child: const Text('Cancel'),
          ),
          FilledButton(
            onPressed: () => Navigator.pop(context, true),
            child: const Text('Change'),
          ),
        ],
      ),
    );
    if (confirmed != true || controller.text.isEmpty) {
      return;
    }
    try {
      await LockService(AppPaths.instance)
          .rekey(widget.password, controller.text);
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('App password changed.')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Could not change password: $e')),
        );
      }
    }
  }

  @override
  void dispose() {
    _sub?.cancel();
    _session?.close();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('DeCypherTek.ai'),
        actions: [
          if (_connecting)
            const Padding(
              padding: EdgeInsets.all(14),
              child: SizedBox(
                height: 18,
                width: 18,
                child: CircularProgressIndicator(strokeWidth: 2),
              ),
            ),
          IconButton(
            icon: const Icon(Icons.settings),
            tooltip: 'Settings',
            onPressed: () => Navigator.push(
              context,
              MaterialPageRoute(
                builder: (_) => SettingsScreen(
                  settings: widget.settings,
                  vmRunning: _vmRunning,
                  onStopVm: _stopVm,
                  onChangePassword: _changePassword,
                ),
              ),
            ),
          ),
        ],
      ),
      body: _buildBody(),
    );
  }

  Widget _buildBody() {
    if (_session == null) {
      return Center(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            const Icon(Icons.terminal, size: 48),
            const SizedBox(height: 12),
            Text(_error ?? 'The VM is not connected.'),
            const SizedBox(height: 16),
            FilledButton(
              onPressed: _connecting ? null : _bootAndConnect,
              child: Text(_connecting ? 'Booting…' : 'Start & connect'),
            ),
          ],
        ),
      );
    }
    return TerminalView(
      _terminal,
      autofocus: true,
      textStyle: const TerminalStyle(fontSize: 12),
    );
  }
}
