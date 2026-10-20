/// The embedded core's four C functions
/// (crates/clax-embed/include/clax_embed.h), each taking the port it
/// posts to as a [SendPort].
library;

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

/// The core's C functions. Every answer is the C function's: see each.
abstract interface class CoreLib {
  /// The functions of [lib], looked up by name now.
  factory CoreLib(DynamicLibrary lib) = _NativeCoreLib;

  /// This platform's library: `libclax_embed.so` on Android, the
  /// executable itself on iOS (linked in statically), and on macOS the
  /// file CLAX_EMBED_LIB names when it is set, else the app bundle's
  /// `Contents/Frameworks/libclax_embed.dylib`.
  static CoreLib load() {
    if (Platform.isAndroid) {
      return CoreLib(DynamicLibrary.open('libclax_embed.so'));
    }
    if (Platform.isIOS) return CoreLib(DynamicLibrary.process());
    final named = Platform.environment['CLAX_EMBED_LIB'];
    if (named != null && named.isNotEmpty) {
      return CoreLib(DynamicLibrary.open(named));
    }
    // <bundle>/Contents/MacOS/<executable>
    final contents = File(Platform.resolvedExecutable).parent.parent.path;
    return CoreLib(
      DynamicLibrary.open('$contents/Frameworks/libclax_embed.dylib'),
    );
  }

  /// `clax_embed_open`: opens the core on [home] and returns its handle,
  /// or 0 when this process has that home open already. [port] hears the
  /// open's progress, then `ready` or `failed`, then `closed`, each as
  /// JSON bytes.
  int open(String home, SendPort port);

  /// `clax_embed_call`: 0 when the core took request frame [req] (its
  /// answer, after the 8 bytes of [tag], goes to [port]), 1 for an unknown
  /// handle, 2 for a frame it could not read.
  int call(int core, Uint8List req, SendPort port, int tag);

  /// `clax_embed_subscribe`: 0 when the core's SSE blocks will be posted
  /// to [port] (an `event: end` block last), 1 for an unknown handle.
  int subscribe(int core, SendPort port);

  /// `clax_embed_close`: 0 when the core is closing (its open port hears
  /// `closed`), 1 for an unknown handle.
  int close(int core);
}

typedef _PostC = Int8 Function(Int64, Pointer<Dart_CObject>);
typedef _OpenC =
    Uint64 Function(Pointer<Uint8>, Size, Pointer<NativeFunction<_PostC>>, Int64);
typedef _OpenD =
    int Function(Pointer<Uint8>, int, Pointer<NativeFunction<_PostC>>, int);
typedef _CallC = Int32 Function(Uint64, Pointer<Uint8>, Size, Int64, Int64);
typedef _CallD = int Function(int, Pointer<Uint8>, int, int, int);
typedef _SubscribeC = Int32 Function(Uint64, Int64);
typedef _SubscribeD = int Function(int, int);
typedef _CloseC = Int32 Function(Uint64);
typedef _CloseD = int Function(int);

final class _NativeCoreLib implements CoreLib {
  _NativeCoreLib(DynamicLibrary lib)
    : _open = lib.lookupFunction<_OpenC, _OpenD>('clax_embed_open'),
      _call = lib.lookupFunction<_CallC, _CallD>('clax_embed_call'),
      _subscribe = lib.lookupFunction<_SubscribeC, _SubscribeD>(
        'clax_embed_subscribe',
      ),
      _close = lib.lookupFunction<_CloseC, _CloseD>('clax_embed_close');

  final _OpenD _open;
  final _CallD _call;
  final _SubscribeD _subscribe;
  final _CloseD _close;

  @override
  int open(String home, SendPort port) => _lent(
    utf8.encode(home),
    (p, n) => _open(p, n, NativeApi.postCObject, port.nativePort),
  );

  @override
  int call(int core, Uint8List req, SendPort port, int tag) =>
      _lent(req, (p, n) => _call(core, p, n, port.nativePort, tag));

  @override
  int subscribe(int core, SendPort port) => _subscribe(core, port.nativePort);

  @override
  int close(int core) => _close(core);

  /// [f] of a native copy of [bytes] and its length; the copy is freed
  /// when [f] returns (the C functions copy what they keep).
  static int _lent(List<int> bytes, int Function(Pointer<Uint8>, int) f) {
    // Never NULL, even for no bytes: NULL is a refusal.
    final p = malloc<Uint8>(bytes.isEmpty ? 1 : bytes.length);
    try {
      p.asTypedList(bytes.length).setAll(0, bytes);
      return f(p, bytes.length);
    } finally {
      malloc.free(p);
    }
  }
}
