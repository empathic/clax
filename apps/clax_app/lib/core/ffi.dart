/// The embedded core's four C functions
/// (crates/clax-embed/include/clax_embed.h), each taking the port it
/// posts to as a [SendPort].
library;

import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:isolate';

import 'package:ffi/ffi.dart';

import 'frame.dart';

/// The core's C functions. Every answer is the C function's: see each.
abstract interface class CoreLib {
  /// The functions of [lib], looked up by name now.
  factory CoreLib(DynamicLibrary lib) = _NativeCoreLib;

  /// This platform's library, found as [coreLibSource] says.
  static CoreLib load() => switch (coreLibSource(
    os: Platform.operatingSystem,
    env: Platform.environment,
    executable: Platform.resolvedExecutable,
    release: const bool.fromEnvironment('dart.vm.product'),
  )) {
    CoreLibOpen(:final path) => CoreLib(DynamicLibrary.open(path)),
    CoreLibProcess() => CoreLib(DynamicLibrary.process()),
    CoreLibUnsupported(:final reason) => throw UnsupportedError(reason),
  };

  /// `clax_embed_open`: opens the core on [home] and returns its handle,
  /// or 0 when this process has that home open already. [port] hears the
  /// open's progress, then `ready` or `failed`, then `closed`, each as
  /// JSON bytes.
  int open(String home, SendPort port);

  /// `clax_embed_call`: 0 when the core took request [frame] (its
  /// answer, after the 8 bytes of [tag], goes to [port]), 1 for an unknown
  /// handle, 2 for a frame it could not read.
  int call(int core, RequestFrame frame, SendPort port, int tag);

  /// `clax_embed_subscribe`: 0 when the core's SSE blocks will be posted
  /// to [port] (an `event: end` block last), 1 for an unknown handle.
  int subscribe(int core, SendPort port);

  /// `clax_embed_close`: 0 when the core is closing (its open port hears
  /// `closed`), 1 for an unknown handle.
  int close(int core);
}

/// Where the core's library is found.
sealed class CoreLibSource {
  const CoreLibSource();
}

/// The shared library at [path] (a bare name is found the platform's way).
final class CoreLibOpen extends CoreLibSource {
  const CoreLibOpen(this.path);

  final String path;

  @override
  bool operator ==(Object other) => other is CoreLibOpen && other.path == path;

  @override
  int get hashCode => path.hashCode;

  @override
  String toString() => 'CoreLibOpen($path)';
}

/// The running executable, which links the core in statically.
final class CoreLibProcess extends CoreLibSource {
  const CoreLibProcess();

  @override
  bool operator ==(Object other) => other is CoreLibProcess;

  @override
  int get hashCode => 0;

  @override
  String toString() => 'CoreLibProcess()';
}

/// No core for this platform; [reason] says why and what to do.
final class CoreLibUnsupported extends CoreLibSource {
  const CoreLibUnsupported(this.reason);

  final String reason;

  @override
  String toString() => 'CoreLibUnsupported($reason)';
}

/// Where the core's library is on [os] (`Platform.operatingSystem`), for
/// the app running as [executable], with environment [env]:
///
/// - outside a [release] build, a non-empty `CLAX_EMBED_LIB` names it on
///   any platform (a host's build, for tests and development);
/// - Android: `libclax_embed.so`, from the APK;
/// - iOS: the process itself, which links the core in statically;
/// - macOS: the app bundle's `Contents/Frameworks/libclax_embed.dylib`,
///   [executable] being `<bundle>/Contents/MacOS/<name>`;
/// - any other platform has none.
CoreLibSource coreLibSource({
  required String os,
  required Map<String, String> env,
  required String executable,
  required bool release,
}) {
  final named = env['CLAX_EMBED_LIB'] ?? '';
  if (!release && named.isNotEmpty) return CoreLibOpen(named);
  switch (os) {
    case 'android':
      return const CoreLibOpen('libclax_embed.so');
    case 'ios':
      return const CoreLibProcess();
    case 'macos':
      final macOS = executable.lastIndexOf('/');
      final contents = executable.substring(0, executable.lastIndexOf('/', macOS - 1));
      return CoreLibOpen('$contents/Frameworks/libclax_embed.dylib');
    default:
      return CoreLibUnsupported(
        'the embedded core is not built for $os'
        '${release ? '' : '; set CLAX_EMBED_LIB to a host build of it (//crates/clax-embed:clax_embed_host)'}',
      );
  }
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
  int call(int core, RequestFrame frame, SendPort port, int tag) =>
      lendFrame(frame, (p, n) => _call(core, p, n, port.nativePort, tag));

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

/// [use] of [frame] written straight into native memory, and its length;
/// the memory is freed when [use] returns (the C functions copy what they
/// keep). The request's body is copied once, into that memory.
T lendFrame<T>(RequestFrame frame, T Function(Pointer<Uint8>, int) use) {
  final n = frame.length;
  final p = malloc<Uint8>(n);
  try {
    frame.writeTo(p.asTypedList(n));
    return use(p, n);
  } finally {
    malloc.free(p);
  }
}
