import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:xterm/xterm.dart' show Terminal;

import '../core/dvm.dart';
import '../core/models.dart';

/// One SSH PTY session inside the VM, wired to an xterm [Terminal].
///
/// Keystrokes flow  terminal -> onOutput -> term.write (base64);
/// VM output flows  term.open events -> terminal.write (utf8, lossy).
class TerminalSession {
  TerminalSession._(this.id, this._profile, this._terminal);

  static const _utf8 = Utf8Codec(allowMalformed: true);

  final int id;
  final AppProfile _profile;
  final Terminal _terminal;
  StreamSubscription<DvmEvent>? _sub;

  final _closed = StreamController<void>.broadcast();
  bool _isClosed = false;

  /// Fires when the shell session ends (VM stopped, sshd gone).
  Stream<void> get onClosed => _closed.stream;

  /// Open a PTY shell in the VM and attach [terminal] to it.
  static Future<TerminalSession> open({
    required AppProfile profile,
    required Terminal terminal,
    required AppSettings settings,
  }) async {
    final r = await Dvm.instance.call(
      'term.open',
      {
        'host': '127.0.0.1',
        'port': profile.sshPort != 0 ? profile.sshPort : settings.port,
        'user': profile.sshUser,
        'key': profile.sshKey,
        'host_key': profile.hostKey,
        'cols': terminal.viewWidth,
        'rows': terminal.viewHeight,
        'timeout_secs': 300,
      },
      // First boot after unlock can be slow; the VM may still be booting.
    );

    final id = (r['term'] as num?)?.toInt() ?? 0;
    final session = TerminalSession._(id, profile, terminal);

    // VM -> screen.
    session._sub = Dvm.instance.events.listen((e) {
      if (e.event == 'out' && e.term == id) {
        terminal.write(_utf8.decode(Uint8List.fromList(e.outBytes)));
      } else if (e.event == 'term_closed' && e.term == id) {
        session._handleClosed();
      }
    });

    // Screen -> VM (keystrokes the xterm widget encoded).
    terminal.onOutput = session.write;

    // Size changes -> window_change.
    terminal.onResize = (cols, rows, pw, ph) {
      if (session._isClosed) {
        return;
      }
      Dvm.instance.call('term.resize', {'term': id, 'cols': cols, 'rows': rows});
    };

    return session;
  }

  void write(String data) {
    if (_isClosed) {
      return;
    }
    final bytes = utf8.encode(data);
    // base64Encode is dart:convert's standard alphabet — the Rust side
    // decodes with the STANDARD engine.
    Dvm.instance.call('term.write', {'term': id, 'b64': base64Encode(bytes)});
  }

  void _handleClosed() {
    if (_isClosed) {
      return;
    }
    _isClosed = true;
    _closed.add(null);
  }

  Future<void> close() async {
    if (_isClosed) {
      return;
    }
    _handleClosed();
    try {
      await Dvm.instance.call('term.close', {'term': id});
    } catch (_) {
      // Session already gone — fine.
    }
    await _sub?.cancel();
  }
}
