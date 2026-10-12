import 'dart:io';

import 'package:flutter/services.dart';

/// Finds the embedded `clax` executable.
abstract interface class ExeLocator {
  /// The absolute path of `clax`.
  Future<String> clax();
}

/// macOS: `<bundle>/Contents/Resources/clax`, beside the app's own
/// executable in `Contents/MacOS`.
class MacExeLocator implements ExeLocator {
  const MacExeLocator();

  @override
  Future<String> clax() async =>
      '${File(Platform.resolvedExecutable).parent.parent.path}/Resources/clax';
}

/// Android: `<nativeLibraryDir>/libclax.so`, the directory answered by the
/// `nativeLibraryDir` method of the `dev.empathic.clax/native` channel.
class AndroidExeLocator implements ExeLocator {
  const AndroidExeLocator({
    this._channel = const MethodChannel('dev.empathic.clax/native'),
  });

  final MethodChannel _channel;

  @override
  Future<String> clax() async {
    final dir = await _channel.invokeMethod<String>('nativeLibraryDir');
    if (dir == null || dir.isEmpty) {
      throw StateError('the native channel answered no nativeLibraryDir');
    }
    return '$dir/libclax.so';
  }
}

/// A fixed path (tests, and builds that name `clax` explicitly).
class FixedExeLocator implements ExeLocator {
  const FixedExeLocator(this.path);
  final String path;

  @override
  Future<String> clax() async => path;
}
