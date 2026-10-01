import 'dart:async';
import 'dart:convert';

import 'ffi.dart';

/// One event from the Rust core — a call reply (`log`, `progress`,
/// `done`, `error`, all carrying `id`) or a pushed event (`out`,
/// `term_closed`, `vm_log`, `vm_exit`).
class DvmEvent {
  DvmEvent(this.raw) : id = (raw['id'] as num?)?.toInt();

  final Map<String, dynamic> raw;
  final int? id;

  String get event => (raw['event'] as String?) ?? '';
  String get line => (raw['line'] as String?) ?? '';
  double? get pct => (raw['pct'] as num?)?.toDouble();
  String get detail => (raw['detail'] as String?) ?? '';
  int? get term => (raw['term'] as num?)?.toInt();
  List<int> get outBytes => b64Decode((raw['b64'] as String?) ?? '');
  int? get exitCode => (raw['code'] as num?)?.toInt();
  Map<String, dynamic> get data =>
      (raw['data'] as Map<String, dynamic>?) ?? const {};
  String get message => (raw['message'] as String?) ?? '';

  bool get isDone => event == 'done';
  bool get isError => event == 'error';
}

/// A started command: [id] for event filtering, [future] for the reply.
class DvmCall {
  DvmCall(this.id, this.future);

  final int id;
  final Future<Map<String, dynamic>> future;
}

/// The typed client over the JSON bus.
///
/// [call] sends a command and completes with the `done` payload (or
/// throws with the `error` message). [startCall] is the same but also
/// exposes the call id, so screens can watch live `log`/`progress` events
/// for that one command via [events]. [events] streams everything — call
/// replies included.
class Dvm {
  Dvm._() {
    _poll = Timer.periodic(const Duration(milliseconds: 20), (_) => _drain());
  }

  static final Dvm instance = Dvm._();

  final _controller = StreamController<DvmEvent>.broadcast();
  final _pending = <int, Completer<Map<String, dynamic>>>{};
  late final Timer _poll;
  bool _draining = false;

  Stream<DvmEvent> get events => _controller.stream;

  /// Send a command; completes with the `done` data payload.
  Future<Map<String, dynamic>> call(
    String cmd, [
    Map<String, dynamic>? args,
    Duration timeout = const Duration(minutes: 20),
  ]) => startCall(cmd, args, timeout).future;

  /// Send a command and keep its id for event filtering.
  DvmCall startCall(
    String cmd, [
    Map<String, dynamic>? args,
    Duration timeout = const Duration(minutes: 20),
  ]) {
    final payload = <String, dynamic>{...?args, 'cmd': cmd};
    final id = dvmCommandRaw(jsonEncode(payload));
    if (id == 0) {
      final err = dvmLastError();
      throw StateError('command "$cmd" rejected: $err');
    }
    final completer = Completer<Map<String, dynamic>>();
    _pending[id] = completer;
    final future = completer.future.timeout(timeout, onTimeout: () {
      _pending.remove(id);
      throw TimeoutException('command "$cmd" timed out');
    });
    return DvmCall(id, future);
  }

  void _drain() {
    if (_draining) {
      return;
    }
    _draining = true;
    try {
      while (true) {
        final batch = dvmPollEventsRaw();
        if (batch == null) {
          break;
        }
        final List<dynamic> list;
        try {
          list = jsonDecode(batch) as List<dynamic>;
        } catch (_) {
          break;
        }
        for (final item in list) {
          if (item is! Map<String, dynamic>) {
            continue;
          }
          final e = DvmEvent(item);
          if (e.id != null) {
            final completer = _pending.remove(e.id);
            if (e.isDone) {
              completer?.complete(e.data);
              if (completer == null) {
                // Reply for a call that timed out — still surface it.
                _controller.add(e);
              }
            } else if (e.isError) {
              completer?.completeError(StateError(e.message));
              if (completer == null) {
                _controller.add(e);
              }
            } else {
              _controller.add(e);
            }
          } else {
            _controller.add(e);
          }
        }
      }
    } finally {
      _draining = false;
    }
  }

  /// Live logs/progress for one call (subsets of [events]).
  Stream<DvmEvent> eventsForCall(int callId) =>
      events.where((e) => e.id == callId && (e.event == 'log' || e.event == 'progress'));

  void dispose() {
    _poll.cancel();
    _controller.close();
  }
}
