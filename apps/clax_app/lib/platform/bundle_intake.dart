import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// What the platform hands Clax from outside it: a `.clax` file the person
/// opened (Files, a message, Finder) as a [path] to read, a refusal by the
/// platform before any byte reached Dart, or a Get in Clax link naming a
/// public artifact at [url].
@immutable
final class BundleIntakeEvent {
  /// The file at [path].
  ///
  /// On Android and iOS it is Clax's own copy, which the reader may delete
  /// once read; on macOS it is the person's file, which must not be
  /// deleted.
  const BundleIntakeEvent.file(String this.path) : refused = null, url = null;

  /// A file or link refused before any byte reached Dart, with a
  /// `BundleError` code: `too_large` (a file over 32 MiB), `not_a_bundle`
  /// (a file that could not be read) or `not_a_link` (a link
  /// [artifactOf] does not accept).
  const BundleIntakeEvent.refused(String this.refused)
    : path = null,
      url = null;

  /// A Get in Clax link, as the address of the public artifact it names,
  /// `https://<host>/a/<id>` ([artifactOf]).
  const BundleIntakeEvent.url(Uri this.url) : path = null, refused = null;

  final String? path;
  final String? refused;
  final Uri? url;

  /// Reads the platform's {path}, {refused} or {url}: a {url} that
  /// [artifactOf] does not accept is refused as `not_a_link`. Null for
  /// anything else.
  static BundleIntakeEvent? of(Object? args) => switch (args) {
    {'path': final String p} when p.isNotEmpty => BundleIntakeEvent.file(p),
    {'refused': final String code} when code.isNotEmpty =>
      BundleIntakeEvent.refused(code),
    {'url': final String link} => switch (artifactOf(link)) {
      final a? => BundleIntakeEvent.url(a),
      null => const BundleIntakeEvent.refused(notALink),
    },
    _ => null,
  };

  /// The refusal code of a link [artifactOf] does not accept.
  static const notALink = 'not_a_link';

  /// The public artifact that a Get in Clax [link] names, as
  /// `https://<host>[:<port>]/a/<id>`; null for any other link.
  ///
  /// Two forms are accepted:
  ///
  /// - `https://clax.dev/a/<id>/get` (the universal link and Android app
  ///   link; a trailing `/`, a query and a fragment are ignored), naming
  ///   `https://clax.dev/a/<id>`;
  /// - `clax:get?url=<artifact URL>`, the form any hosted Clax can give,
  ///   with exactly one `url` that is `https://<host>[:<port>]/a/<id>`
  ///   (a trailing `/` allowed), with no user info, query or fragment, and
  ///   a host of ASCII letters, digits, `-` and `.`, or an IPv6 address.
  ///
  /// `<id>` is an artifact ID: 12 characters of lowercase Crockford base32.
  static Uri? artifactOf(String link) {
    final u = Uri.tryParse(link);
    if (u == null) return null;
    switch (u.scheme) {
      case 'https':
        if (u.host != _claxDev || u.userInfo.isNotEmpty || u.hasPort) {
          return null;
        }
        final id = switch (_segments(u)) {
          ['a', final id, 'get'] when _artifactId.hasMatch(id) => id,
          _ => null,
        };
        if (id == null) return null;
        return Uri(scheme: 'https', host: _claxDev, path: '/a/$id');
      case 'clax':
        if (u.hasAuthority || u.path != 'get' || u.hasFragment) return null;
        final urls = u.queryParametersAll['url'];
        if (urls == null || urls.length != 1) return null;
        final inner = Uri.tryParse(urls.single);
        if (inner == null ||
            inner.scheme != 'https' ||
            !_host.hasMatch(inner.host) ||
            inner.userInfo.isNotEmpty ||
            inner.hasQuery ||
            inner.hasFragment) {
          return null;
        }
        return switch (_segments(inner)) {
          ['a', final id] when _artifactId.hasMatch(id) => Uri(
            scheme: 'https',
            host: inner.host,
            port: inner.hasPort ? inner.port : null,
            path: '/a/$id',
          ),
          _ => null,
        };
      default:
        return null;
    }
  }

  static const _claxDev = 'clax.dev';

  /// A host as letters, digits, `-` and `.` (a DNS name, punycode
  /// included, or an IPv4 address), or an IPv6 address: never a percent
  /// escape or a non-ASCII character.
  static final _host = RegExp(r'^(?:[a-z0-9.-]+|[0-9a-f:.]*:[0-9a-f:.]*)$');
  static final _artifactId = RegExp(r'^[0-9a-hjkmnp-tv-z]{12}$');

  /// [u]'s path segments without one trailing empty segment (a trailing
  /// `/`).
  static List<String> _segments(Uri u) {
    final segs = u.pathSegments;
    return segs.isNotEmpty && segs.last.isEmpty
        ? segs.sublist(0, segs.length - 1)
        : segs;
  }

  @override
  bool operator ==(Object other) =>
      other is BundleIntakeEvent &&
      other.path == path &&
      other.refused == refused &&
      other.url == url;

  @override
  int get hashCode => Object.hash(path, refused, url);

  @override
  String toString() => switch (this) {
    BundleIntakeEvent(path: final p?) => 'BundleIntakeEvent.file($p)',
    BundleIntakeEvent(refused: final r?) => 'BundleIntakeEvent.refused($r)',
    _ => 'BundleIntakeEvent.url($url)',
  };
}

/// The files and Get in Clax links the person opens in Clax through the
/// platform: the Android intent filters on `BundleReceiverActivity`, the
/// iOS and macOS document types, and the `clax:` URL scheme and clax.dev's
/// links.
abstract interface class BundleIntake {
  /// Each opened file or link, once: the ones that launched Clax first,
  /// then each one opened while it runs. Single-subscription; what arrives
  /// before the listener is kept for it.
  Stream<BundleIntakeEvent> get opened;
}

/// [BundleIntake] over the `dev.empathic.clax/bundles` channel, served by
/// `BundleIntake.kt` on Android, `BundleIntake.swift` on iOS and
/// `AppDelegate.swift` on macOS.
///
/// Dart to platform: `initial` → a list of {path}, {refused} or {url}, what
/// launched Clax; after it the platform sends each later one itself.
/// Platform to Dart: `opened` {path}, {refused} or {url}. A {url} is the
/// link as the platform got it; one that
/// [BundleIntakeEvent.artifactOf] does not accept is refused as
/// `not_a_link`.
class ChannelBundleIntake implements BundleIntake {
  ChannelBundleIntake({this._channel = const MethodChannel(channelName)}) {
    _channel.setMethodCallHandler(_onCall);
    unawaited(_initial());
  }

  static const channelName = 'dev.empathic.clax/bundles';

  final MethodChannel _channel;
  final _opened = StreamController<BundleIntakeEvent>();

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
      if (BundleIntakeEvent.of(f) case final b?) _opened.add(b);
    }
  }

  Future<Object?> _onCall(MethodCall call) async {
    if (call.method == 'opened') {
      if (BundleIntakeEvent.of(call.arguments) case final b?) _opened.add(b);
    }
    return null;
  }

  @override
  Stream<BundleIntakeEvent> get opened => _opened.stream;
}

/// A [BundleIntake] for tests: [open], [refuse] and [openLink] stand in
/// for the platform.
class FakeBundleIntake implements BundleIntake {
  final _opened = StreamController<BundleIntakeEvent>();

  /// The person opened the file at [path].
  void open(String path) => _opened.add(BundleIntakeEvent.file(path));

  /// The platform refused a file with [code].
  void refuse(String code) => _opened.add(BundleIntakeEvent.refused(code));

  /// The person followed [link]: delivered as [BundleIntakeEvent.url] when
  /// [BundleIntakeEvent.artifactOf] accepts it, refused as `not_a_link`
  /// otherwise, as [ChannelBundleIntake] does.
  void openLink(String link) {
    if (BundleIntakeEvent.of({'url': link}) case final e?) _opened.add(e);
  }

  @override
  Stream<BundleIntakeEvent> get opened => _opened.stream;
}
