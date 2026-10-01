import 'package:flutter/material.dart';

import 'screens/home_shell.dart';
import 'screens/lock_screen.dart';
import 'screens/onboarding_screen.dart';
import 'services/app_paths.dart';
import 'services/lock_service.dart';

void main() {
  runApp(const DecypherApp());
}

class DecypherApp extends StatelessWidget {
  const DecypherApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'DeCypherTek.ai',
      debugShowCheckedModeBanner: false,
      theme: ThemeData(
        brightness: Brightness.dark,
        colorSchemeSeed: const Color(0xFF00B8A9),
        useMaterial3: true,
      ),
      home: const StartupGate(),
    );
  }
}

/// Decides what the user sees first: onboarding on a fresh install, the
/// lock screen on every later launch.
class StartupGate extends StatefulWidget {
  const StartupGate({super.key});

  @override
  State<StartupGate> createState() => _StartupGateState();
}

class _StartupGateState extends State<StartupGate> {
  bool _loading = true;
  bool _provisioned = false;
  String _error = '';

  @override
  void initState() {
    super.initState();
    _bootstrap();
  }

  Future<void> _bootstrap() async {
    try {
      final paths = await AppPaths.resolve();
      final lock = LockService(paths);
      final exists = await lock.exists();
      if (!mounted) {
        return;
      }
      setState(() {
        _provisioned = exists;
        _loading = false;
      });
    } catch (e) {
      if (!mounted) {
        return;
      }
      setState(() {
        _error = '$e';
        _loading = false;
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    if (_loading) {
      return const Scaffold(
        body: Center(child: CircularProgressIndicator()),
      );
    }
    if (_error.isNotEmpty()) {
      return Scaffold(
        body: Center(
          child: Padding(
            padding: const EdgeInsets.all(24),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                const Icon(Icons.error_outline, size: 48),
                const SizedBox(height: 12),
                Text(_error, textAlign: TextAlign.center),
              ],
            ),
          ),
        ),
      );
    }
    if (!_provisioned) {
      return const OnboardingScreen();
    }
    return LockScreen(
      onUnlocked: (password, profile, settings) {
        Navigator.of(context).pushReplacement(
          MaterialPageRoute(
            builder: (_) => HomeShell(
              password: password,
              profile: profile,
              settings: settings,
            ),
          ),
        );
      },
    );
  }
}
