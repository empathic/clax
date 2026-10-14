import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// A `.clax` file the person opened in Clax from outside it (Files, a
/// message, Finder): a [path] to read, or a refusal by the platform before
/// any byte reached Dart.
@immutable
final class OpenedBundle {
  /// The file at [path].
  ///
  /// On Android it is Clax's own copy under `<cache>/incoming/`, which the
  /// reader may delete once read; on macOS it is the person's file, which
  /// must not be deleted.
  const OpenedBundle.file(String this.path) : refused = null;

  /// A file the platform refused, with a `BundleError` code: `too_large`
  /// (over 32 MiB) or `not_a_bundle` (it could not be read).
  const OpenedBundle.refused(String this.refused) : path = null;

  final String? path;
  final String? refused;

  /// Reads the platform's {path} or {refused}; null for anything else.
  static OpenedBundle? of(Object? args) => switch (args) {
    {'path': final String p} when p.isNotEmpty => OpenedBundle.file(p),
    {'refused': final String code} when code.isNotEmpty => OpenedBundle.refused(
      code,
    ),
    _ => null,
  };

  @override
  bool operator ==(Object other) =>
      other is OpenedBundle && other.path == path && other.refused == refused;

  @override
  int get hashCode => Object.hash(path, refused);

  @override
  String toString() => path != null
      ? 'OpenedBundle.file($path)'
      : 'OpenedBundle.refused($refused)';
}

/// The files the person opens in Clax through the platform: the Android
/// intent filters on `MainActivity` and the macOS document type.
abstract interface class BundleIntake {
  /// Each opened file, once: the ones that launched Clax first, then each
  /// one opened while it runs. Single-subscription; what arrives before the
  /// listener is kept for it.
  Stream<OpenedBundle> get opened;
}

/// [BundleIntake] over the `dev.empathic.clax/bundles` channel, served by
/// `BundleIntake.kt` on Android and `AppDelegate.swift` on macOS.
///
/// Dart to platform: `initial` → a list of {path} or {refused}, the files
/// that launched Clax; after it the platform sends each later file itself.
/// Platform to Dart: `opened` {path} or {refused}.
class ChannelBundleIntake implements BundleIntake {
  ChannelBundleIntake({this._channel = const MethodChannel(channelName)}) {
    _channel.setMethodCallHandler(_onCall);
    unawaited(_initial());
  }

  static const channelName = 'dev.empathic.clax/bundles';

  final MethodChannel _channel;
  final _opened = StreamController<OpenedBundle>();

  Future<void> _initial() async {
    Object? files;
    try {
      files = await _channel.invokeMethod<Object?>('initial');
    } on PlatformException {
      return;
    } on MissingPluginException {
      return;
    }
    if (files is! List) return;
    for (final f in files) {
      if (OpenedBundle.of(f) case final b?) _opened.add(b);
    }
  }

  Future<Object?> _onCall(MethodCall call) async {
    if (call.method == 'opened') {
      if (OpenedBundle.of(call.arguments) case final b?) _opened.add(b);
    }
    return null;
  }

  @override
  Stream<OpenedBundle> get opened => _opened.stream;
}

/// A [BundleIntake] for tests: [open] and [refuse] stand in for the
/// platform.
class FakeBundleIntake implements BundleIntake {
  final _opened = StreamController<OpenedBundle>();

  /// The person opened the file at [path].
  void open(String path) => _opened.add(OpenedBundle.file(path));

  /// The platform refused a file with [code].
  void refuse(String code) => _opened.add(OpenedBundle.refused(code));

  @override
  Stream<OpenedBundle> get opened => _opened.stream;
}
