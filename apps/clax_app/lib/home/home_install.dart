/// Installing the shipped home app (`assets/home/home.clax`) on the first
/// run, and finding it again on later ones.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:flutter/services.dart' show AssetBundle, rootBundle;

import '../bundle/installer.dart';
import '../bundle/manifest.dart';
import '../bundle/trust.dart';
import '../clax/api.dart';
import '../platform/bundle_key.dart';
import 'home_settings.dart';

/// What `assets/home/release.json` records of the shipped home bundle:
/// its [release] (semver), its manifest [version], [lineage] and author
/// [key] (base64url), and its content hash ([hash], `sha256:…`).
class HomeRelease {
  const HomeRelease({
    required this.release,
    required this.hash,
    required this.version,
    required this.lineage,
    required this.key,
  });

  final String release, hash, lineage, key;
  final int version;

  /// [json] as `release.json` holds it; throws [FormatException] when a
  /// member is missing or of the wrong type.
  factory HomeRelease.parse(String json) {
    final j = jsonDecode(json);
    if (j case {
      'release': final String release,
      'hash': final String hash,
      'version': final int version,
      'lineage': final String lineage,
      'key': final String key,
    }) {
      return HomeRelease(
        release: release,
        hash: hash,
        version: version,
        lineage: lineage,
        key: key,
      );
    }
    throw const FormatException('release.json lacks a member');
  }
}

/// The home bundle the app ships and its release record.
abstract interface class ShippedHome {
  Future<Uint8List> bundle();
  Future<HomeRelease> release();
}

/// [ShippedHome] from the app's assets.
class AssetShippedHome implements ShippedHome {
  AssetShippedHome([AssetBundle? assets]) : _assets = assets ?? rootBundle;

  final AssetBundle _assets;

  static const bundlePath = 'assets/home/home.clax';
  static const releasePath = 'assets/home/release.json';

  @override
  Future<Uint8List> bundle() async =>
      (await _assets.load(bundlePath)).buffer.asUint8List();

  @override
  Future<HomeRelease> release() async =>
      HomeRelease.parse(await _assets.loadString(releasePath, cache: false));
}

/// The shipped bundle is not the one its release record names.
class HomeInstallError implements Exception {
  const HomeInstallError(this.message);
  final String message;
  @override
  String toString() => 'HomeInstallError: $message';
}

/// A [BundleKey] with no seed: an [Installer] over it never reads the
/// keychain, and decides as an install that has not exported yet. The home
/// bundle is signed by its own release key, never this install's.
BundleKey homeInstallKey() => BundleKey(const _NoSeed());

class _NoSeed implements BundleSeedStorage {
  const _NoSeed();
  @override
  Future<String?> read() async => null;
  @override
  Future<void> write(String seed) async =>
      throw UnsupportedError('the home installer signs nothing');
}

/// The home app's artifact ID, installing the shipped bundle when there is
/// none: [HomeSettings.homeAppId] while the daemon has that app; else an
/// app on the shipped bundle's lineage and key that [installer]'s lineage
/// store knows (an earlier install whose ID was never saved, so the app is
/// not made twice); else a new install of [shipped] (the app's assets by
/// default). The ID found is saved in [settings].
///
/// The bundle must be the one `release.json` names: its author key,
/// lineage, version and content hash, besides the signature
/// [Installer.open] verifies against the key the bundle carries; else
/// [HomeInstallError]. Needs only the local daemon. Throws what
/// [ClaxApi] and [Installer] throw, with [settings] unchanged.
///
/// A new install first tells [installing] (`HomeHost.installing`) of the
/// app it makes: a future of its ID, which completes as soon as the
/// daemon made it (null when no app was made), so the home screen never
/// lists the home app before [settings] names it.
Future<String> ensureHomeApp(
  HomeSettings settings,
  Installer installer,
  ClaxApi api, {
  ShippedHome? shipped,
  void Function(Future<String?> made)? installing,
}) async {
  if (settings.homeAppId case final aid? when await _exists(api, aid)) {
    return aid;
  }
  final s = shipped ?? AssetShippedHome();
  final release = await s.release();
  final opened = await installer.open(await s.bundle());
  checkShipped(opened.manifest, release);
  final d = opened.decision;
  final aid = switch (d.answer) {
    TrustAnswer.own ||
    TrustAnswer.update ||
    TrustAnswer.current ||
    TrustAnswer.conflict ||
    TrustAnswer.older => d.aid!,
    TrustAnswer.install ||
    TrustAnswer.foreign ||
    TrustAnswer.forked => await _install(installer, opened, installing),
  };
  final was = settings.homeAppId;
  settings.homeAppId = aid;
  try {
    await settings.save();
  } on Object {
    settings.homeAppId = was;
    rethrow;
  }
  return aid;
}

/// [Installer.install] of [opened], telling [installing] of the app's ID
/// once it is made (null when the install failed first).
Future<String> _install(
  Installer installer,
  OpenedBundle opened,
  void Function(Future<String?> made)? installing,
) async {
  final made = Completer<String?>();
  installing?.call(made.future);
  try {
    return (await installer.install(opened, made: made.complete)).aid;
  } finally {
    if (!made.isCompleted) made.complete(null);
  }
}

/// Throws [HomeInstallError] unless [m] is the bundle [release] names:
/// its author key, lineage, version and content hash.
void checkShipped(Manifest m, HomeRelease release) {
  final mismatch = [
    if (m.author.key != release.key) 'author key',
    if (m.lineage != release.lineage) 'lineage',
    if (m.version != release.version) 'version',
    if (m.contentSha256 != release.hash) 'content hash',
  ];
  if (mismatch.isNotEmpty) {
    throw HomeInstallError(
      'the shipped home bundle differs from release.json in its '
      '${mismatch.join(', ')}',
    );
  }
}

Future<bool> _exists(ClaxApi api, String aid) async {
  try {
    await api.getArtifact(aid);
    return true;
  } on ClaxApiError catch (e) {
    if (e.isNotFound) return false;
    rethrow;
  }
}
