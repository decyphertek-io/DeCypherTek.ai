import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

/// Raw C bindings to libdecyphertek_core.so.
///
/// The whole surface is five functions: enqueue a JSON command, poll a
/// JSON array of events, free a returned string, read the last error and
/// read the version. Everything else is JSON on top of these.
final class _Bindings {
  _Bindings._() {
    final DynamicLibrary lib;
    if (Platform.isAndroid) {
      lib = DynamicLibrary.open('libdecyphertek_core.so');
    } else if (Platform.isMacOS) {
      lib = DynamicLibrary.open('libdecyphertek_core.dylib');
    } else {
      lib = DynamicLibrary.open('libdecyphertek_core.so');
    }
    command = lib.lookupFunction<
        Uint32 Function(Pointer<Utf8>),
        int Function(Pointer<Utf8>)>('dvm_command');
    pollEvents = lib.lookupFunction<
        Pointer<Utf8> Function(),
        Pointer<Utf8> Function()>('dvm_poll_events');
    freeString = lib.lookupFunction<
        Void Function(Pointer<Utf8>),
        void Function(Pointer<Utf8>)>('dvm_free_string');
    lastError = lib.lookupFunction<
        Pointer<Utf8> Function(),
        Pointer<Utf8> Function()>('dvm_last_error');
    version = lib.lookupFunction<
        Pointer<Utf8> Function(),
        Pointer<Utf8> Function()>('dvm_version');
  }

  static final _Bindings instance = _Bindings._();

  late final int Function(Pointer<Utf8>) command;
  late final Pointer<Utf8> Function() pollEvents;
  late final void Function(Pointer<Utf8>) freeString;
  late final Pointer<Utf8> Function() lastError;
  late final Pointer<Utf8> Function() version;
}

/// Enqueue a JSON command; returns its call id (0 = parse failure).
int dvmCommandRaw(String json) {
  final ptr = json.toNativeUtf8();
  try {
    return _Bindings.instance.command(ptr);
  } finally {
    malloc.free(ptr);
  }
}

/// Drain pending events (a JSON array string), or null if none.
String? dvmPollEventsRaw() {
  final ptr = _Bindings.instance.pollEvents();
  if (ptr == nullptr) {
    return null;
  }
  try {
    return ptr.toDartString();
  } finally {
    _Bindings.instance.freeString(ptr);
  }
}

/// The last error message set on this thread.
String dvmLastError() {
  final ptr = _Bindings.instance.lastError();
  return ptr == nullptr ? '' : ptr.toDartString();
}

/// The Rust core's version string.
String dvmVersion() {
  final ptr = _Bindings.instance.version();
  return ptr == nullptr ? '' : ptr.toDartString();
}

/// Convenience: encode/decode helpers shared by the rest of the app.
String b64Encode(List<int> bytes) => base64Encode(bytes);
List<int> b64Decode(String s) => base64Decode(s);
